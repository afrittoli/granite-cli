---
name: update-capability-table
description: Use this skill any time a launcher or capability is added, removed, or has its supported_capabilities changed, to keep the Capability Support table in README.md in sync with the source code.
---

# Update Capability Table Skill

This skill regenerates the **Capability Support** matrix in `README.md` by
parsing the launcher and capability source files. It is the authoritative way
to keep the table in sync — never edit the table by hand.

## When to Use This Skill

- A new launcher is registered in `src/launchers/mod.rs`
- A new capability is registered in `src/capabilities/mod.rs`
- An existing launcher's `supported_capabilities` set changes
- A capability's `BindingType` changes
- A capability's metadata `name` or `description` changes
- The CI step `Check capability table is up to date` fails on a PR

## Prerequisites

- `bash` (macOS system bash is sufficient — no Homebrew needed)
- Standard Unix tools: `grep`, `sed`, `awk` — all present on macOS and Linux

No internet connection required. Everything is read from local source files.

## Quick Start

```bash
# Regenerate the table in README.md
./scripts/update-capability-table.sh

# Verify it is up to date (CI mode — exits 1 if stale)
./scripts/update-capability-table.sh --check

# Review what changed
git diff README.md
```

## What the Script Does

1. Reads launcher ids (in registration order) from `src/launchers/mod.rs`
2. Reads capability ids (in registration order) from `src/capabilities/mod.rs`
3. For each launcher, extracts its `supported_capabilities: HashSet::from([...])`
   to determine which `BindingType` values it supports
4. For each capability, extracts its `BindingType` and metadata description
5. Generates a markdown matrix and description table
6. Replaces the block in `README.md` between:
   `<!-- capability-table-start -->` and `<!-- capability-table-end -->`

## Adding a New Launcher

1. Create `src/launchers/<id>.rs` and implement `fn metadata() -> LauncherMetadata`
   with `supported_capabilities: HashSet::from([BindingType::X, ...])`
2. Register it in `src/launchers/mod.rs`:
   `factory.register::<YourLauncher>("your-id");`
3. Run `./scripts/update-capability-table.sh`
4. Commit `README.md` alongside your launcher source

## Adding a New Capability

1. Create `src/capabilities/<id>.rs` and implement `fn metadata() -> CapabilityMetadata`
   with the capability's `name`, `description`, and `supported_binding_types`
2. Register it in `src/capabilities/mod.rs`:
   `factory.register::<YourCapability>("your-id");`
3. Run `./scripts/update-capability-table.sh`
4. Commit `README.md` alongside your capability source

## CI Enforcement

The `rust` job in `.github/workflows/ci.yml` runs:

```bash
bash scripts/update-capability-table.sh --check
```

This step fails if `README.md` is stale, blocking the PR until the table is
regenerated and committed.

## Related Files

- [`scripts/update-capability-table.sh`](../../../scripts/update-capability-table.sh) — the generator script
- [`src/launchers/mod.rs`](../../../src/launchers/mod.rs) — launcher registration
- [`src/capabilities/mod.rs`](../../../src/capabilities/mod.rs) — capability registration
- [`README.md`](../../../README.md) — contains the generated table
- [`.github/workflows/ci.yml`](../../../.github/workflows/ci.yml) — CI enforcement step
