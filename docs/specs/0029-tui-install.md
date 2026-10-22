# Plan: Installing From the TUI

Issue: to be filed. Follows spec 0028.

## Overview

Spec 0028 lets `provider setup`, `launcher setup`, the setup wizard and
`setup --auto --install` install a missing provider or launcher. The
installer runs attached: it inherits granite-cli's terminal and standard
input, and its output goes to standard error. The TUI owns the terminal, so
spec 0028 leaves it out: when its setup pane finds a type missing, it shows
the manual instructions and installs nothing.

This spec makes the setup pane install too. It has to solve what attached
installs avoid: showing the installer's output inside the pane, answering or
refusing the installer's questions, and stopping the installer from the
pane.

The TUI runs `provider setup` and `launcher setup` in its setup pane through
`spawn_setup` in `src/utils/ui/app.rs`. The pane shows the command's output
lines and answers its prompts. `Esc` today takes effect at the next prompt.

## Use Cases

1. **Install from the setup pane.** Setting up a provider or launcher from
   the TUI makes the same offer as in a terminal, shows the installer's
   output in the pane as it arrives, and leaves the pane on the setup's next
   step when the install succeeds.
2. **Stop an install.** `Esc` during an install stops the installer and every
   process it started, closes the pane, and saves nothing.
3. **An installer that asks for input.** A method that may ask for input,
   such as one that calls `sudo`, either gets the terminal for its question
   or is shown as instructions, depending on decision 1.
4. **See what is missing before opening a type.** The Providers and Launchers
   catalogs mark types whose program is not found on PATH, or whose server
   is not answering.

## Proposal

### Design decisions

1. **How the installer reaches the user.** Two ways, to be decided:
   - **Captured into the pane.** The installer gets no input, its output
     lines go into the pane's output log, and it runs in its own process
     group (a process tree on Windows) so `Esc` can kill all of it. A method
     that may ask for input cannot get an answer, so the pane shows it as
     instructions. The spec 0028 implementation branch did this in its
     Sub-Task 9; that code is on `0029-starting-material`.
   - **Suspend the TUI.** The TUI leaves the alternate screen, restores the
     terminal, runs the installer attached as a terminal session does, then
     redraws. Questions work, Ctrl-C stops the installer, and no output
     capture or process group is needed. The pane does not show the output.
2. **A stop is not a failure.** Stopping an install reports "Install
   stopped", not a failure with manual instructions, and the rest of a
   wizard's chosen installs do not start.
3. **Installer output is shown safely.** Captured output has its control
   characters removed before it is shown, except tab.

### In the TUI

```
┌ providers — ollama ──────────────────────────────────────────────┐
│ Running health check...                                          │
│ ⚠ Ollama is not answering at http://localhost:11434.             │
│   'ollama' was not found on PATH. It may be installed elsewhere. │
│ Running: brew install ollama                                     │
│   │ ==> Fetching ollama                                          │
│   │ ==> Pouring ollama--…arm64_sequoia.bottle.tar.gz             │
│                                                                  │
└ [↑↓/jk] Scroll output  [Esc] Stop install ───────────────────────┘
```

## Open Questions

1. **Captured or suspended** (decision 1).
2. **Marking missing tools in the catalogs.** Showing "not found on PATH" or
   "not answering" in the Providers and Launchers rows needs a detection
   pass when the TUI opens, which it does not do today. How long may that
   take before the TUI is usable?
3. **A time limit for captured installs.** A captured installer that hangs,
   for example on a stalled download, has nobody watching its terminal.

## Out of Scope

- Everything spec 0028 covers outside the TUI.
- Installing a method's prerequisites, upgrading or uninstalling tools, as
  in spec 0028.

## What the spec 0028 implementation review found in the captured design

The spec 0028 implementation review covered the captured design, and found
these problems, which the captured option has to solve:

- Captured output lost bytes: `next_line` read into a buffer local to one
  `select!` branch, so a partial line was dropped whenever another branch,
  such as the 100 ms stop check, completed first.
- Only `Ctrl-C` killed the installer's process group; `SIGTERM` and `SIGHUP`
  left it running.
- A stop was reported as an install failure, and the wizard then started its
  remaining installs and stopped each one.
- `stop_requested` used `try_recv` and could swallow a queued answer, and
  `RUNNING_GROUP` held one process id.
- Raw installer output reached the plain and terminal backends.

## Sub-Tasks

### Sub-Task 1 — Install from the setup pane

**Intent**
Make the setup pane install a missing type, as decision 1 settles it.

**Expected Outcomes**

The pane offers the same installs as a terminal session. With the captured
design, installer output lines appear in the pane's output log as they
arrive, the footer reads "[Esc] Stop install" while an installer runs, and
`Esc` stops the installer and every process it started and closes the pane,
with nothing saved. With the suspended design, the TUI hands the terminal to
the installer and redraws afterwards.

Tests cover: output from a fake installer reaching the pane, or the TUI
suspending and resuming around a fake installer; the footer hint while an
installer runs; a stop reported as stopped, not failed.

**Relevant Context**
- `src/utils/ui/app.rs` (`spawn_setup`; the logger silenced while the TUI
  owns the terminal in `run_interactive_tui`)
- `src/utils/ui/setup_pane.rs` (`Esc` acts at the next prompt)
- `src/utils/ui/tui_ui.rs` (`OutputLine`)
- branch `0029-starting-material`: the captured design as the spec 0028
  implementation branch had it, on top of that implementation

**Status** — `[ ]` not started

---

### Sub-Task 2 — Mark missing tools in the catalogs

**Intent**
Show in the Providers and Launchers catalogs which types are missing.

**Expected Outcomes**

To be written after Open Question 2 is answered.

**Status** — `[ ]` not started
