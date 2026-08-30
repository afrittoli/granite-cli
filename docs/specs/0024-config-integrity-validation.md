# Plan: Config Reference Integrity & Remediation

## Overview

Configured items reference each other by id (a capability's `model_id`, a
model's `provider_id`, a launcher's `enabled_capabilities`), and nothing
validates those references stay live. 

Configuration may become inconsistent upon removal (see #47) which causes
dangling references that may prevent from using one command until the issue
is fixed (see #90). It can also become inconsistent during creation, when a
nested "configure a new X" step silently fails to persist but is trusted
anyway.

This plan covers three aspects for the entire CLI:
- how to detect inconsistencies in configuration
- how to notify the user about them and help them fix such issues seamlessly
- how to prevent new inconsistencies from being introduced while configuring
  something else


## Proposal

We introduce a new `is_configured()` contract across `Model`, `Launcher`, and
`Capability`, which verifies if a specific instance is consistently configured.

- **`is_configured()`** checks are cascaded: must check the next hop's
  `is_configured`, not just construction success.
- **`is_configured()`** refers to static configuration only. Runtime checks
  are performed by `health_check()` instead (e.g. is a provider running)

```
┌────────────────────────────┐
│ Launcher::is_configured()  │
└──────────────┬─────────────┘
               │   every enabled capability must appear
               │   in CapabilitySource(..).instances()
               ▼
┌────────────────────────────┐
│ Capability::is_configured()│
└──────────────┬─────────────┘
               │   its model resolves, and that model
               │   is itself is_configured()
               ▼
┌────────────────────────────┐
│   Model::is_configured()   │
└──────────────┬─────────────┘
               │   provider_config() is Some
               ▼
┌────────────────────────────┐
│      Provider exists?      │
└────────────────────────────┘
```

`health_check()` (Sub-Task 7) walks the same chain but is a separate,
I/O-bound check — never part of this cascade, only invoked by `launch` and
`setup`.

Nested "configure a new X" flows (e.g. picking a provider while setting up
a model) can silently create a broken or unpersisted reference if the
nested step fails to save. We make these flows re-validate what they
created instead of trusting a bare `Ok(())`.

We define a per-command policy for what to do about a broken reference
once found (a list shouldn't interrupt with a prompt; a launch should).

### Command policy

| Command | On a broken reference |
|---|---|
| List (`model/capability/launcher/provider list`) | Inline annotation, never prompt |
| Info/detail (`model/capability info`) | Warning + offer remediation |
| Setup (create) | Remediation only for a dependency the wizard is about to use — see [open question](#open-questions)  |
| Setup (overwrite of an existing instance) | Visually flag defaults that are themselves broken |
| Launch | Offer remediation, abort by default if declined; then, separately, `health_check()` fail-fast (no remediation possible for a down service) |
| Remove | Warn and offer a choice before creating a new dangling reference |

Remediation is interactive-only, gated on **both** `Ui::is_interactive()`
*and* the command not being in an auto/non-prompting mode (e.g. `setup
--auto`) — `is_interactive()` alone only reflects the output backend, not a
command-level flag. Warnings always show, routed to stderr for `json`/`markdown`.

Note that a bug exists today, so `warn()` currently write to stdout for
`json`/`markdown`. This needs to be fixed and sent to stderr instead.

## Open Questions

- Should a broken candidate appear at all in a `setup` selection list
  (e.g. "Select a model for this capability")? Today `*Source::from_config`
  filters it out of `instances()` entirely.
- Should we implement multi-hop remediation within one session, when fixing
  one reference reveals another.

## Out of Scope (Future Work)

- Cross-process conflict resolution / file locking — granite-cli has no
  daemon; every invocation is load-run-exit, so this is a narrow race, not
  addressed here.


---

## Sub-Tasks

---

### Sub-Task 1 — `is_configured()` on `Model`, `Launcher`, `Capability`

**Intent**
One shared contract answering whether an instance's references still
resolve, checking real resolvability one hop at a time rather than mere
construction success.

**Expected Outcomes**

We add the following method to the `Model`, `Launcher`, and `Capability`
traits, defaulting to `Ok(())`. `Provider` has no outbound references, so it
doesn't need one.

```rust
fn is_configured(&self) -> Result<(), String>
```

For a model, this checks that its provider configuration is actually
present. For a capability, it checks that its model and provider
dependencies resolve, and that the resolved model is itself correctly
configured — resolving successfully isn't enough on its own. For a
launcher, it checks that every capability it enables appears among the
capabilities that have already passed this same check, rather than doing a
raw lookup into configuration. That distinction is what lets a launcher →
capability → model → provider chain be caught correctly, without the
launcher ever needing to know that models or providers exist. Today a
launcher's enabled capabilities are stored as a plain list of strings
rather than through the structured dependency mechanism capabilities
already use for their own references; this work brings it in line.

The three sources that construct these instances — for models, launchers,
and capabilities — apply this check when building their list of live
instances, skipping and warning about anything broken. That's the same
treatment already given to an unrecognized type name.

This does introduce some redundant work: resolving a model rebuilds its
source from scratch each time it's requested, and a launcher's check
builds its own capability source internally. In the worst case that's
proportional to the number of launchers times capabilities times models,
but it all stays in memory and runs once per process, so it should be
unnoticeable at realistic configuration sizes — memoizing it can be a
later optimization if it ever matters.

The doc comment on `is_configured()` should state this "one hop of real
resolvability, not construction success" rule directly on the method, not
only here.

Tests cover: one healthy and one dangling instance per source, checking
that only the dangling one is skipped; a full launcher → capability →
model → provider chain where only the provider is missing, confirming the
check genuinely cascades rather than stopping one hop deep; and a fake
provider whose health check panics if called, run through the same code
path, proving the cascade never performs I/O.

**Relevant Context**
- `src/models/mod.rs:33-70` (`ModelSource::from_config`), `86-126` (`take`)
- `src/capabilities/mod.rs` (`CapabilitySource::from_config`, current skip-and-warn shape via `is_healthy()`)
- `src/dependency/mod.rs` (`Requirement`/`Configured<U>`/`resolve()`)

**Status** — `[ ]` not started

---

### Sub-Task 2 — `validate()` helper + `Remove`-time check

**Intent**
A shared, reusable query for which instances in a source are broken, plus
a proactive check before `Remove` creates a new dangling reference.

**Expected Outcomes**

A pure helper function takes an already-built source and reports every
instance whose `is_configured()` check failed:

```rust
fn validate<U>(source: &impl Configured<U>) -> Vec<DanglingRef>

struct DanglingRef {
    kind: &'static str,    // "capability", "model", "launcher"
    instance_id: String,
    reason: String,        // is_configured()'s error message, verbatim
}
```

It lives in `src/config/validate.rs` and does nothing beyond what
constructing the source already does — no UI, no extra I/O.

Before any of the four removal methods on `Config` deletes an entry, we
scan the other three configuration maps for anything that depends on the
id being removed. If something does, the command layer — not `Config`
itself, per Spec 0001 — offers a choice: remove both together, cancel, or
remove only the item that was asked for. Non-interactive backends default
to removing only what was asked, with a warning on stderr.

```
⚠ Removing 'granite-3.1-8b-instruct' will break:
  - capability 'chat' (agent-model)

  [1] Remove 'granite-3.1-8b-instruct' and 'chat' together
  [2] Cancel — keep 'granite-3.1-8b-instruct'
  [3] Remove only 'granite-3.1-8b-instruct' — fix 'chat' later
>
```

Tests cover: running `validate` against a source seeded with several
known-dangling instances returns exactly the expected list; and, for the
`Remove`-time check, a model with one dependent capability produces the
right final configuration for each of the three prompt outcomes, including
the non-interactive default.

**Relevant Context**
- `src/config/mod.rs` (`remove_model` etc., all currently unconditional)
- `src/capabilities/base.rs` (`Dependency` enum, already structured per-instance)

**Status** — `[ ]` not started

---

### Sub-Task 3 — Fix non-interactive `warn()` to actually use stderr

**Intent**
Non-interactive output must stay clean and parseable; warnings can't be
mixed into the structured stream. This is broken today, independent of
everything else in this plan.

**Expected Outcomes**

Both non-interactive backends currently send warnings to standard output
instead of standard error. The JSON backend emits its warning through the
same writer as every other output, which is wired to stdout in production;
the Markdown backend's warning is a plain, unrouted print. Both need to
write to stderr instead.

Tests cover: capturing stdout and stderr separately for each backend,
calling `warn()`, and confirming nothing lands on stdout while the message
appears on stderr.

**Relevant Context**
- `JsonOutput::new` (`writer: Mutex::new(Box::new(std::io::stdout()))`)

**Status** — `[ ]` not started

---

### Sub-Task 4 — List annotation, info/detail remediation, setup-overwrite flagging

**Intent**
Wire `is_configured()`/`validate()` into the read- and configure-facing
command categories, each reacting the way the command policy table says
it should.

**Expected Outcomes**

List commands — for models, capabilities, launchers, and providers — gain
a status column, or an inline suffix, populated from `is_configured()`:

```
ID       PROVIDER   NOTES
model-1  ollama
model-2  ollama     ⚠ invalid provider
model-3  lm-studio
```

They never prompt: a test double that panics if `select` or `confirm` is
called must not panic when driven through a list containing a broken
entry. This should be configurable, defaulting to on.

Info and detail commands show a warning and offer the remediation prompt
from Sub-Task 5, gated on whether the session is interactive.

When `setup`'s overwrite flow presents an existing instance's current
values as defaults, any default that is itself a dangling reference should
be flagged inline in the prompt text.

Offering remediation during a fresh `setup`, for a dependency the wizard
is about to use, depends on resolving the open question above first, and
isn't implemented until then.

Tests cover: a UI double that panics on `select`/`confirm`, driven through
a list containing a broken entry, confirming the list never prompts; and,
for info/detail, canned answers for each remediation choice against a
broken instance, confirming the resulting configuration is correct.

**Relevant Context**
- `src/commands/model.rs`, `capability.rs`, `launcher.rs`, `provider.rs`
  (`list`/`info`/`setup` functions)

**Status** — `[ ]` not started

---

### Sub-Task 5 — Remediation prompt + `Ui::is_interactive()`

**Intent**
One reusable prompt that every remediation-offering command drives the
same way.

**Expected Outcomes**

The `Ui` trait gains a method for whether the current session can prompt
at all, defaulting to true and overridden to false only for the JSON and
Markdown backends — simpler and less fragile than detecting interactivity
by pattern-matching on the existing "non-interactive" error string.

```rust
fn is_interactive(&self) -> bool  // default: true
```

The prompt itself walks through broken references one at a time, offering
three choices: reconfigure the instance, which drives the existing
per-type setup command pre-selected on the right instance and type; remove
it, via the existing removal command; or skip it and leave it as-is. Skip
is also the automatic fallback whenever prompting isn't offered at all —
either because the session isn't interactive, or because the command
itself is running in a non-prompting mode such as `setup --auto` — except
for `launch`, which aborts by default instead of skipping.

```
⚠ Configuration issue (1 of 2)

  Capability 'chat' (agent-model) depends on model
  'granite-3.1-8b-instruct', which is no longer configured.

  [1] Reconfigure 'chat' now — pick a different model
  [2] Remove 'chat'
  [3] Skip for now — 'chat' stays disabled until fixed
>
```

Tests cover: canned answers confirming that reconfigure actually invokes
setup pre-selected on the right instance, that remove calls the right
removal function, and that neither a non-interactive session nor an
auto-mode flag ever reaches the underlying prompt call.

**Relevant Context**
- `src/commands/capability.rs` (`CapabilityCommands::setup`, reused by
  Reconfigure)

**Status** — `[ ]` not started

---

### Sub-Task 6 — Creation-time integrity

**Intent**
A nested "configure a new X on the fly" step that doesn't actually persist
must not be treated as a successful dependency resolution.

**Expected Outcomes**

Every insert method on `Config` updates its in-memory map before
attempting to save to disk, and every caller today treats a save failure
as a non-fatal warning rather than propagating it — so a nested setup step
can report success even though nothing was actually written. The fix gives
the two "configure a new X" helpers — the one used when resolving a
capability's model dependency, and the one used when selecting a provider
for a model — a shared check after the nested setup call returns: look the
resulting id up in configuration and confirm it's both present and
actually satisfies what was being asked for, rather than trusting the
return value alone.

Separately, the provider-selection helper doesn't currently re-validate
the id it returns at all. If a user picks "configure a new provider,"
types a name that collides with an existing provider, and declines to
overwrite it, the helper still returns that existing provider's id — even
if it doesn't actually satisfy what the model needs. It should get the
same re-validation the model-dependency helper already has.

Tests cover: inserting a provider into an unwritable configuration
directory returns an error while the new entry still shows up in memory,
which is the actual root cause this sub-task addresses; and, once fixed,
declining an overwrite onto a mismatched existing provider is rejected or
re-prompted rather than silently reused.

**Relevant Context**
- `src/config/mod.rs` (`insert_model`/`insert_provider`/`insert_capability`/`insert_launcher`, `save()`)
- `src/commands/capability.rs:334-352` (`resolve_model_dependency`)
- `src/commands/model.rs:722-770` (`select_provider`)

**Status** — `[ ]` not started

---

### Sub-Task 7 — `health_check()`: runtime dimension

**Intent**
A separate, I/O-bound liveness check, called only where it's actually
relevant — never folded into `is_configured()`.

**Expected Outcomes**

`Model`, `Capability`, and `Launcher` each gain a `health_check()` method
that delegates down the same chain `is_configured()` does: a model asks
its provider, a capability asks its resolved model, and a launcher
aggregates across the capabilities it has enabled.

`launch` gets a new pre-flight step, run after the existing check that the
binary exists. It checks `is_configured()` first, offering remediation and
aborting by default if that's declined, and only then runs
`health_check()`, failing fast with a clear, decoded error — for example,
"Provider 'ollama' is not reachable: `<error>`. Start it and try again." —
with no remediation offered, since there's nothing granite-cli can do
about an external service being down. Today `launch` only checks that the
binary exists; it says nothing about whether the service behind it is
actually reachable.

The three `setup` commands call `health_check()` too, but only as an
advisory warning shown after `is_configured()` passes — it never blocks
the setup from completing.

Tests cover: `launch` aborting by default when remediation is declined for
a dangling configuration reference, and separately failing fast when the
health check reports the service unreachable — in both cases before any
subprocess is actually spawned; and `setup` emitting a warning for the
same unreachable-provider scenario while still saving the configuration
successfully.

**Relevant Context**
- `src/providers/base.rs:159` (`Provider::health_check`, `HealthStatus`)
- `src/commands/launcher.rs` (current `launch` pre-flight: `validate_command()` only)

**Status** — `[ ]` not started
