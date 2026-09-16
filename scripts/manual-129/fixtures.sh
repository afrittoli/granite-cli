#!/usr/bin/env bash
# Fixtures for the manual test plan of the issue #129 fixes
# (branch specs/0025-follow-up).
#
#   cargo build
#   source scripts/manual-129/fixtures.sh
#   gc_scenario_auto_bob
#   gc setup --auto
#
# `gc` runs the debug binary with GRANITE_CLI_HOME pointed at a scratch tree
# and PATH set to $GC_HOME/bin alone, so launcher discovery finds only the
# stub binaries a scenario created, and nothing here touches your own config
# or sees the launchers installed on this machine.
#
# This file is sourced into an interactive shell, so it sets no shell options:
# `set -u` here would stay set afterwards and break unrelated things.

# Own variables, so sourcing this after scripts/manual-0025/fixtures.sh in the
# same shell does not inherit that plan's GC_HOME and write into its tree.
GC_HOME="${GC129_HOME:-/tmp/granite-129}"
GC_BIN="${GC129_BIN:-$PWD/target/debug/granite-cli}"

# Discovery health-checks each provider type at that type's default endpoint,
# so the fake server has to answer on Ollama's default port.
GC_OLLAMA_PORT="${GC_OLLAMA_PORT:-11434}"

# PATH holds the stub directory alone, so launcher discovery cannot see the
# launchers installed on this machine. Set GC129_KEEP_PATH=1 to append the
# real PATH, for a scenario that needs a tool from it.
gc() {
  local gc_path="$GC_HOME/bin"
  [ -n "${GC129_KEEP_PATH:-}" ] && gc_path="$gc_path:$PATH"
  GRANITE_CLI_HOME="$GC_HOME" PATH="$gc_path" "$GC_BIN" "$@"
}

gc_reset() {
  rm -rf "$GC_HOME"
  mkdir -p "$GC_HOME"/models "$GC_HOME"/providers "$GC_HOME"/capabilities \
           "$GC_HOME"/launchers "$GC_HOME"/recommended_configs "$GC_HOME"/bin
}

gc_cleanup() {
  gc_stop_fake_ollama
  rm -rf "$GC_HOME"
  echo "removed $GC_HOME"
}

gc_show() {
  echo "--- $GC_HOME ---"
  find "$GC_HOME" -name '*.yaml' | sort | while read -r f; do
    echo "== ${f#"$GC_HOME"/}"
    sed 's/^/   /' "$f"
  done
}

# -- building blocks --------------------------------------------------------

# gc_launcher_stub <name>...
#
# Launcher discovery constructs each registered type with its default config
# and calls validate_command, which looks the type's default command up on
# PATH. An executable of that name is all it takes to be "detected".
gc_launcher_stub() {
  local name
  for name in "$@"; do
    printf '#!/bin/sh\necho "stub launcher %s: $*"\n' "$name" > "$GC_HOME/bin/$name"
    chmod +x "$GC_HOME/bin/$name"
  done
  echo "stub launchers: $*"
}

# A fake Ollama answering the health check (GET /api/tags) and chat POSTs, so
# provider discovery reports ollama healthy without a real install. A real
# Ollama already listening on the port is used as it is.
gc_start_fake_ollama() {
  gc_stop_fake_ollama
  mkdir -p "$GC_HOME"
  if curl -s -m 2 -o /dev/null "http://127.0.0.1:${GC_OLLAMA_PORT}/api/tags"; then
    echo "port $GC_OLLAMA_PORT already answers /api/tags, using it"
    return 0
  fi
  python3 - "$GC_OLLAMA_PORT" > "$GC_HOME/ollama.log" 2>&1 <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def _send(self, obj):
        body = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        print(f"GET {self.path}", flush=True)
        self._send({"models": []})

    def do_POST(self):
        n = int(self.headers.get("content-length") or 0)
        req = json.loads(self.rfile.read(n) or b"{}")
        print(f"POST {self.path} model={req.get('model')}", flush=True)
        self._send({"model": req.get("model"), "response": "ok"})

    def log_message(self, *a):
        pass

HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
  echo $! > "$GC_HOME/.fake-ollama.pid"
  sleep 0.4
  if ! curl -s -m 2 -o /dev/null "http://127.0.0.1:${GC_OLLAMA_PORT}/api/tags"; then
    echo "fake ollama did not come up on port $GC_OLLAMA_PORT:"
    tail -3 "$GC_HOME/ollama.log"
    return 1
  fi
  echo "fake ollama on 127.0.0.1:$GC_OLLAMA_PORT, log: $GC_HOME/ollama.log"
}

