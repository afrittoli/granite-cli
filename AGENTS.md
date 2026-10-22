## Project Practices

- Avoid adding new dependencies if an existing dependency or standard library could serve the same function
- Use the minimal visibility scoping for every function (only `pub` if necessary, crate-pub unless full pub necessary)
- When laying out a source file, organize it as follows:
    1. `use` directives
    2. Public functions/structs/enums/etc (mark with `/*-- public --*/` to begin section)
    3. Private functions/structs/enums/etc (mark with `/*-- private --*/` to begin seciton)
    4. Tests (mark with `/*-- tests --*/` to begin seciton)
- When formatting code, use `./scripts/fmt.sh` which runs both `clippy` and `fmt` with the right versions of the rust toolchain via `rustup`
- When writing unit tests, nothing should _ever_ depend on the local environment. This includes hardware detection, existence of pre-installed binaries, internet connectivity, and anything else that could differ between development environments

## End-to-end and manual testing

- The end-to-end test kit lives in `tests/e2e/`; `tests/e2e/README.md` describes it. Use and extend it (`lib.sh`, `fixtures/`, `run.sh`, `manual-test-template.md`) for new end-to-end tests, fixtures and manual test guides
- Run the CI set with `tests/e2e/run.sh`, and the tests that need a local environment with `tests/e2e/run.sh --local`
- A test in `tests/e2e/hermetic/` needs only the binary and the files it writes: no network, model server, hardware detection, launcher command on `PATH` or terminal. CI runs these. A test that needs any of those goes in `tests/e2e/local/` and skips itself when the requirement is missing
- When a feature changes behaviour that a script can check, add or update a hermetic test in the same branch. Assert on `-o json` output and do not depend on row order
- Write a feature branch's manual test guide from `tests/e2e/manual-test-template.md`, with setup through `tests/e2e/lib.sh`. Record the commit and machine the output was observed on, what was not verified, and findings
- In a manual test guide, put every step a script can run in a test under `tests/e2e/`, list each step's kind and status in the guide's test map, and tag step headings `[Automated]` or `[Manual]`. End every command that shows scripted behaviour with `</dev/null`, so a pasted command does not prompt; give the keys and prompts for steps that need a terminal
- Add a fixture to `tests/e2e/fixtures/` when two or more tests or guides need it. Fixtures must load on `main`; a fixture for a branch-only feature lands with that feature
- The rule for unit tests above still applies: unit tests must not depend on the local environment. End-to-end tests are the place for behaviour that needs the built binary
