# Installer self-update and compatibility preflight

`bts-install` verifies compatibility before operations that can change a BTS installation.

## Self-update

Update the installer from the newest compatible stable GitHub Release:

```sh
sudo bts-install self-update
```

Update only the installer from a bounded candidate line:

```sh
sudo bts-install self-update --track rc/0.3
```

`self-update` changes only the installer binary and does not change installation state. Use `bts-install upgrade --track rc` to enrol the managed installation into the newest candidate line; that selection is persisted in bounded form, such as `rc/0.3`. Later plain upgrades follow rc.2, rc.3 and subsequent candidates on that line without moving to `rc/0.4`. Passing bare `--track rc` again is the explicit action that may select a newer candidate line. Use `--release v0.3.0-rc.2` to pin one exact release instead.

The installer downloads the `bts-install` asset named by the release manifest, verifies its SHA-256 checksum, writes the replacement beside the running executable, syncs it, and activates it with an atomic rename. A failed download or verification leaves the running installer untouched. Self-update refuses an implicit downgrade.

Releases created before self-update support still require the documented checksum-verified manual bootstrap once. After an installer containing this feature is installed, later compatible releases can update it through this command.

## Compatibility preflight

Local-only mutating operations continue to work offline. Reading installer state validates its schema before `remove`, `configure`, or `uninstall` can make changes.

Operations that consume a release (`install`, `add`, and `upgrade`) fetch and validate the selected release manifest before package installation, account creation, service changes, tty takeover, staging, or activation. This verifies the release manifest and component bundle format before host mutation begins.

When a remote target release contains a newer installer, `bts-install` verifies and activates that installer first, then re-executes the original command through the new binary. A dry run reports that the self-update would occur without replacing the binary.

An installer that cannot parse the selected release manifest stops before changing the host. Run an explicit self-update with a compatible release before retrying when a future release changes a format beyond the running installer's supported schema.

## Upgrade release source

`bts-install upgrade` is network-aware for GitHub-backed installations. Unless `--repository`, `--track` or `--release` is supplied explicitly, it reuses the repository and release selection stored in installer state, then checks GitHub Releases before changing components.

A `stable` installation follows the newest published compatible stable release and never moves to a prerelease implicitly. `stable/0.3` remains on the 0.3 stable line. A candidate track such as `rc/0.3` follows candidates only on that line; when the stable release corresponding to its newest candidate is published, the installer offers it and persists `stable/0.3`. Exact releases remain pinned. For example:

```sh
sudo bts-install upgrade --track rc/0.3
sudo bts-install upgrade --release v0.3.0-rc.2
```

For compatibility, legacy `--channel v0.3.0-rc.1` selections and matching Installer v2 state are migrated to `rc/0.3`. Legacy stable version tags remain exact pins.

Installations made from `--release-dir` remain local. Because local source paths are deliberately not persisted, a later local upgrade must provide `--release-dir` again.
