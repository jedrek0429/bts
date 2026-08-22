# BTS agent instructions

## Required checks

Before opening or updating a pull request, run:

cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

The CI workflow is a stable test runner, not a feature-specific test definition. New tests belong in the Rust crates and normal integration-test directories so the existing workspace test command discovers and runs them automatically. Do not add or modify CI jobs merely to make a feature's tests run when Cargo's normal test discovery can run them.

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

## Test-driven development

BTS development uses rigorous test-driven development. Tests are executable specifications and must drive production changes.

For every bug fix, feature, behavioural change or refactor that changes observable behaviour:

1. Define the required behaviour and acceptance criteria before changing production code.
2. Write the smallest meaningful automated test or tests that express that behaviour.
3. Run the relevant test and verify that it fails for the expected reason. This is the required RED state. A test that already passes does not demonstrate the missing behaviour and is not sufficient evidence for TDD.
4. Only after observing the expected failure, write the minimum production code necessary to satisfy the specification.
5. Run the new test and the complete relevant existing suite until all tests pass. This is the GREEN state.
6. Refactor only while the complete suite remains green. Refactoring must preserve externally observable behaviour unless a separately specified failing test requires a behaviour change.

Additional requirements:

- Every reported bug must receive a regression test that reproduces the bug before the fix is implemented whenever the behaviour can be automated.
- Test observable behaviour, public contracts, state transitions, outputs, errors and side effects. Do not test source-code strings, formatting, private implementation details or the mere presence of a particular implementation unless that artifact is itself a public contract.
- Prefer deterministic unit tests for pure logic and component behaviour. Use integration tests for boundaries such as protocol exchange, filesystem operations, process/service reconciliation, installer transactions and interactions between BTS crates.
- Cover success paths, failure paths, boundary conditions, invalid inputs, recovery, idempotency and regressions where they are relevant to the specification.
- Existing tests are specifications. Never weaken, delete, skip, bypass or rewrite a test merely because new production code fails it.
- Change an existing test only after establishing that the test contains a genuine mistake, models an obsolete requirement that has explicitly changed, or asserts behaviour contrary to the authoritative specification. Document the reason for that test change in the commit or pull request.
- Do not alter expected values, mocks, fixtures, timeouts or assertions simply to make CI green. Fix production code when production code violates the specification.
- Keep test doubles behaviourally faithful to the boundary they represent. A defective fixture or mock may be corrected when its mismatch with the real contract is demonstrated; that correction is not permission to weaken the behavioural assertion.
- A production-code commit should normally be preceded in history by the failing specification/regression test that requires it. Do not implement first and add tests afterwards merely to obtain coverage.
- Do not consider a change complete because its new tests pass in isolation. The full workspace suite must remain green so every established BTS behaviour is continuously revalidated.
- Hardware-dependent acceptance criteria that cannot be represented faithfully in automation remain manual release-gate tests. Automate every deterministic layer beneath them and never claim the hardware criterion itself passed without physical testing.

## Development rules

- Implement only the assigned GitHub issue.
- Do not broaden the scope without an explicit issue update.
- Do not change public protocol contracts unless the issue requires it.
- Preserve separation between bts-core, bts-protocol, bts-display,
  bts-telephony, bts-addons, bts-client and bts-cli.
- New behaviour is incomplete without automated tests satisfying the TDD rules above.
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
