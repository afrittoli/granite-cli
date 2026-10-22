/// A [`Ui`] implementation that drives interactive prompts through the ratatui
/// TUI rather than blocking the terminal directly.
///
/// Setup commands call the normal `Ui` methods (`select`, `text`, `confirm`,
/// …).  Instead of showing a dialoguer widget, each call:
///
/// 1. Sends a [`Prompt`] describing what to display over `prompt_tx`.
/// 2. Blocks on `answer_rx` until the TUI event loop delivers an [`Answer`].
///
/// The TUI side renders the current prompt inside the right pane, routes key
/// events to it, and sends back an `Answer` when the user confirms.
///
/// Non-interactive output methods (`info`, `warn`, `error`, …) append lines
/// to a shared `Vec<OutputLine>` that the TUI displays above the active prompt.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::utils::ui::base::{PullHandle, Ui};

/*-- public --*/

/// A single line of non-interactive output emitted during setup.
#[derive(Debug, Clone)]
pub struct OutputLine {
    pub level: OutputLevel,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutputLevel {
    Info,
    Warn,
    Error,
}

/// Live state for one active or completed pull/download.
#[derive(Debug, Clone)]
pub struct PullState {
    pub label: String,
    /// `None` means total is not yet known (spinner mode).
    pub total: Option<u64>,
    pub downloaded: u64,
    pub done: bool,
    pub error: Option<String>,
}

/// A prompt the setup wizard needs answered.
#[derive(Debug, Clone)]
pub enum Prompt {
    /// Pick one item from a list.  Returns the chosen index.
    Select {
        message: String,
        items: Vec<String>,
        default: usize,
    },
    /// Pick zero or more items from a list.  Returns the chosen indices.
    MultiSelect {
        message: String,
        items: Vec<String>,
        defaults: Vec<bool>,
    },
    /// Yes/no question.  Returns `true`/`false`.
    Confirm { message: String, default: bool },
    /// Free-text entry.  Returns the entered string.
    Text {
        message: String,
        default: String,
        allow_empty: bool,
        /// When true the pane renders typed characters as `*`.
        password: bool,
    },
}

/// The answer the TUI sends back for one [`Prompt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Index(usize),
    Indices(Vec<usize>),
    Bool(bool),
    Text(String),
}

pub struct TuiUi {
    /// Prompts sent to the TUI render loop.
    pub prompt_tx: std::sync::mpsc::SyncSender<Prompt>,
    /// Answers received from the TUI render loop.
    answer_rx: Mutex<std::sync::mpsc::Receiver<Answer>>,
    /// Lines of non-interactive output accumulated so far.
    pub output: Arc<Mutex<Vec<OutputLine>>>,
    /// Live pull/download states, keyed by handle id.
    pub pulls: Arc<Mutex<HashMap<u64, PullState>>>,
    /// Counter for allocating pull handles.
    next_handle: Mutex<u64>,
    /// True while an install command runs, read by the setup pane footer.
    installing: Arc<std::sync::atomic::AtomicBool>,
}

impl TuiUi {
    pub fn new(
        prompt_tx: std::sync::mpsc::SyncSender<Prompt>,
        answer_rx: std::sync::mpsc::Receiver<Answer>,
        output: Arc<Mutex<Vec<OutputLine>>>,
        pulls: Arc<Mutex<HashMap<u64, PullState>>>,
        installing: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            prompt_tx,
            answer_rx: Mutex::new(answer_rx),
            output,
            pulls,
            next_handle: Mutex::new(0),
            installing,
        }
    }

    fn ask(&self, prompt: Prompt) -> anyhow::Result<Answer> {
        self.prompt_tx
            .send(prompt)
            .map_err(|_| anyhow::anyhow!("TUI setup pane closed unexpectedly"))?;
        self.answer_rx
            .lock()
            .unwrap()
            .recv()
            .map_err(|_| anyhow::anyhow!("TUI setup pane closed unexpectedly"))
    }

    fn push_line(&self, level: OutputLevel, text: &str) {
        if let Ok(mut guard) = self.output.lock() {
            // Split on embedded newlines so each ratatui `Line` stays single-line.
            for part in text.split('\n') {
                guard.push(OutputLine {
                    level: level.clone(),
                    text: part.to_string(),
                });
            }
        }
    }
}

