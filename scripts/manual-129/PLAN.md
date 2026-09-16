# Manual test plan — issue #129 fixes

Covers the five fixes on `specs/0025-follow-up` from the outside: the wizard
prompts, `setup --auto` against detected launchers, and the warnings that load
time and `configure_all` print, none of which the unit suite drives end to end.

Every scenario writes a complete configuration under `GRANITE_CLI_HOME`
(`/tmp/granite-129` by default) and runs with `PATH` set to the scenario's own
stub directory, so nothing here touches your own configuration and discovery
never sees the launchers installed on this machine.

## Setup

```bash
cargo build
source scripts/manual-129/fixtures.sh
```

That defines `gc` (the debug binary against the scratch config), the scenario
functions and the helpers. `gc_show` prints the config tree, `gc_cleanup`
removes it and stops the fake Ollama.

Three things every scenario needs:

- **A detected launcher.** Discovery calls `validate_command`, which looks the
  launcher type's default command up on `PATH`. `gc_launcher_stub claude`
  writes an executable named `claude` into `$GC_HOME/bin`, and `gc` runs with
  `PATH` set to that directory alone, so a launcher you have installed for
  real is not detected. `GC129_KEEP_PATH=1` appends your own `PATH` again.
- **A healthy provider.** Discovery health-checks each provider type at its
  default endpoint. `gc_start_fake_ollama` answers `GET /api/tags` on port
  11434, so `ollama` comes back healthy. A real Ollama already listening there
  is used as it is.
- **A recommended config.** Each scenario writes its own under
  `recommended_configs/`, naming `granite-4.2-3b`, `granite-4.2-8b` or
  `granite-vision-4.1-4b` at a low `min_context_length`, so the models resolve
  on any machine. The built-in configs aim at `granite-4.2-30b` at 65536
  context, which does not resolve on a laptop, and a capability that fails to
  resolve looks exactly like one that was skipped on purpose.

To see the old behaviour in any scenario, check out the commit's parent and
rebuild: `git checkout <sha>~1 && cargo build`.

A run may also configure `openrouter` alongside `ollama`, since discovery
health-checks it too. It serves no model in these scenarios and can be ignored.

---

## 1. Each capability gets the model its recommended config lists

Commit `45a1abb`. The scenario's `claude.yaml` lists `granite-4.2-8b` for
`sub-agent-code` and `granite-4.2-3b` for `sub-agent-explore`.

```bash
gc_scenario_wizard_models
gc setup
```

In the prompts: keep `claude`, select `sub-agent-code` and `sub-agent-explore`
(check them if they are not pre-checked), and in the model step keep both
`granite-4.2-8b` and `granite-4.2-3b`.

```bash
gc_show
```

Expect `capabilities/sub-agent-code.yaml` to carry `model_id: granite-4.2-8b`
and `capabilities/sub-agent-explore.yaml` to carry `model_id: granite-4.2-3b`.
Run the whole scenario a second time and expect the same two, since the
fallback that used to decide this read a `HashSet` in iteration order.

Before the fix, both capabilities got whichever model came first out of that
set, so the two files named the same model and which one changed between runs.

## 2. `setup --auto` skips a capability the launcher cannot bind

Commit `b29e8b8`. The scenario's `bob.yaml` lists `agent-model` and
`vision-mcp`, and both resolve. `bob` binds only `BindingType::Mcp`.

```bash
gc_scenario_auto_bob
gc setup --auto
gc launcher list && gc capability list && gc model list
```

Expect the summary to read `Capabilities: vision-mcp` and `Models:
granite-vision-4.1-4b`. No `agent-model` capability, no `granite-4.2-3b`
model, and `launchers/bob.yaml` listing `vision-mcp` under
`enabled_capabilities`.

The mirror case, where `vision-mcp` is the one that cannot bind:

```bash
gc_scenario_auto_pi
gc setup --auto
gc capability list && gc model list
```

Expect `Capabilities: agent-model` and `Models: granite-4.2-3b`, with no
`granite-vision-4.1-4b`.

Before the fix, both scenarios configured the capability and its model, and
`configure_all` then enabled neither, so the entries sat unused.

## 3. A second `setup --auto` run keeps what the first one configured

Commit `0a3d0cf`.

```bash
gc_scenario_auto_first_run
gc setup --auto
gc_show
```

Expect `models/granite-4.2-3b.yaml` with `provider_id: ollama` and `variant:
Ollama/Q4_K_M`, a capability `agent-model` naming that model, and
`launchers/claude.yaml`.

Now change the model's variant by hand, to something the recommendation would
not have picked, and add a launcher that was not installed during the first
run:

```bash
sed -i '' 's|^variant: .*|variant: Ollama/Q8_0|' $GC_HOME/models/granite-4.2-3b.yaml
gc_scenario_auto_second_run
gc setup --auto
gc_show
```

Expect:

- **A summary reading `Models: none` and `Capabilities: none`**, with
  `Launchers: goose`. Those two lines report what this run selected, not what
  is configured: the model and the capability were left alone because they
  already exist.
- **The model file unchanged**, still `Ollama/Q8_0` with `provider_id:
  ollama`.
- **The capability unchanged**, still naming `granite-4.2-3b`.
- **`launchers/goose.yaml` written**, with `agent-model` under
  `enabled_capabilities`.

Before the fix, the second run found no healthy provider, because discovery
skips a configured one, so nothing resolved and `goose` got no capabilities.
Where a model did resolve, it was rewritten with the recommended provider and
variant, discarding the edit above.

## 4. A recommended config that does not load recommends nothing

Commit `1447ef4`. The fixture writes `recommended_configs/claude.yaml` with
`variant_precision:`, one letter short of `variant_precisions:`. Both this
scenario and the next write the same file, and the spelling is the only
difference between them.

```bash
gc_scenario_bad_recommended
gc capability list
```

Expect two warnings at load, before the table:

- `Skipping config file: Failed to parse config file: .../claude.yaml:
  capabilities[0].models.model_id.models[0]: unknown field `variant_precision`,
  expected one of `model`, `variant_formats`, `variant_precisions``
- `Recommended config .../claude.yaml was not loaded: setup recommends no
  capabilities for launcher "claude" until the file is fixed or removed`

Then:

```bash
gc setup --auto
gc capability list
```

Expect `Models: none` and `Capabilities: none`, since `claude`'s recommended
config is now the empty entry.

The same file with the spelling fixed, which the next scenario writes for you:

```bash
gc_scenario_good_recommended
gc setup --auto
gc capability list
```

Expect no warnings, `Capabilities: agent-model` and `Models: granite-4.2-3b`.

Before the fix, the broken file was dropped silently and `claude` fell back to
the built-in configuration, which is the one the file was written to replace.
With the typo, the precision allow-list was ignored, so any precision of the
model was admitted.

## 5. A capability with no model is skipped once

Commit `59bb91a`.

```bash
gc_scenario_no_models
gc setup
```

In the prompts: keep `claude`, keep `agent-model`, and in the model step
deselect every model.

Expect exactly one `Skipping 'agent-model': no compatible model available.`
line, no `capabilities/agent-model.yaml`, and `gc capability list` empty.

Before the fix, that line printed twice: once from the per-slot loop and once
from the check after it.

## Cleanup

```bash
gc_cleanup
```
