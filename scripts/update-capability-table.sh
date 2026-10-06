#!/usr/bin/env bash
# update-capability-table.sh
#
# Parses launcher and capability source files to generate the
# Capability Support matrix in README.md.
#
# Usage:
#   ./scripts/update-capability-table.sh           # update README.md in place
#   ./scripts/update-capability-table.sh --check   # exit 1 if README.md is stale
#
# The script rewrites the block between:
#   <!-- capability-table-start -->
#   <!-- capability-table-end -->

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

CHECK_MODE=false
if [[ "${1:-}" == "--check" ]]; then
    CHECK_MODE=true
fi

README="README.md"
LAUNCHERS_MOD="src/launchers/mod.rs"
CAPABILITIES_MOD="src/capabilities/mod.rs"
LAUNCHERS_DIR="src/launchers"
CAPABILITIES_DIR="src/capabilities"

# ---------------------------------------------------------------------------
# Helper: extract quoted id from a factory.register line
#   factory.register::<FooType>("some-id");  =>  some-id
# ---------------------------------------------------------------------------
extract_id() {
    echo "$1" | sed 's/.*register::<[^>]*>("\([^"]*\)").*/\1/'
}

# ---------------------------------------------------------------------------
# 1. Extract launcher ids (in registration order)
# ---------------------------------------------------------------------------
launcher_ids=()
while IFS= read -r line; do
    id=$(extract_id "$line")
    [[ -n "$id" && "$id" != "$line" ]] && launcher_ids+=("$id")
done < <(grep 'factory\.register' "$LAUNCHERS_MOD")

# ---------------------------------------------------------------------------
# 2. Extract capability ids (in registration order)
# ---------------------------------------------------------------------------
capability_ids=()
while IFS= read -r line; do
    id=$(extract_id "$line")
    [[ -n "$id" && "$id" != "$line" ]] && capability_ids+=("$id")
done < <(grep 'factory\.register' "$CAPABILITIES_MOD")

# ---------------------------------------------------------------------------
# 3. For each launcher, extract its supported BindingTypes
#    Source: supported_capabilities: HashSet::from([BindingType::X, ...])
#    Bob launcher lives in a subdirectory — handle that.
# ---------------------------------------------------------------------------
declare -A launcher_types  # launcher_types["bob"]="Mcp SubAgent"

for id in "${launcher_ids[@]}"; do
    if [[ -f "${LAUNCHERS_DIR}/${id}/mod.rs" ]]; then
        src="${LAUNCHERS_DIR}/${id}/mod.rs"
    elif [[ -f "${LAUNCHERS_DIR}/${id}.rs" ]]; then
        src="${LAUNCHERS_DIR}/${id}.rs"
    else
        echo "WARNING: source file not found for launcher '${id}'" >&2
        launcher_types["$id"]=""
        continue
    fi

    # Extract all BindingType::X tokens from the supported_capabilities block.
    # Use grep -A to grab the block, then extract tokens portably.
    types=$(grep -A5 'supported_capabilities:' "$src" \
            | grep -o 'BindingType::[A-Za-z]*' \
            | sed 's/BindingType:://' \
            | tr '\n' ' ' || true)

    launcher_types["$id"]="${types% }"
done

# ---------------------------------------------------------------------------
# 4. Capability metadata: binding type, display name, description
# ---------------------------------------------------------------------------
declare -A cap_binding_type
declare -A cap_display_name
declare -A cap_description

# Source file per capability id
declare -A cap_src_file
cap_src_file["agent-model"]="${CAPABILITIES_DIR}/agent_model.rs"
cap_src_file["vision-mcp"]="${CAPABILITIES_DIR}/vision_mcp/mod.rs"
cap_src_file["sub-agent"]="${CAPABILITIES_DIR}/sub_agent.rs"
cap_src_file["sub-agent-code"]="${CAPABILITIES_DIR}/sub_agent_code.rs"
cap_src_file["sub-agent-explore"]="${CAPABILITIES_DIR}/sub_agent_explore.rs"
cap_src_file["sub-agent-plan"]="${CAPABILITIES_DIR}/sub_agent_plan.rs"

# Display names from metadata() name: field
declare -A cap_display_name_map
cap_display_name_map["agent-model"]="Agent Model Binding"
cap_display_name_map["vision-mcp"]="Vision MCP Server"
cap_display_name_map["sub-agent"]="Sub-Agent"
cap_display_name_map["sub-agent-code"]="Code Sub-Agent"
cap_display_name_map["sub-agent-explore"]="Explore Sub-Agent"
cap_display_name_map["sub-agent-plan"]="Plan Sub-Agent"

# Description overrides for capabilities whose source has macro definitions
# that confuse the awk extractor (e.g. declare_sub_agent_full! in sub_agent.rs
# contains template lines like `description: $description_cap.to_string()`).
declare -A cap_description_override
cap_description_override["sub-agent"]="Defines a named sub-agent (prompt, tool allow-list, and model) that a launched coding agent can delegate to."

