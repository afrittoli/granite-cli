# Update Command & Create/Update Flow Consistency

## Problem Statement

`model`, `capability`, `provider`, and `launcher` each expose a `setup`
subcommand that is the *only* way to configure an instance — there is no
`update`. When the id passed to `setup` collides with an already-configured
instance, `setup` silently doubles as an editor for it: it asks a bare
"already configured. Overwrite?" and, on yes, re-runs the same
`prompt_from_schema` flow seeded with the existing values as defaults
(`src/commands/provider.rs:163-174`, `src/commands/capability.rs:150-158`,
`src/commands/model.rs:520-528`, `src/commands/launcher.rs:154-164`). This
works, but it's confusing for four independent reasons found while tracing
the four command modules:

**1. No dedicated `update` command, so intent is inferred from a coin-flip
prompt.** A user who runs `provider setup openai-compatible --id ollama`
meaning to change ollama's `base_url` gets exactly the same interaction as
someone who typo'd an id while trying to create something new. The command
name never tells you which one is about to happen; you find out from a
yes/no question.

**2. The collision prompt shows nothing about what it's about to overwrite.**
`"Provider instance 'ollama' is already configured. Overwrite?"` doesn't
show `ollama`'s current type, base URL, or any other field — the same detail
`provider info`-style output already knows how to render elsewhere is simply
not shown here. A user using `setup` as their update path (per point 1) has
no way to confirm they've got the right instance before agreeing to
re-prompt through its whole schema.

**3. Collision *detection* itself isn't consistent across the four
resources.** Provider and capability check only an exact `instance_id`
match. Launcher does that *and* a second check — scanning every configured
launcher for the same `launcher_type` under a *different* name — and, if
found, offers to redirect into updating that instead:

```rust
/// **Diverges from Provider setup**: scans all configured launchers for any
/// entry with the same `launcher_type` — not just the same `instance_id` —
/// and, if one exists under a different name, offers to either update that
/// existing entry or proceed with the new name. This lets the user avoid
/// accidentally creating duplicate configs for the same tool.
```
(`src/commands/launcher.rs:74-78`, logic at `launcher.rs:120-152`)

The doc comment names the divergence explicitly — this was a deliberate,
launcher-only addition, not an oversight, but it means the "did you mean to
update something?" experience depends on which resource type you're
configuring. Model has neither check as a nickname concern, because model
has no nickname at all (point 4).

**4. Model has no `instance_id`/nickname — the catalog `model_id` doubles as
the instance key — and its overwrite check runs *after* the interactive
variant-selection prompt**, not immediately after resolving the id like the
other three (`model.rs:499-528`: `Select model variant:` happens, then the
already-configured check). A user picks a variant, and only then learns the
model was already configured and must decide whether to discard that
selection.

**5. Selecting an existing resource has no shared pattern.** Dependency
resolution (`CapabilityCommands::resolve_model_dependency`,
`ModelCommands::select_provider`) builds a `ui.select` menu of existing
instances plus a "Configure a new X..." escape hatch — a good pattern, but
implemented twice, independently, with no shared helper. Meanwhile all four
top-level `remove` commands (`provider.rs:265`, `capability.rs:489`,
`launcher.rs:240`, `model.rs:706`) take a bare required `String` id and do no
listing or selection at all — if you don't already know the exact id from a
prior `list`, there's no way to pick from what's configured. Three different
UX patterns for "point at a configured instance," none shared.

**6. A correctness gap, found while tracing the overwrite path.** All three
nickname-bearing resources build their saved config directly from the
*argument* type, never checking it against the existing instance's stored
type:

```rust
let provider_config = crate::config::ProviderConfig {
    provider_id: instance_id.clone(),
    provider_type: provider_type.to_string(),   // <- from the `setup` argument
    config,
};
```
(`provider.rs:188-191`; the identical shape appears at `capability.rs` for
`CapabilityConfig` and `launcher.rs` for `LauncherConfig`)

