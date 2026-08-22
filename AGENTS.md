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

Use the same Conventional Commits format for pull request titles. A pull request title should describe the aggregate change being proposed and use an appropriate type, optional scope and breaking-change marker where applicable.

When a commit fully resolves a GitHub issue, include a GitHub closing keyword and issue reference in the commit body, for example `Closes #123`, `Fixes #123`, or `Resolves #123`. Use a closing keyword only when the commit actually completes the issue; otherwise use a plain issue reference such as `Refs #123`.

This requirement ensures that completed issues are closed automatically when the resolving commit reaches the repository's default branch, including commits first merged through a `release/**` branch.

CI validates all non-merge commits introduced by a pull request or direct push to `main` or `release/**`.

## Development rules

- Use test-driven development for behavioural changes. Define the intended behaviour in the issue or specification first, then write or update automated tests that express that behaviour before implementing production code.
- Develop the implementation around those tests until they pass.
- Treat tests as part of the specification. Do not alter, weaken, delete, bypass or replace tests merely to make CI green.
- Change a test only when investigation establishes that the test itself contains a genuine mistake, encodes superseded requirements, or incorrectly models the real system. Document that reason in the commit or pull request.
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

## Backwards compatibility

BTS is developing rapidly and breaking changes are expected. Backwards
compatibility is not a default requirement unless an issue or an existing public
protocol contract explicitly requires it.

- Do not retain deprecated CLI options, aliases, configuration formats or code
  paths solely for backwards compatibility.
- Prefer removing obsolete behaviour over maintaining parallel legacy and new
  implementations.
- Add a narrow, one-way migration only when it is inexpensive and needed to keep
  already-deployed installations upgradeable. Do not expose migrations as
  permanent public interfaces.
- Document intentional breaking changes and any retained migration in the pull
  request.

## Hardware-dependent work

Do not claim that physical display, Raspberry Pi, Asterisk, audio,
telephone or DTMF behaviour has been verified unless it was tested on
real hardware. Mark such acceptance criteria as requiring manual testing.