for id in "${capability_ids[@]}"; do
    src="${cap_src_file[$id]:-}"
    if [[ -z "$src" || ! -f "$src" ]]; then
        echo "WARNING: source file not found for capability '${id}'" >&2
        cap_binding_type["$id"]=""
        cap_description["$id"]=""
        cap_display_name["$id"]="$id"
        continue
    fi

    # First BindingType token in the file
    btype=$(grep -o 'BindingType::[A-Za-z]*' "$src" | head -1 | sed 's/BindingType:://')
    cap_binding_type["$id"]="${btype:-}"
    cap_display_name["$id"]="${cap_display_name_map[$id]:-$id}"

    # Description extraction:
    #   - For declare_sub_agent_*! macro calls (sub_agent*.rs):
    #     the capability description is the 2nd quoted "..."; argument.
    #     Structure: TypeName / ConfigName / "Name"; / "Description"; / ...
    #     So it's the second line matching /".*;$/ inside the macro block.
    #   - For fn metadata() definitions (agent_model.rs, vision_mcp):
    #     read from the description: field.
    desc=$(awk '
        /declare_sub_agent/ { in_macro=1; quoted_count=0; next }
        in_macro && /^[[:space:]]*"[^"]*";/ {
            quoted_count++
            if (quoted_count == 2) {
                s = $0
                sub(/[^"]*"/, "", s)
                sub(/".*$/, "", s)
                if (length(s) > 10) { print s; exit }
            }
        }
        in_macro && /^\)/ { in_macro=0 }
        /fn metadata/ { in_meta=1 }
        in_meta && /description:/ {
            s = $0
            sub(/[^"]*"/, "", s)
            sub(/".*$/, "", s)
            if (length(s) > 10) { print s; exit }
        }
        in_meta && /^\}/ { in_meta=0 }
    ' "$src" || true)

    # Apply override if set (takes precedence over extracted value)
    override="${cap_description_override[$id]:-}"
    cap_description["$id"]="${override:-${desc:-}}"
done

# ---------------------------------------------------------------------------
# 5. Generate the replacement block
# ---------------------------------------------------------------------------
generate_block() {
    echo "<!-- capability-table-start -->"
    echo ""

    # Header
    header="| Capability | Type"
    separator="|---|---"
    for id in "${launcher_ids[@]}"; do
        header+=" | \`${id}\`"
        separator+="|:---:"
    done
    echo "${header} |"
    echo "${separator}|"

    # Data rows
    for cap_id in "${capability_ids[@]}"; do
        btype="${cap_binding_type[$cap_id]:-}"
        display="${cap_display_name[$cap_id]:-$cap_id}"
        row="| \`${cap_id}\` | ${display}"
        for l_id in "${launcher_ids[@]}"; do
            if echo " ${launcher_types[$l_id]:-} " | grep -qw "$btype"; then
                row+=" | ✅"
            else
                row+=" | ❌"
            fi
        done
        echo "${row} |"
    done

    echo ""
    echo "**Capability descriptions:**"
    echo ""
    echo "| Capability | Description |"
    echo "|---|---|"
    for cap_id in "${capability_ids[@]}"; do
        echo "| \`${cap_id}\` | ${cap_description[$cap_id]:-} |"
    done

    echo ""
    echo "**Binding types** used to determine compatibility:"
    echo "- **AgentModel** — passes a model's connection details directly to the launcher"
    echo "- **Mcp** — exposes the capability as an MCP server the launcher can call"
    echo "- **SubAgent** — exposes the capability as a named sub-agent the launcher can delegate to"
    echo ""
    echo "<!-- capability-table-end -->"
}

BLOCK_FILE=$(mktemp)
generate_block > "$BLOCK_FILE"

# ---------------------------------------------------------------------------
# 6. Replace sentinel block in README.md using a temp file for the block
# ---------------------------------------------------------------------------
UPDATED_FILE=$(mktemp)
awk -v block_file="$BLOCK_FILE" '
    /<!-- capability-table-start -->/ {
        in_block = 1
        while ((getline line < block_file) > 0) print line
        next
    }
    /<!-- capability-table-end -->/ { in_block = 0; next }
    !in_block { print }
' "$README" > "$UPDATED_FILE"

if $CHECK_MODE; then
    if diff -q "$README" "$UPDATED_FILE" > /dev/null 2>&1; then
        echo "✅ Capability table in ${README} is up to date."
        rm -f "$BLOCK_FILE" "$UPDATED_FILE"
        exit 0
    else
        echo "❌ Capability table in ${README} is out of date." >&2
        echo "   Run ./scripts/update-capability-table.sh to regenerate it." >&2
        diff "$README" "$UPDATED_FILE" >&2 || true
        rm -f "$BLOCK_FILE" "$UPDATED_FILE"
        exit 1
    fi
else
    cp "$UPDATED_FILE" "$README"
    rm -f "$BLOCK_FILE" "$UPDATED_FILE"
    echo "✅ ${README} capability table updated."
fi
