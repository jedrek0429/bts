# Versioning and releases

## Versions

| Category | Source | Changes when |
| --- | --- | --- |
| BTS product and all workspace crates | `[workspace.package].version` | Every BTS release |
| Core API, Addon API and built-in addons | `compatibility.json` | Their corresponding contract or implementation breaks compatibility |
| Release manifest and component bundle | `compatibility.json` | Their structure or layout breaks compatibility |
| Installer state and JSON output | `compatibility.json` | Persisted or machine-readable data breaks compatibility |

`Cargo.toml` and `compatibility.json` are the only version sources. Every BTS crate inherits the product version. `bts-compat` generates Rust constants and versioned Core paths from the compatibility file; release tooling reads the same file. Documentation must not define a competing value.

Compatibility versions are independent of product releases. Additive contract changes retain the current API/schema version. Breaking network contracts add a new route/module version alongside the old one during migration. Persisted state changes require a migration before its schema number changes. Built-in addon versions use SemVer.

## Release flow

`release/X.Y.x` must contain an `X.Y.*` workspace version:

```text
0.4.0-dev.0  development; CI artefacts only
0.4.0-rc.1   automatically tagged v0.4.0-rc.1 and published as a prerelease
0.4.0-rc.2   next immutable candidate
0.4.0        merge to main; the merge commit is tagged v0.4.0 and published stable
```

Change the workspace version in a reviewed commit and update `Cargo.lock`. A tag must equal the workspace version with a leading `v`; tags are never moved. Further work after an RC must use a new `dev.N` or `rc.N` version.

Every push runs CI. Candidate and stable promotion rerun CI before tagging and packaging. Configure required reviewers on the GitHub `release` environment if human approval is desired.

Install stable by omitting `--channel`; install a candidate explicitly:

```sh
sudo bts-install install full
sudo bts-install install full --channel v0.4.0-rc.1
```

Branches and Actions artefacts are not installation sources. `stable` excludes drafts, prereleases and legacy releases without an Installer v2 manifest.

For an unpublished `dev.N` build, run `scripts/build-release all` and install its directory with `--release-dir`; see the [development guide](development.md).