Concretely: `provider setup <different-type> --id ollama`, where `ollama` is
already configured as some other type, shows the same generic "already
configured. Overwrite?" prompt — nothing calls out that the *type* would
also silently change — and answering yes retypes the instance in place. This
isn't exercised by any existing test (`provider.rs`, `capability.rs`,
`launcher.rs` test modules have no overwrite/retype coverage). It's a latent
bug independent of the UX issues above, but it strengthens the case for a
dedicated `update` path that structurally can't take a mismatched type,
rather than relying on a human reading a generic confirm prompt carefully.

### Summary table

| Resource   | Nickname separate from type? | Collision check scope | Shows existing config before asking? | Extra behavior |
|------------|:---:|---|:---:|---|
| provider   | yes | exact id only | no | — |
| capability | yes | exact id only | no | — |
| launcher   | yes | exact id **+** same-type/different-name scan | no | offers to redirect into updating a same-type sibling |
| model      | no (`model_id` is both) | exact id only, checked **after** variant selection | no | — |

## Goals

1. Add an explicit `update` subcommand to `model`, `capability`, `provider`,
   and `launcher`, symmetric with the existing `remove`, so the command name
   itself states intent instead of a prompt answer deciding it.
2. Make `setup`'s collision handling identical across all four resources:
   same detection scope, same messaging, same next step.
3. When `setup` finds a colliding id, show what's already configured there
   (reusing the existing `ui.detail` primitive `*_info` commands already
   use) before asking anything, and route a "yes" straight into the same
   code path `update` uses — no separate overwrite implementation to keep in
   sync.
4. Introduce one shared helper for "resolve which configured instance of
   resource X the user means" — used by `update`, `remove`, and the two
   existing dependency-resolution call sites — replacing three independent
   implementations (two `ui.select` call sites, plus `remove`'s
   argument-only lookup) with one.
5. Close the silent-retype gap (finding 6) as part of introducing `update`,
   not as an unrelated follow-up — `update` never takes a type argument, so
   the bug's shape (type argument overriding stored type) cannot occur on
   that path, and `setup`'s collision redirect (goal 3) reuses the existing
   stored type rather than the argument.

## Non-Goals

- **The bulk `granite-cli setup` wizard** (`docs/specs/0023-setup-wizard.md`,
  `src/commands/setup.rs`) — a separate, already-implemented top-level
  command that orchestrates all four resources together. Orthogonal to this
  spec's per-resource `update`; the only shared concern is naming — this
  spec's new subcommands are `<resource> update`, never bare `update`, so
  there's no collision with top-level `granite-cli setup`.
- **TUI / full-screen selection.** Stays within the existing
  `dialoguer`-backed `Ui::select`/`Ui::confirm` trait methods.