gc_stop_fake_ollama() {
  local pidfile="$GC_HOME/.fake-ollama.pid"
  [ -f "$pidfile" ] || return 0
  kill "$(cat "$pidfile")" 2> /dev/null
  wait "$(cat "$pidfile")" 2> /dev/null
  rm -f "$pidfile"
}

# gc_provider_ollama -- a configured provider, as an earlier setup would leave
# it. Discovery skips a configured provider, so this is also what makes
# `healthy_provider_types` empty in scenario 3.
gc_provider_ollama() {
  cat > "$GC_HOME/providers/ollama.yaml" <<YAML
provider_id: ollama
type: ollama
config:
  base_url: http://127.0.0.1:${GC_OLLAMA_PORT}
  verify_ssl: false
YAML
}

# gc_recommended <launcher>  -- reads the file body on stdin
gc_recommended() {
  cat > "$GC_HOME/recommended_configs/$1.yaml"
}

# gc_model_variant <model_id>  -- prints the variant a model config carries
gc_model_variant() {
  grep '^variant:' "$GC_HOME/models/$1.yaml"
}

# -- scenarios --------------------------------------------------------------

# Every scenario writes its own recommended config, naming models small enough
# to resolve on any machine these tests are likely to run on. The built-in
# configs aim at granite-4.2-30b at 65536 context, which does not resolve on a
# laptop, and a capability that does not resolve looks the same as one that was
# skipped on purpose.

# 1. Wizard binds each capability to the model its config lists (45a1abb).
#    sub-agent-code gets granite-4.2-8b, sub-agent-explore granite-4.2-3b.
gc_scenario_wizard_models() {
  gc_reset
  gc_launcher_stub claude
  gc_start_fake_ollama
  gc_recommended claude <<'YAML'
launcher: "claude"

capabilities:
  - capability: sub-agent-code
    models:
      model_id:
        min_context_length: 8192
        models:
          - model: granite-4.2-8b
            variant_formats:
              - Ollama
            variant_precisions:
              - Q4_K_M

  - capability: sub-agent-explore
    models:
      model_id:
        min_context_length: 8192
        models:
          - model: granite-4.2-3b
            variant_formats:
              - Ollama
            variant_precisions:
              - Q4_K_M
YAML
  echo "scenario: wizard, claude detected, run 'gc setup'"
}

# 2. setup --auto skips a capability the launcher cannot bind (b29e8b8).
#    bob binds only BindingType::Mcp, so agent-model is the one to skip.
gc_scenario_auto_bob() {
  gc_reset
  gc_launcher_stub bob
  gc_start_fake_ollama
  gc_recommended bob <<'YAML'
launcher: "bob"

capabilities:
  - capability: agent-model
    models:
      model_id:
        min_context_length: 8192
        models:
          - model: granite-4.2-3b
            variant_formats:
              - Ollama
            variant_precisions:
              - Q4_K_M

  - capability: vision-mcp
    models:
      model_id:
        min_context_length: 4096
        models:
          - model: granite-vision-4.1-4b
YAML
  echo "scenario: setup --auto with bob as the only launcher"
}

