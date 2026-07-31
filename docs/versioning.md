# Versioning and releases

## Versions

| Category | Source/current | Changes when |
| --- | --- | --- |
| BTS product and all workspace crates | `[workspace.package].version` | Every BTS release |
| Core HTTP/WebSocket contract used by every remote component | `v1` | A remote API breaks compatibility |
| Core–Addon contract | Addon API `v1` | The addon contract breaks compatibility |
| Built-in addon implementation | Per-addon SemVer | That addon changes |
| Release manifest | Schema `1` | Manifest structure breaks compatibility |
| Component bundle | Format `1` | Archive layout breaks compatibility |
| Installer state | Schema `2` | Persistent state changes; migrations are required |
| Installer JSON output | Schema `1` | Machine-readable output breaks compatibility |

`[workspace.package].version` in `Cargo.toml` is the BTS product version. Every BTS crate inherits it. API and schema versions are independent and do not change with each product release. Additive contract changes stay within the current API version; breaking network contracts use a new route/module version alongside the old one during migration.

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