- **Config schema or on-disk format changes.**
- **`Ui::is_interactive()` / startup dangling-reference validation**
  (proposed separately for issue #90) — that spec's remediation loop will
  want to *call into* whatever `update` ends up looking like (its "Reconfigure"
  option is described as driving `capability setup <type> <id>`, which under
  this spec becomes `capability update <id>`), but designing that
  integration is out of scope here; this spec only needs to leave `update`
  in a shape that's easy to call from elsewhere (see Architecture).

## Design

### 1. `update`, symmetric with `remove`

For the three nickname-bearing resources, `update` takes only the instance
id — never a type argument, since the type is fixed at creation and read
back from the existing config:

```rust
#[derive(Subcommand, Debug)]
enum ProviderSubcommands {
    ...
    /// Update an existing provider instance's configuration
    Update {
        /// Configured provider instance ID to update
        provider_id: Option<String>,
    },
    ...
}
```

(same shape for `CapabilitySubcommands::Update { capability_id: Option<String> }`
and `LauncherSubcommands::Update { launcher_id: Option<String> }`)

For model, `update` is the direct, no-collision-detour path onto the
existing overwrite branch of `ModelCommands::setup` — same variant-reselect,
same provider-reselect, just entered directly instead of via a confirm:

```rust
Update {
    /// Configured model ID to update
    model_id: Option<String>,
},
```

`Option<String>` (not a required `String`) on all four so the id can be
omitted — see the shared resolution helper below. When given, an id that
doesn't match a configured instance is a hard error, mirroring `remove`'s
existing message:
`"No provider configured with id '{id}'. Run 'provider list' to see configured instances."`
(extending today's `remove` wording, `provider.rs:267`, with the added
pointer — the current message stops at "Nothing to remove.").

`update`'s implementation is almost entirely a re-slice of each `setup`'s
existing tail: look up `existing_config` (guaranteed `Some` by this point),
resolve `schema`/`defaults` from it exactly as today, call
`prompt_from_schema`, save. The type-registry lookup, the "type not found"
branch, and the instance-name prompt at the top of every `setup` today all
disappear from this path — they only exist to handle the create case.

### 2. Shared instance-resolution helper

New helper, one copy shared by all four resource modules — proposed home
`src/commands/instance.rs`:

```rust
/// Resolve which configured instance of a resource the caller means.
/// If `id` is `Some`, look it up directly (case for a positional arg the
/// user typed) — `Err` if it doesn't match. If `id` is `None`, present
/// every configured instance via `ui.select` and return the chosen one.
/// `configured` must be non-empty in the `None` case; callers check that
/// first so they can give a resource-specific "nothing configured yet"
/// message instead of an empty select menu.
pub fn resolve_instance_id(
    ui: &dyn Ui,
    kind: &str,               // "provider", "model", ...
    configured: &[String],    // sorted ids, same source `list` uses
    id: Option<&str>,
) -> anyhow::Result<String> {
    match id {
        Some(id) if configured.iter().any(|c| c == id) => Ok(id.to_string()),
        Some(id) => anyhow::bail!(
            "No {kind} configured with id '{id}'. Run '{kind} list' to see configured instances."
        ),
        None => {
            let index = ui.select(&format!("Select a {kind}:"), configured, 0)?;
            Ok(configured[index].clone())
        }
    }
}
```

This becomes the one place that decides how "point at a configured
instance" behaves, used by:

- `update` and `remove` for all four resources (replacing `remove`'s current
  argument-only lookup — this is the behavior change that actually answers
  "selection of existing resources is not consistent across commands").
- `ModelCommands::select_provider` and
  `CapabilityCommands::resolve_model_dependency`'s existing-instance branch
  — today's bespoke `ui.select(...)` calls at `model.rs:743-744` and
  `capability.rs:315-316` become calls into this helper (their
  "...Configure a new X" escape-hatch option stays layered on top, appended
  to `configured` before the call, same as today).

Non-interactive backends need no new handling: `ui.select` on
`JsonOutput`/`MarkdownOutput` already returns `non_interactive()`'s existing
"interactive prompts are not supported... rerun with --output=terminal or
--output=plain" error (`src/utils/ui/backends/json.rs:126-127`,
`.../markdown.rs:69-70`), so `resolve_instance_id`'s `None`-id branch
degrades the same way every other prompt already does — no separate
non-interactive fallback to design.

### 3. `setup`'s collision handling: show, then redirect into `update`

Replace each `setup`'s bare overwrite confirm with a shared sequence (a
second helper alongside `resolve_instance_id`, or inline in each `setup`
calling the same two steps — implementation detail, not load-bearing):

1. Exact-id lookup only (see point 4 below for the launcher same-type scan).
2. If found, render it: `ctx.ui.detail(&instance_id, &existing_fields)` —
   same primitive `capability info`/`provider info`-style commands already
   use, so the user sees the current type and config values, not just that
   the name is taken.