# 2b. The mirror case: pi binds only AgentModel, so vision-mcp is skipped.
gc_scenario_auto_pi() {
  gc_scenario_auto_bob > /dev/null
  rm -f "$GC_HOME/bin/bob" "$GC_HOME/recommended_configs/bob.yaml"
  gc_launcher_stub pi > /dev/null
  sed 's/^launcher: "bob"/launcher: "pi"/' > "$GC_HOME/recommended_configs/pi.yaml" <<'YAML'
launcher: "bob"

capabilities:
  - capability: agent-model
    models:
      model_id:
        min_context_length: 8192
        models:
          - model: granite-4.2-3b
            variant_formats:
              - Ollama
            variant_precisions:
              - Q4_K_M

  - capability: vision-mcp
    models:
      model_id:
        min_context_length: 4096
        models:
          - model: granite-vision-4.1-4b
YAML
  echo "scenario: setup --auto with pi as the only launcher"
}

# 3. A second setup --auto run keeps what the first one configured (0a3d0cf).
#    Run gc setup --auto after this, then gc_scenario_auto_second_run.
gc_scenario_auto_first_run() {
  gc_reset
  gc_launcher_stub claude
  gc_start_fake_ollama
  gc_recommended claude <<'YAML'
launcher: "claude"

capabilities:
  - capability: agent-model
    models:
      model_id:
        min_context_length: 8192
        models:
          - model: granite-4.2-3b
            variant_formats:
              - Ollama
            variant_precisions:
              - Q4_K_M
YAML
  echo "scenario: first --auto run, then edit a model and add a launcher"
}

# Adds a launcher that was not there during the first run, with a config of
# its own. The provider the first run configured stays, so discovery reports
# no healthy provider and resolution has to use the configured one.
gc_scenario_auto_second_run() {
  gc_launcher_stub goose > /dev/null
  gc_recommended goose <<'YAML'
launcher: "goose"

capabilities:
  - capability: agent-model
    models:
      model_id:
        min_context_length: 8192
        models:
          - model: granite-4.2-3b
            variant_formats:
              - Ollama
            variant_precisions:
              - Q4_K_M
YAML
  echo "scenario: goose added, run 'gc setup --auto' again"
}

# 4. A user recommended config that does not load (1447ef4). The field is
#    variant_precision, one letter short of variant_precisions, which serde
#    used to accept and ignore.
gc_scenario_bad_recommended() {
  gc_reset
  gc_launcher_stub claude
  gc_start_fake_ollama
  gc_recommended claude <<'YAML'
launcher: "claude"

capabilities:
  - capability: agent-model
    models:
      model_id:
        min_context_length: 8192
        models:
          - model: granite-4.2-3b
            variant_formats:
              - Ollama
            variant_precision:
              - Q4_K_M
YAML
  echo "scenario: recommended_configs/claude.yaml has a typo'd field"
}

# The same file with the field spelled correctly, so the only difference
# between this scenario and the one above is that spelling.
gc_scenario_good_recommended() {
  gc_scenario_bad_recommended > /dev/null
  gc_recommended claude <<'YAML'
launcher: "claude"

capabilities:
  - capability: agent-model
    models:
      model_id:
        min_context_length: 8192
        models:
          - model: granite-4.2-3b
            variant_formats:
              - Ollama
            variant_precisions:
              - Q4_K_M
YAML
  echo "scenario: the same file, field spelled variant_precisions"
}

# 5. A capability with no model is skipped once (59bb91a). Run 'gc setup' and
#    deselect every model in the model step.
gc_scenario_no_models() {
  gc_reset
  gc_launcher_stub claude
  gc_start_fake_ollama
  gc_recommended claude <<'YAML'
launcher: "claude"

capabilities:
  - capability: agent-model
    models:
      model_id:
        min_context_length: 8192
        models:
          - model: granite-4.2-3b
            variant_formats:
              - Ollama
            variant_precisions:
              - Q4_K_M
YAML
  echo "scenario: wizard, deselect every model in the model step"
}
