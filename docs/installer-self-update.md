# Installer self-update and compatibility preflight

`bts-install` verifies compatibility before operations that can change a BTS installation.

## Self-update

Update the installer from the newest compatible stable GitHub Release:

```sh
sudo bts-install self-update
```

Select a published prerelease explicitly when testing a release candidate:

```sh
sudo bts-install self-update --channel v0.3.0-rc.2
```

The installer downloads the `bts-install` asset named by the release manifest, verifies its SHA-256 checksum, writes the replacement beside the running executable, syncs it, and activates it with an atomic rename. A failed download or verification leaves the running installer untouched. Self-update refuses an implicit downgrade.

Releases created before self-update support still require the documented checksum-verified manual bootstrap once. After an installer containing this feature is installed, later compatible releases can update it through this command.

## Compatibility preflight

Local-only mutating operations continue to work offline. Reading installer state validates its schema before `remove`, `configure`, or `uninstall` can make changes.

Operations that consume a release (`install`, `add`, and `upgrade`) fetch and validate the selected release manifest before package installation, account creation, service changes, tty takeover, staging, or activation. This verifies the release manifest and component bundle format before host mutation begins.

When a remote target release contains a newer installer, `bts-install` verifies and activates that installer first, then re-executes the original command through the new binary. A dry run reports that the self-update would occur without replacing the binary.

An installer that cannot parse the selected release manifest stops before changing the host. Run an explicit self-update with a compatible release before retrying when a future release changes a format beyond the running installer's supported schema.

## Upgrade release source

`bts-install upgrade` is network-aware for GitHub-backed installations. Unless `--repository` or `--channel` is supplied explicitly, it reuses the repository and release channel stored in installer state, then checks GitHub Releases before changing components.

A stable installation therefore follows the newest published compatible stable release and never moves to a prerelease implicitly. An explicitly pinned prerelease tag remains pinned until the operator selects another tag. For example:

```sh
sudo bts-install upgrade --channel v0.3.0-rc.2
```

Installations made from `--release-dir` remain local. Because local source paths are deliberately not persisted, a later local upgrade must provide `--release-dir` again.