3. Ask a question that names the actual next step, not a bare "Overwrite?":
   `ui.confirm(&format!("Run 'update {instance_id}' now?"), false)`. On yes,
   call straight into the same function `update` calls — not a duplicate
   overwrite branch — passing the *existing* stored type, never the type
   argument `setup` was invoked with (this is what closes finding 6: the
   value that reaches `*Config { ..._type: ... }` after this point can only
   ever be the stored one).
4. On no: `"{kind} setup skipped."`, unchanged from today.

This directly answers the request to keep the "asked whether to update"
option while making "what's about to happen" explicit, and removes the
duplicate implementation of the overwrite/update logic that exists today
inside every `setup`.

### 4. Launcher's same-type-different-name scan: keep the note, drop the silent retype risk it doesn't actually have today, generalize the informational value

The scan at `launcher.rs:120-152` is useful — knowing "you already have a
`claude` launcher configured as `claude-remote`" before creating a second
one is good information, not launcher-specific. Two changes:

- **Keep it as create-time information, for all four resources, not just
  launcher.** When `setup` is about to create a genuinely new id and other
  instances of the same catalog type already exist, show the same kind of
  note (`"Note: a provider of type 'openai-compatible' already exists:
  ollama, lm-studio"`) so the user creating `lm-studio-2` finds out before
  committing, not after. This is informational only — it does not block or
  redirect anything.
- **Drop the redirect-into-update-a-different-id offer.** Creating a second
  named instance of the same catalog type is a legitimate, common case (it's
  the entire reason nicknames exist — see `provider.rs`'s own doc comment:
  "`openai-compatible` backing `llama-cpp`, `ollama`, `lm-studio`"), so it
  shouldn't require declining a confirm to proceed. If the user actually
  meant to change one of the listed instances, `update <that-id>` is now the
  explicit way to say so — per goal 1, intent should come from the command
  they type, not from answering a prompt correctly.

Net effect: launcher's `setup` loses its bespoke branch and the doc comment
calling out the divergence; all four resources get the same (simpler)
same-type note, and the doc comment at `launcher.rs:74-78` is deleted along
with the code it described.

## CLI Surface

