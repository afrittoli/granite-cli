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
# Parallel-array helpers (bash 3.x associative-array replacement)
#
# Each "map" is represented as two indexed arrays — one for keys, one for
# values — plus a lookup function that iterates them.
#
# Usage:
#   map_set  <keys_var> <vals_var> <key> <value>
#   map_get  <keys_var> <vals_var> <key>   # prints value or empty string
# ---------------------------------------------------------------------------
map_set() {
    local _keys_var="$1" _vals_var="$2" _key="$3" _val="$4"
    local _i
    # Overwrite if key already exists
    eval "local _len=\${#${_keys_var}[@]}"
    for (( _i=0; _i<_len; _i++ )); do
        eval "local _k=\"\${${_keys_var}[${_i}]}\""
        if [[ "$_k" == "$_key" ]]; then
            eval "${_vals_var}[${_i}]=\"\${_val}\""
            return
        fi
    done
    # New entry
    eval "${_keys_var}+=(\"${_key}\")"
    eval "${_vals_var}+=(\"${_val}\")"
}

map_get() {
    local _keys_var="$1" _vals_var="$2" _key="$3"
    local _i
    eval "local _len=\${#${_keys_var}[@]}"
    for (( _i=0; _i<_len; _i++ )); do
        eval "local _k=\"\${${_keys_var}[${_i}]}\""
        if [[ "$_k" == "$_key" ]]; then
            eval "printf '%s' \"\${${_vals_var}[${_i}]}\""
            return
        fi
    done
    # Key not found — return empty
    printf '%s' ''
}

# ---------------------------------------------------------------------------
# Static lookup functions (replaces declare -A with fixed keys)
# ---------------------------------------------------------------------------

# Returns the source file path for a capability id, or empty string.
cap_src_file() {
    case "$1" in
        agent-model)       printf '%s' "${CAPABILITIES_DIR}/agent_model.rs" ;;
        vision-mcp)        printf '%s' "${CAPABILITIES_DIR}/vision_mcp/mod.rs" ;;
        sub-agent)         printf '%s' "${CAPABILITIES_DIR}/sub_agent.rs" ;;
        sub-agent-code)    printf '%s' "${CAPABILITIES_DIR}/sub_agent_code.rs" ;;
        sub-agent-explore) printf '%s' "${CAPABILITIES_DIR}/sub_agent_explore.rs" ;;
        sub-agent-plan)    printf '%s' "${CAPABILITIES_DIR}/sub_agent_plan.rs" ;;
        *)                 printf '%s' '' ;;
    esac
}

# Returns the display name for a capability id, or the id itself as fallback.
cap_display_name_map() {
    case "$1" in
        agent-model)       printf '%s' "Agent Model Binding" ;;
        vision-mcp)        printf '%s' "Vision MCP Server" ;;
        sub-agent)         printf '%s' "Sub-Agent" ;;
        sub-agent-code)    printf '%s' "Code Sub-Agent" ;;
        sub-agent-explore) printf '%s' "Explore Sub-Agent" ;;
        sub-agent-plan)    printf '%s' "Plan Sub-Agent" ;;
        *)                 printf '%s' "$1" ;;
    esac
}

# Returns a description override for a capability id, or empty string.
# Description overrides for capabilities whose source has macro definitions
# that confuse the awk extractor (e.g. declare_sub_agent_full! in sub_agent.rs
# contains template lines like `description: $description_cap.to_string()`).
cap_description_override() {
    case "$1" in
        sub-agent) printf '%s' "Defines a named sub-agent (prompt, tool allow-list, and model) that a launched coding agent can delegate to." ;;
        *)         printf '%s' '' ;;
    esac
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
launcher_type_keys=()   # parallel arrays: launcher_type_keys[i] / launcher_type_vals[i]
launcher_type_vals=()

for id in "${launcher_ids[@]}"; do
    if [[ -f "${LAUNCHERS_DIR}/${id}/mod.rs" ]]; then
        src="${LAUNCHERS_DIR}/${id}/mod.rs"
    elif [[ -f "${LAUNCHERS_DIR}/${id}.rs" ]]; then
        src="${LAUNCHERS_DIR}/${id}.rs"
    else
        echo "WARNING: source file not found for launcher '${id}'" >&2
        map_set launcher_type_keys launcher_type_vals "$id" ""
        continue
    fi

    # Extract all BindingType::X tokens from the supported_capabilities block.
    # Use grep -A to grab the block, then extract tokens portably.
    types=$(grep -A5 'supported_capabilities:' "$src" \
            | grep -o 'BindingType::[A-Za-z]*' \
            | sed 's/BindingType:://' \
            | tr '\n' ' ' || true)

    map_set launcher_type_keys launcher_type_vals "$id" "${types% }"
done

# ---------------------------------------------------------------------------
# 4. Capability metadata: binding type, display name, description
# ---------------------------------------------------------------------------
cap_btype_keys=()   # parallel arrays for cap_binding_type
cap_btype_vals=()

cap_dname_keys=()   # parallel arrays for cap_display_name
cap_dname_vals=()

cap_desc_keys=()    # parallel arrays for cap_description
cap_desc_vals=()

for id in "${capability_ids[@]}"; do
    src=$(cap_src_file "$id")
    if [[ -z "$src" || ! -f "$src" ]]; then
        echo "WARNING: source file not found for capability '${id}'" >&2
        map_set cap_btype_keys cap_btype_vals "$id" ""
        map_set cap_desc_keys  cap_desc_vals  "$id" ""
        map_set cap_dname_keys cap_dname_vals "$id" "$id"
        continue
    fi

    # First BindingType token in the file
    btype=$(grep -o 'BindingType::[A-Za-z]*' "$src" | head -1 | sed 's/BindingType:://')
    map_set cap_btype_keys cap_btype_vals "$id" "${btype:-}"
    map_set cap_dname_keys cap_dname_vals "$id" "$(cap_display_name_map "$id")"

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
    override=$(cap_description_override "$id")
    map_set cap_desc_keys cap_desc_vals "$id" "${override:-${desc:-}}"
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
        btype=$(map_get cap_btype_keys cap_btype_vals "$cap_id")
        display=$(map_get cap_dname_keys cap_dname_vals "$cap_id")
        [[ -z "$display" ]] && display="$cap_id"
        row="| \`${cap_id}\` | ${display}"
        for l_id in "${launcher_ids[@]}"; do
            ltypes=$(map_get launcher_type_keys launcher_type_vals "$l_id")
            if echo " ${ltypes} " | grep -qw "$btype"; then
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
        desc=$(map_get cap_desc_keys cap_desc_vals "$cap_id")
        echo "| \`${cap_id}\` | ${desc} |"
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