impl Ui for TuiUi {
    /// Prompts go to the TUI's own pane over `prompt_tx`, so a TUI session
    /// can ask whatever stdin and stderr are connected to.
    fn is_interactive(&self) -> bool {
        true
    }

    fn install_running(&self, running: bool) {
        self.installing
            .store(running, std::sync::atomic::Ordering::SeqCst);
    }

    /// The pane drops its end of the answer channel when the user presses
    /// `Esc`, which is how a running install learns to stop.
    fn stop_requested(&self) -> bool {
        matches!(
            self.answer_rx.lock().map(|rx| rx.try_recv()),
            Ok(Err(std::sync::mpsc::TryRecvError::Disconnected))
        )
    }

    fn table(&self, _title: &str, _headers: &[&str], _rows: &[Vec<String>]) {}
    fn detail(&self, _title: &str, _fields: &[(&str, String)]) {}

    fn status(&self, label: &str, ok: bool, detail: &str) {
        let mark = if ok { "✓" } else { "✗" };
        self.push_line(
            if ok {
                OutputLevel::Info
            } else {
                OutputLevel::Warn
            },
            &format!("{mark} {label}  {detail}"),
        );
    }

    fn info(&self, msg: &str) {
        self.push_line(OutputLevel::Info, msg);
    }
    fn warn(&self, msg: &str) {
        self.push_line(OutputLevel::Warn, msg);
    }
    fn error(&self, msg: &str) {
        self.push_line(OutputLevel::Error, msg);
    }

    fn pull_start(&self, label: &str, total_bytes: Option<u64>) -> PullHandle {
        let id = {
            let mut n = self.next_handle.lock().unwrap();
            let id = *n;
            *n += 1;
            id
        };
        self.pulls.lock().unwrap().insert(
            id,
            PullState {
                label: label.to_string(),
                total: total_bytes,
                downloaded: 0,
                done: false,
                error: None,
            },
        );
        PullHandle(id)
    }

    fn pull_progress(&self, handle: PullHandle, downloaded: u64, total: Option<u64>) {
        if let Ok(mut guard) = self.pulls.lock()
            && let Some(state) = guard.get_mut(&handle.0)
        {
            state.downloaded = downloaded;
            if total.is_some() {
                state.total = total;
            }
        }
    }

    fn pull_finish(&self, handle: PullHandle, label: &str, error: Option<&str>) {
        if let Ok(mut guard) = self.pulls.lock()
            && let Some(state) = guard.get_mut(&handle.0)
        {
            state.done = true;
            state.error = error.map(|e| e.to_string());
        }
        // Also push a summary line to the output log.
        match error {
            None => self.push_line(OutputLevel::Info, &format!("✓ {label} — done")),
            Some(e) => self.push_line(OutputLevel::Error, &format!("✗ {label}: {e}")),
        }
    }

    fn select(&self, prompt: &str, items: &[String], default: usize) -> anyhow::Result<usize> {
        match self.ask(Prompt::Select {
            message: prompt.to_string(),
            items: items.to_vec(),
            default,
        })? {
            Answer::Index(i) => Ok(i),
            _ => anyhow::bail!("unexpected answer type for select"),
        }
    }

    fn multi_select(
        &self,
        prompt: &str,
        items: &[String],
        defaults: &[bool],
    ) -> anyhow::Result<Vec<usize>> {
        match self.ask(Prompt::MultiSelect {
            message: prompt.to_string(),
            items: items.to_vec(),
            defaults: defaults.to_vec(),
        })? {
            Answer::Indices(v) => Ok(v),
            _ => anyhow::bail!("unexpected answer type for multi_select"),
        }
    }