| Command | Today | This spec |
|---|---|---|
| `provider setup <type> [--id]` | create **or** silent overwrite-on-collision | create only; collision shows existing config + offers `update` |
| `provider update [id]` | *(doesn't exist)* | update only; id optional → interactive picker |
| `provider remove <id>` | id required | id optional → interactive picker (unchanged when given) |
| `capability setup/update/remove` | same shape as provider, mirrored | same shape as provider, mirrored |
| `launcher setup/update/remove` | `setup` has the extra same-type redirect | same shape as provider (redirect removed, note generalized) |
| `model setup/update/remove` | `setup` has no nickname, overwrite-check runs after variant select | `setup` create-only, same-id check *before* variant select; `update` is the direct overwrite path |

## Example Flows

**Today**, updating a provider's URL:

```
$ granite-cli provider setup openai-compatible --id ollama
Setting up provider instance: openai-compatible
...
Provider instance 'ollama' is already configured. Overwrite?y
[re-prompts every field from scratch, seeded with old values]
```

**After this spec:**

```
$ granite-cli provider update ollama
Updating provider 'ollama' (openai-compatible):
  base_url: http://localhost:11434
  ...
[re-prompts every field, seeded with old values — same as today's overwrite path]
```

```
$ granite-cli provider update
Select a provider:
> ollama
  lm-studio
  llama-cpp
```

**`setup` hitting a collision, after this spec:**

```
$ granite-cli provider setup openai-compatible --id ollama
Setting up provider instance: openai-compatible
...
Instance name: ollama

'ollama' is already configured:
  Type: openai-compatible
  base_url: http://localhost:11434

Run 'update ollama' now? [y/N]
```

## Confirm Prompt Timing (side investigation)

Separately from the above: confirm prompts (`ui.confirm`, used throughout
every flow above) resolve on the first `y`/`n`/`Y`/`N` keystroke, without
waiting for Enter. This is not something granite-cli implemented — it's
`dialoguer::Confirm`'s documented default
(`~/.cargo/.../dialoguer-0.11.0/src/prompts/confirm.rs`):

```rust
/// Sets when to react to user input.
///
/// When `false` (default), we check on each user keystroke immediately as
/// it is typed. Valid inputs can be one of 'y', 'n', or a newline to accept
/// the default.
///
/// When `true`, the user must type their choice and hit the Enter key before
/// proceeding.
pub fn wait_for_newline(mut self, wait: bool) -> Self { ... }
```

`Ui::confirm`'s default implementation (`src/utils/ui/base.rs:213-218`)
never calls `.wait_for_newline(true)`, so it inherits the immediate-keystroke
default. There's no cross-CLI convention here — this is a per-library
choice, and libraries genuinely differ (readline-style prompts like `apt`'s
require Enter; some terminal UI toolkits treat a single keystroke as a
complete answer since a yes/no question has no use for edit-then-confirm).
What's more likely the actual source of confusion: `dialoguer`'s own
*other* prompt types (`Select`, `Input`) both still require Enter — only
`Confirm` defaults to immediate — so the inconsistency is arguably within
this one codebase's prompt behavior, not just "fast vs. slow."

**Recommended fix**, independent of everything else in this spec: add
`.wait_for_newline(true)` to `Ui::confirm`'s default impl. One line, one call
site (the trait default — every `ui.confirm(...)` call site in the codebase
is unaffected), makes `confirm` match `select`/`text`'s existing
type-then-Enter model. Worth landing as its own small commit rather than
folded into this spec's changes, since it touches every confirm prompt in
the codebase, not just the create/update ones.

## Implementation Details

Suggested order, each independently testable:

1. **`resolve_instance_id` helper** (`src/commands/instance.rs`) — pure
   function taking a `&dyn Ui` and data, no `Config`/registry coupling. Unit
   tests via `CaptureUi` (already used throughout `commands/*.rs` tests):
   `Some(existing_id)` → `Ok`, `Some(missing_id)` → `Err` with the expected
   message, `None` → drives `ui.select` and returns the chosen entry.
2. **`remove` for all four** — swap the `String` id argument for
   `Option<String>`, route through `resolve_instance_id`. Smallest possible
   behavior change; existing "not found" tests should need only message
   updates.
3. **`update` for all four** — new subcommand + command-layer function, each
   sliced from its `setup`'s existing overwrite-branch tail (schema lookup →
   `defaults` from existing config → `prompt_from_schema` → save → whatever
   `setup` does after saving, e.g. provider's health check, model's pull
   offer).
4. **`setup`'s collision handling** — replace the bare overwrite confirm
   with detail-then-redirect-into-`update`, for all four uniformly. This is
   the change that also closes finding 6 (retype gap), since the type value
   flowing into the save can now only come from the existing config on this
   path.
5. **Generalize/drop launcher's same-type scan** — replace
   `launcher.rs:120-152`'s redirect logic with the create-time note (applied
   to all four `setup`s, not launcher-only); delete the divergence doc
   comment at `launcher.rs:74-78`.
6. **`resolve_model_dependency`/`select_provider` reuse** — swap their
   existing-instance `ui.select` calls for `resolve_instance_id`, keeping
   the "Configure a new X..." append-then-select shape unchanged.
7. **(Independent) `Ui::confirm` `wait_for_newline(true)`** — can land
   before, after, or interleaved with the above; touches only
   `src/utils/ui/base.rs:213-218`.

## Testing Strategy

- **`resolve_instance_id`**: the three cases above, via `CaptureUi`.
- **`remove` with omitted id**: `CaptureUi` scripted to pick index 1 of 3;
  assert the right instance was removed and the other two remain.
