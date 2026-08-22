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

The CI workflow is a stable test runner, not a feature-specific test definition.
Put new tests in Rust modules and normal integration-test directories so Cargo
discovers them automatically. Do not add or modify CI jobs merely to run tests
for one feature.

## Test-driven development

Tests are executable specifications and must drive production changes.

For every bug fix, feature or observable behavioural change:

1. Define the observable requirement and acceptance criteria before changing production code.
2. Write the smallest meaningful automated test that expresses the requirement.
3. Run it and confirm that it fails for the expected reason (RED). A test that already passes does not prove missing behaviour.
4. Only then write the minimum production code needed to satisfy the test.
5. Run the focused test, the relevant crate suite and the complete workspace suite (GREEN).
6. Refactor only while the complete suite remains green.

Every automatable bug must have a regression test that demonstrates the bug
before the fix. Test observable behaviour, public contracts, state transitions,
outputs, errors and side effects rather than source strings or incidental private
implementation details. Existing tests are specifications: never weaken, skip,
delete or rewrite a legitimate test merely to make an implementation pass. A
test may change only when its mistake or explicitly obsolete requirement has
been established and the reason is documented.

Use deterministic unit tests for module behaviour and integration tests for
public or cross-component boundaries. Cover relevant failures, boundaries,
recovery and idempotency. Hardware-only criteria remain a clearly identified
manual release gate; automate every deterministic layer beneath them.

## Change and pull-request scope

- One fix or one feature gets exactly one dedicated branch and one pull request.
- Base pull requests on the designated working branch whenever possible. For
  release work this is normally `release/X.Y.x`. Prefer that shared integration branch over a stack whenever the change
  can stand on its own.
- A branch and its pull request must have one specific, reviewable purpose. Keep
  the diff small enough that a reviewer can identify every design choice,
  assumption and behavioural consequence.
- Never silently broaden assigned work. If another fix, feature or refactor is
  discovered, stop and either obtain an explicit scope change or put it on a
  separate dedicated branch and pull request.
- Do not mix opportunistic cleanup, unrelated refactors, policy changes or
  release-process changes into a feature branch. Documentation and tests that
  directly specify the feature are part of its scope.
- Do not create parallel replacement branches for the same work. Names such as
  `-v2`, `-final`, `-followup`, `-review` or similar variants are prohibited.
  Continue on the original dedicated branch unless its pull request has been
  merged or explicitly abandoned.
- Stack a pull request only when its change directly depends on code in another
  pull request that is still waiting for review. Do not stack independent fixes
  or features merely because they are being developed at the same time; base
  each of those independently on the designated working branch.
- When a direct dependency requires a stack, keep each change in a focused pull
  request. Each dependent PR targets the branch immediately below it; the
  bottom PR targets the required long-lived release branch.
- A stack has strict linear history. Finish each lower branch before building
  its child, create only one child at each level, and update descendants in
  order if an ancestor changes. Prefer forward, linear development over
  repeatedly revisiting an ancestor and rebasing several speculative branches.
- Record a stack's order and dependency in every non-bottom PR description.
- Branch names must describe their single fix or feature, for example
  `fix/display-blank-persistence` or `feat/terminal-groups`.

## Development rules

- Implement only the assigned GitHub issue.
- Do not broaden the scope without explicit human approval and an issue or PR update.
- Do not change public protocol contracts unless the issue requires it.
- Preserve separation between bts-core, bts-protocol, bts-display,
  bts-telephony, bts-addons, bts-client and bts-cli.
- Add automated tests for all new behaviour under the TDD rules above.
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