    fn confirm(&self, prompt: &str, default: bool) -> anyhow::Result<bool> {
        match self.ask(Prompt::Confirm {
            message: prompt.to_string(),
            default,
        })? {
            Answer::Bool(b) => Ok(b),
            _ => anyhow::bail!("unexpected answer type for confirm"),
        }
    }

    fn text(&self, prompt: &str, default: &str) -> anyhow::Result<String> {
        match self.ask(Prompt::Text {
            message: prompt.to_string(),
            default: default.to_string(),
            allow_empty: false,
            password: false,
        })? {
            Answer::Text(s) => Ok(s),
            _ => anyhow::bail!("unexpected answer type for text"),
        }
    }

    fn text_optional(&self, prompt: &str, default: &str) -> anyhow::Result<String> {
        match self.ask(Prompt::Text {
            message: prompt.to_string(),
            default: default.to_string(),
            allow_empty: true,
            password: false,
        })? {
            Answer::Text(s) => Ok(s),
            _ => anyhow::bail!("unexpected answer type for text_optional"),
        }
    }

    fn password(&self, prompt: &str) -> anyhow::Result<String> {
        match self.ask(Prompt::Text {
            message: prompt.to_string(),
            default: String::new(),
            allow_empty: true,
            password: true,
        })? {
            Answer::Text(s) => Ok(s),
            _ => anyhow::bail!("unexpected answer type for password"),
        }
    }
}

// Setup tasks run on a blocking thread; Ui requires Send + Sync.
unsafe impl Send for TuiUi {}
unsafe impl Sync for TuiUi {}