- **`update`**: for each resource, a `Config` with one pre-existing
  instance; drive through `CaptureUi` with canned schema answers; assert the
  saved config reflects the new values and the *type* is unchanged from
  before the call (guards finding 6 directly — assert this even though
  `update` structurally can't take a type argument, as a regression guard on
  the helper `setup`'s collision path shares).
- **`setup` collision path**: pre-configure an instance, run `setup` with a
  colliding id and, separately, a colliding id *and a different type
  argument*; assert (a) the existing config's fields are shown via
  `ctx.ui.detail_prompts`/equivalent `CaptureUi` field before the confirm
  fires, and (b) on accept, the saved type equals the *original* stored
  type, not the argument — the regression test for finding 6.
- **Same-type note**: configure two providers of the same type; run `setup`
  creating a third with a new id; assert the note lists both existing ids
  and that no confirm/redirect prompt fires (this is the behavior change
  from launcher's current redirect-and-confirm to information-only).
- **Model's reordered check**: assert the "already configured" check now
  fires (and can short-circuit) *before* `Select model variant:` is ever
  prompted, using `CaptureUi`'s prompt-call ordering.
- **`wait_for_newline`**: `dialoguer::Confirm` interaction isn't exercised
  by `CaptureUi` (it's a real-terminal concern), so this is a manual/CHANGELOG
  note rather than a unit test — same as any other dialoguer-level behavior
  change in this codebase.

## Commit Sequence

| # | Message | Key changes |
|---|---------|--------------|
| 1 | `feat: add resolve_instance_id shared helper` | `src/commands/instance.rs`, unit tests |
| 2 | `feat: make remove's id argument optional across all resources` | `Option<String>` + `resolve_instance_id` in `provider.rs`/`capability.rs`/`launcher.rs`/`model.rs` |
| 3 | `feat: add update subcommand for provider, capability, launcher, model` | New `Update` variants in `main.rs`, new command-layer functions sliced from each `setup` |
| 4 | `feat: setup shows existing config and redirects into update on collision` | Replaces bare overwrite confirm in all four `setup`s; closes the retype gap |
| 5 | `refactor: generalize same-type note, drop launcher's redirect-into-update` | `launcher.rs:120-152` simplified; note logic shared across all four |
| 6 | `refactor: reuse resolve_instance_id in dependency-resolution selects` | `capability.rs` `resolve_model_dependency`, `model.rs` `select_provider` |
| 7 | `fix: make confirm prompts wait for Enter` | One-line `wait_for_newline(true)` in `src/utils/ui/base.rs`, independent of the rest |

## Success Criteria

- `granite-cli provider update <id>` (and the capability/launcher/model
  equivalents) exist, require no type argument, and error clearly on an
  unknown id.
- `granite-cli provider remove` (and friends) with no argument presents an
  interactive picker over currently configured instances; unchanged
  behavior when an id is given.
- `granite-cli provider setup <type> --id <existing-id>` shows the existing
  instance's current type and config before asking anything, and any
  "yes, proceed" answer saves the *existing* type regardless of what type
  argument was passed — reproducing finding 6's scenario now fails to
  retype the instance.
- Launcher's `setup` behaves identically to the other three resources'
  `setup` on a same-type-different-name collision (informational note, no
  redirect prompt); the diverging doc comment and branch are gone.
- `cargo test` covers all of the above without a real TTY.
- `cargo clippy -- -D warnings` and `cargo fmt --check` pass.

## Future Work (Out of Scope for This Spec)

- **Wiring `update` into the dangling-reference remediation flow** proposed
  for issue #90 — that spec's "Reconfigure" remediation option should call
  whatever `update` looks like after this spec lands, pre-seeding the
  broken instance's id, but the two specs can land independently in either
  order.
- **`--yes`/non-interactive flags for scripted create/update** — not raised
  by the original request; worth a separate spec if CI/scripting use cases
  for `setup`/`update` come up.
- **Fuzzy/partial id matching in `resolve_instance_id`** (e.g. suggesting
  "did you mean 'ollama'?" on a near-miss) — the helper as designed here
  does exact matching only, consistent with today's `remove` behavior; a
  nice-to-have, not required to close the inconsistency this spec targets.
