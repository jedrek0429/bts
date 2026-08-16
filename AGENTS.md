# BTS agent instructions

## Required checks

Before opening or updating a pull request, run:

cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

## Commit messages

Use Conventional Commits for every authored commit subject:

`<type>[optional scope][!]: <description>`

Allowed types: `build`, `chore`, `ci`, `docs`, `feat`, `fix`, `perf`, `refactor`, `revert`, `style`, `test`.

Examples:

- `feat(terminal): add group targeting`
- `fix(display): restore packaged Cabin font discovery`
- `ci(release): enforce immutable release tags`

When a commit fully resolves a GitHub issue, include a GitHub closing keyword and issue reference in the commit body, for example `Closes #123`, `Fixes #123`, or `Resolves #123`. Use a closing keyword only when the commit actually completes the issue; otherwise use a plain issue reference such as `Refs #123`.

This requirement ensures that completed issues are closed automatically when the resolving commit reaches the repository's default branch, including commits first merged through a `release/**` branch.

CI validates all non-merge commits introduced by a pull request or direct push to `main` or `release/**`.

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
