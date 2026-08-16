# Versioning and releases

## Versions

| Category | Source | Changes when |
| --- | --- | --- |
| BTS product and all workspace crates | `[workspace.package].version` | Every BTS release |
| Core API, Addon API and built-in addons | `compatibility.json` | Their corresponding contract or implementation breaks compatibility |
| Release manifest and component bundle | `compatibility.json` | Their structure or layout breaks compatibility |
| Installer state and JSON output | `compatibility.json` | Persisted or machine-readable data breaks compatibility |

`Cargo.toml` and `compatibility.json` are the version sources. Every BTS crate inherits the product version. `bts-compat` generates Rust constants and versioned Core paths from the compatibility file; release tooling reads the same file. Documentation describes the lifecycle without defining a competing version.

Compatibility versions are independent of product releases. Additive contract changes retain the current API/schema version. Breaking network contracts add a new route/module version alongside the old one during migration. Persisted state changes require a migration before its schema number changes. Built-in addon versions use SemVer.

## Operator workflows

The Actions interface exposes release operations by intent:

- **Publish release candidate** runs manually on `release/X.Y.x`. It derives the next `rc.N`, updates `Cargo.toml` and `Cargo.lock`, runs the canonical CI workflow, creates an immutable tag, builds release artifacts and publishes a GitHub prerelease.
- **Create stable release PR** runs manually on `release/X.Y.x`. The branch HEAD must be exactly a published RC. It changes the workspace to the stable version, validates that commit, and opens `release/X.Y.x -> main`. It publishes nothing.
- **Publish stable release** runs automatically when a release-line PR is merged into `main`. It validates the merge commit, creates the immutable stable tag, builds release artifacts, publishes the stable GitHub Release, then advances the existing maintenance line to the next patch development version.
- **Create next release line** runs manually on `main` when development of the next minor version should begin. It derives the next minor version, creates `release/X.Y.x`, updates `Cargo.toml` and `Cargo.lock`, and validates the new development state before pushing it.

`Build release artifacts` is reusable implementation machinery and has no manual trigger. `CI` is read-only and keeps Cargo's `--locked` checks so inconsistent version metadata fails immediately.

## Candidate flow

A release line starts in development, for example:

```text
release/0.4.x
0.4.0-dev.0
```

Run **Publish release candidate** on that branch. The workflow derives `0.4.0-rc.1` when no candidate tags exist. After fixes, running it again derives `0.4.0-rc.2`, then `rc.3`, and so on. Candidate numbering is sequential and tags never move.

If validation fails after the candidate commit is pushed, the version remains an unpublished `rc.N`. Fix the branch and rerun the same workflow; it retries that candidate number until it is successfully tagged.

## Stable flow

When a published candidate is accepted, run **Create stable release PR** on its release branch. Stable preparation requires the branch HEAD to equal the published candidate tag exactly, which prevents untested post-candidate changes from entering the stable release.

For example:

```text
v0.4.0-rc.2 at release/0.4.x HEAD
        -> Create stable release PR
0.4.0 commit on release/0.4.x
        -> validated PR to main
        -> maintainer merges PR
v0.4.0 on the resulting main commit
```

Merging that PR is the explicit stable-publication action.

## After a stable release

After the stable GitHub Release is published, automation advances only the release line that was just published:

```text
release/0.4.x -> 0.4.1-dev.0
```

This keeps the current minor line ready for patch maintenance without creating speculative future branches.

When feature development for the next minor version is actually planned, run **Create next release line** from `main`:

```text
main at 0.4.0
        -> Create next release line
release/0.5.x -> 0.5.0-dev.0
```

The workflow refuses to overwrite an existing next release line. Creating the branch is therefore an explicit project decision rather than an automatic consequence of publishing the previous minor release.

## Cargo lockfile policy

Release workflows change the workspace version with `scripts/release-version.py` and run `cargo update --workspace` so workspace package entries in `Cargo.lock` follow the new product version. CI and release builds then use `--locked`.

Release preparation is allowed to modify only `Cargo.toml` and `Cargo.lock`; unexpected file changes abort the workflow.

## Installation

Install stable by omitting `--channel`; install a candidate explicitly:

```sh
sudo bts-install install full
sudo bts-install install full --channel v0.4.0-rc.1
```

Branches and Actions artifacts are development inputs rather than installation channels. `stable` excludes drafts, prereleases and legacy releases without an Installer v2 manifest.

For an unpublished development build, run `scripts/build-release all` and install its directory with `--release-dir`; see the [development guide](development.md).
