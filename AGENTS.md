# BTS agent instructions

## Required checks

Before opening or updating a pull request, run:

cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

## Development rules

- Implement only the assigned GitHub issue.
- Do not broaden the scope without an explicit issue update.
- Do not change public protocol contracts unless the issue requires it.
- Preserve separation between bts-core, bts-protocol, bts-display,
  bts-telephony, bts-addons, bts-client and bts-cli.
- Add automated tests for all new behaviour.
- Do not weaken or delete tests merely to make CI pass.
- Do not merge pull requests.
- Document architectural decisions and unresolved assumptions in the PR.
- Use British English in user-facing text.

## Hardware-dependent work

Do not claim that physical display, Raspberry Pi, Asterisk, audio,
telephone or DTMF behaviour has been verified unless it was tested on
real hardware. Mark such acceptance criteria as requiring manual testing.