/*-- tests --*/

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::shared::install::{InstallOutcome, install_unattended, offer_install};
    use crate::utils::install::{InstallEnv, InstallInfo, InstallMethod, MethodKind, Os, RunMode};
    use crate::utils::test_support::{FakeInstallEnv, FakeRun};
    use crate::utils::ui::setup_pane::SetupPane;

    /// A `TuiUi` and the `SetupPane` it talks to, wired as `spawn_setup`
    /// wires them.
    fn wired() -> (Arc<TuiUi>, SetupPane) {
        let (prompt_tx, prompt_rx) = std::sync::mpsc::sync_channel(0);
        let (answer_tx, answer_rx) = std::sync::mpsc::sync_channel(0);
        let output = Arc::new(Mutex::new(Vec::new()));
        let pulls = Arc::new(Mutex::new(HashMap::new()));
        let installing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ui = Arc::new(TuiUi::new(
            prompt_tx,
            answer_rx,
            output.clone(),
            pulls.clone(),
            installing.clone(),
        ));
        let pane = SetupPane::new("t".to_string(), output, pulls, prompt_rx, answer_tx)
            .with_install_flag(installing);
        (ui, pane)
    }

    fn tool_info() -> InstallInfo {
        InstallInfo::default()
            .program("tool")
            .method(InstallMethod::new(
                &[Os::Linux],
                MethodKind::Npm,
                "npm install -g tool",
            ))
    }

    fn ctx_with(ui: Arc<TuiUi>, env: Arc<FakeInstallEnv>) -> crate::AppContext {
        let mut ctx = crate::AppContext::new(crate::config::Config::default(), ui);
        ctx.set_install_env(env);
        ctx
    }

    #[test]
    fn the_tui_cannot_lend_the_terminal() {
        let (ui, _pane) = wired();
        assert!(!ui.can_lend_terminal());
    }

    #[tokio::test]
    async fn installer_output_reaches_the_pane() {
        let (ui, pane) = wired();
        let env = Arc::new(
            FakeInstallEnv::new(Os::Linux)
                .with_programs(&["npm"])
                .with_run(
                    "npm install -g tool",
                    FakeRun::ok(&["added 1 package"]).installs("tool", "/usr/bin/tool"),
                ),
        );
        let ctx = ctx_with(ui, env.clone());

        let outcome = install_unattended(&ctx, "Tool", &tool_info())
            .await
            .unwrap();

        assert!(matches!(outcome, InstallOutcome::Installed { .. }));
        assert_eq!(
            env.runs(),
            vec![("npm install -g tool".to_string(), RunMode::Captured)]
        );
        let lines: Vec<String> = pane
            .output
            .lock()
            .unwrap()
            .iter()
            .map(|l| l.text.clone())
            .collect();
        assert!(lines.contains(&"Running: npm install -g tool".to_string()));
        assert!(lines.contains(&"  │ added 1 package".to_string()));
    }

    #[tokio::test]
    async fn esc_reaches_the_install_environment_as_a_stop_request() {
        let (ui, mut pane) = wired();
        assert!(!ui.stop_requested());

        pane.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(pane.finished);
        drop(pane); // the TUI drops a finished pane
        assert!(ui.stop_requested());

        let env = Arc::new(
            FakeInstallEnv::new(Os::Linux)
                .with_programs(&["npm"])
                .with_run(
                    "npm install -g tool",
                    FakeRun::ok(&[]).installs("tool", "/x"),
                ),
        );
        let ctx = ctx_with(ui, env.clone());
        let outcome = install_unattended(&ctx, "Tool", &tool_info())
            .await
            .unwrap();

        assert_eq!(outcome, InstallOutcome::Failed);
        assert_eq!(
            env.find("tool"),
            None,
            "a stopped install puts nothing on PATH"
        );
    }

    /// The pane's side is a thread that answers the first prompt with its
    /// first item and returns the items it was shown, with the answer sender.
    #[tokio::test]
    async fn an_offer_in_the_pane_leaves_out_attended_methods_and_runs_captured() {
        let (prompt_tx, prompt_rx) = std::sync::mpsc::sync_channel(0);
        let (answer_tx, answer_rx) = std::sync::mpsc::sync_channel(0);
        let ui = Arc::new(TuiUi::new(
            prompt_tx,
            answer_rx,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ));
        let pane = std::thread::spawn(move || match prompt_rx.recv().unwrap() {
            Prompt::Select { items, .. } => {
                answer_tx.send(Answer::Index(0)).unwrap();
                // A closed answer channel reads as Esc, so keep it open.
                (items, answer_tx)
            }
            _ => panic!("expected a select"),
        });
        let info = tool_info().method(
            InstallMethod::new(
                &[Os::Linux],
                MethodKind::Script,
                "curl -fsSL https://example.com/install.sh | sh",
            )
            .attended(),
        );
        let env = Arc::new(
            FakeInstallEnv::new(Os::Linux)
                .with_programs(&["npm", "curl"])
                .with_run(
                    "npm install -g tool",
                    FakeRun::ok(&[]).installs("tool", "/usr/bin/tool"),
                ),
        );
        let ctx = ctx_with(ui, env.clone());

        let outcome = offer_install(&ctx, "Tool", &info).await.unwrap();

        let (items, _answer_tx) = pane.join().unwrap();
        assert_eq!(
            items,
            vec![
                "npm install -g tool".to_string(),
                "Don't install (show manual instructions)".to_string()
            ]
        );
        assert!(matches!(outcome, InstallOutcome::Installed { .. }));
        assert_eq!(
            env.runs(),
            vec![("npm install -g tool".to_string(), RunMode::Captured)]
        );
    }

    #[test]
    fn install_running_sets_the_flag_the_pane_reads() {
        let (ui, pane) = wired();
        ui.install_running(true);
        assert!(pane.hint().contains("[Esc] Stop install"));
        ui.install_running(false);
        assert!(!pane.hint().contains("Stop install"));
    }

    #[test]
    fn is_always_interactive() {
        let (ui, _pane) = wired();
        assert!(ui.is_interactive());
    }
}
