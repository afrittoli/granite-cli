//! What `provider info`, `launcher info`, `provider setup`, `launcher setup`
//! and `setup` show and do about installing a provider or launcher type.

// Local
use crate::utils::install::{InstallEnv, InstallInfo, Platform, Unavailable, assess};

/*-- public --*/

/// Show the install detail of a type as an "Install" block, unless `info`
/// holds nothing.
pub(crate) fn show_install_detail(ctx: &crate::AppContext, info: &InstallInfo) {
    let fields = install_fields(&*ctx.install_env(), info, ctx.ui.can_lend_terminal());
    if fields.is_empty() {
        return;
    }
    let fields: Vec<(&str, String)> = fields
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    ctx.ui.detail("Install", &fields);
}

/// The install detail of a type, as fields for `Ui::detail`: the program and
/// where it was found, each method for this operating system with whether it
/// runs here or why not, the run command, the start notes for this operating
/// system, the page and the note. Empty when `info` holds nothing.
///
/// Methods for other operating systems are left out. When a key repeats,
/// as "Install" does for several methods, each one is numbered ("Install 1",
/// "Install 2"), since the JSON backend keeps one value per key.
///
/// `allow_attended` is whether a method that asks for input may run, which
/// is `Ui::can_lend_terminal`. The verdict on an attended method therefore
/// depends on the output format of this run.
pub(crate) fn install_fields(
    env: &dyn InstallEnv,
    info: &InstallInfo,
    allow_attended: bool,
) -> Vec<(String, String)> {
    let platform = Platform::detect(env);
    let mut fields: Vec<(&'static str, String)> = Vec::new();

    if let Some(program) = &info.program {
        let found = match env.find(program) {
            Some(path) => format!("{program} ({})", path.display()),
            None => format!("{program} (not found on PATH)"),
        };
        fields.push(("Program", found));
    }

    for status in assess(info, &platform, allow_attended) {
        let verdict = match &status.unavailable {
            Some(Unavailable::WrongOs) => continue,
            Some(reason) => reason.to_string(),
            None => "✓ runs on this machine".to_string(),
        };
        fields.push(("Install", format!("{}  [{verdict}]", status.method.command)));
    }

    if let Some(run) = &info.run_command {
        fields.push(("Run", run.clone()));
    }
    for note in info.start_notes_for(platform.os) {
        fields.push(("Start", note.to_string()));
    }
    if let Some(page) = &info.page {
        fields.push(("Page", page.clone()));
    }
    if let Some(note) = &info.note {
        fields.push(("Note", note.clone()));
    }
    number_repeated_keys(fields)
}

/*-- private --*/

/// Turn each key that appears more than once into "<key> 1", "<key> 2", in
/// order.
fn number_repeated_keys(fields: Vec<(&'static str, String)>) -> Vec<(String, String)> {
    let count = |key: &str| fields.iter().filter(|(k, _)| *k == key).count();
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    fields
        .iter()
        .map(|(k, v)| {
            if count(k) > 1 {
                let n = seen.entry(k).or_insert(0);
                *n += 1;
                (format!("{k} {n}"), v.clone())
            } else {
                (k.to_string(), v.clone())
            }
        })
        .collect()
}

/*-- tests --*/

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::install::{InstallMethod, MethodKind, Os};
    use crate::utils::test_support::FakeInstallEnv;

    fn info() -> InstallInfo {
        InstallInfo::default()
            .program("tool")
            .page("https://example.com")
            .note("A note.")
            .run_command("tool serve")
            .start_note(&[Os::Mac], "brew services start tool")
            .start_note(&[Os::Linux], "systemctl start tool")
            .method(InstallMethod::new(
                &[Os::Mac],
                MethodKind::Brew,
                "brew install tool",
            ))
            .method(InstallMethod::new(
                &[Os::Linux],
                MethodKind::Npm,
                "npm i -g tool",
            ))
    }

    fn pair(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(k, v)| pair(k, v)).collect()
    }

    #[test]
    fn fields_list_this_os_only_with_verdicts() {
        let env = FakeInstallEnv::new(Os::Mac).with_programs(&["brew", "tool"]);
        let fields = install_fields(&env, &info(), false);
        assert_eq!(
            fields,
            pairs(&[
                ("Program", "tool (/usr/bin/tool)"),
                ("Install", "brew install tool  [✓ runs on this machine]"),
                ("Run", "tool serve"),
                ("Start", "brew services start tool"),
                ("Page", "https://example.com"),
                ("Note", "A note."),
            ])
        );
    }

    #[test]
    fn fields_give_the_reason_a_method_cannot_run() {
        let env = FakeInstallEnv::new(Os::Mac);
        let fields = install_fields(&env, &info(), false);
        assert!(fields.contains(&pair("Program", "tool (not found on PATH)")));
        assert!(fields.contains(&pair("Install", "brew install tool  [Homebrew not found]")));
    }

    #[test]
    fn repeated_keys_are_numbered() {
        let info = InstallInfo::default()
            .method(InstallMethod::new(
                &[Os::Linux],
                MethodKind::Npm,
                "npm i -g a",
            ))
            .method(InstallMethod::new(
                &[Os::Linux],
                MethodKind::Uv,
                "uv tool install a",
            ))
            .page("https://example.com");
        let env = FakeInstallEnv::new(Os::Linux).with_programs(&["npm"]);
        let keys: Vec<String> = install_fields(&env, &info, false)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, vec!["Install 1", "Install 2", "Page"]);
    }

    #[test]
    fn attended_method_runs_only_when_the_terminal_can_be_lent() {
        let info = InstallInfo::default()
            .method(InstallMethod::new(&[Os::Linux], MethodKind::Script, "curl x | sh").attended());
        let env = FakeInstallEnv::new(Os::Linux).with_programs(&["curl"]);
        assert_eq!(
            install_fields(&env, &info, false),
            pairs(&[(
                "Install",
                "curl x | sh  [asks for input; run it in a terminal]"
            )])
        );
        assert_eq!(
            install_fields(&env, &info, true),
            pairs(&[("Install", "curl x | sh  [✓ runs on this machine]")])
        );
    }

    #[test]
    fn empty_info_has_no_fields() {
        let env = FakeInstallEnv::default();
        assert!(install_fields(&env, &InstallInfo::default(), false).is_empty());
    }
}
