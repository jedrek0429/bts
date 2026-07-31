# BTS Installer v2

`bts-install` is the sole supported BTS v0.3 deployment manager. It installs portable release bundles rather than building source or installing native BTS packages.

## Roles and components

Roles are shortcuts resolved before planning:

| Role | Components |
| --- | --- |
| `full` | Core, Display, Telephony, Addons |
| `server` | Core, Telephony, Addons |
| `display` | Display |
| `custom` | Explicit `--component` selections |

Core, Display, Telephony and Addons remain independently deployable. Each component has its own bundle, service, account requirements, configuration and activation link. A remote Display reads `BTS_CORE_WS_URL`; it neither requires nor orders itself after a local Core service.

## Commands

```text
bts-install install [ROLE]
bts-install add COMPONENT...
bts-install remove COMPONENT... [--purge]
bts-install upgrade [COMPONENT...]
bts-install configure [COMPONENT]
bts-install status [--json]
bts-install doctor [--json]
bts-install uninstall [COMPONENT...] [--purge]
bts-install licence
bts-install warranty
```

`--dry-run` resolves and prints the same plan used for a real operation. `--yes` confirms host changes for non-interactive use. `--no-start` stages and enables new releases without starting them. `--quiet` suppresses normal output; `--json` provides stable machine-readable plans, status and diagnostics without an interactive legal banner.

## Persistent state

Installer state is stored at `/var/lib/bts-install/state.json` with mode `0600`. It records schema and installer versions, the overall and per-component BTS versions, selected role, authoritative component set, repository/channel, platform/architecture, timestamps and whether tty1 was installer-managed. It never stores credentials. Writes use a fully synced temporary file, atomic rename and parent-directory sync. Schema 1 is migrated to schema 2 when read; newer schemas are rejected.

## Configuration

Services load a shared non-secret `/etc/bts/bts.env` and one optional component file:

```text
/etc/bts/core.env
/etc/bts/display.env
/etc/bts/telephony.env
/etc/bts/addons.env
```

Installer-written files use mode `0640` in the traversable, non-writable `/etc/bts` directory. Server component files are owned by `root:bts`; Display configuration is owned by `root:bts-display`, so a display-only host needs no unrelated service account. Interactive Telephony configuration reads and confirms the ARI password without echoing it. Automation uses `--secret-file` (with no group or other access) or `--secret-fd`; password command arguments are deliberately unsupported. ARI validation distinguishes unreachable endpoints, rejected authentication, malformed configuration and successful responses before configuration replacement or service restart.

Display configuration accepts `--core-url` or a prompt and requires the published WebSocket path. Connectivity may be temporarily unavailable: Display stays active, shows a calm disconnected state and reconnects automatically.

Remote Addons and Telephony hosts use `--core-http-url`; Addons additionally accepts `--core-ws-url`. Non-interactive installation of a Core client without a local Core requires its applicable endpoint flags, so the installer never silently assumes a local service.

## Installation and reconciliation

The installer detects Debian-family or Arch Linux through `/etc/os-release` and normalises `amd64`/`x86_64` and `arm64`/`aarch64`. Only distribution-specific runtime dependency names and commands live in platform adapters. Native BTS packages are out of scope.

The selected release manifest provides every asset name and checksum. Assets are downloaded, SHA-256 verified, checked for compatible manifest/bundle schemas, and inspected before extraction. Absolute paths, parent traversal, special members and escaping links are rejected. Re-running an already satisfied component plan performs no work. Adding or removing one component does not restart unrelated services; dependency references are calculated from the complete desired component set.

Display installation creates the kiosk account, installs Cage/seatd/font runtime dependencies, grants display device groups, discloses tty1 takeover, and enables only Display. ARM64 DRM, Cage, seatd and tty behaviour must still be accepted on real supported hardware.

## Activation, upgrades and rollback

Complete component releases are staged beneath:

```text
/usr/lib/bts/components/<component>/releases/<version>/
/usr/lib/bts/components/<component>/current -> releases/<version>
```

The `current` link is replaced atomically only after bundle validation. An upgrade defaults to the installed component set and rejects explicit uninstalled components. Only affected services are stopped and started. If required startup fails, activation links are restored to their previous targets and rollback success is reported. Configuration and installer state are not replaced by a failed activation.

## Status and doctor

`status` reports installer/BTS versions, role, component installation, unit enablement/activity and configured remote endpoints without secrets. `doctor` checks state, component configuration permissions, activated binaries and service activity, and supplies a concrete recovery command for errors. Hardware-specific graphics, seat, tty, Asterisk and ARI behaviour requires real-host acceptance and is not claimed by automated tests.

## Removal

Removal stops and disables only selected services, removes their activation link, updates state and preserves configuration. `--purge` additionally removes the selected installer-owned component configuration. Display removal unmasks and restores the tty1 login service. User data and release history are retained because they may not be solely installer-owned.

## Legal interface

Interactive mutating entry points show the program/version, `GPL-3.0-or-later` identifier, no-warranty notice and `licence` command. Help, version, licence and warranty work offline. The full GPL text is installed at `/usr/share/licenses/bts/LICENSE`; `licence` honours `$PAGER` and otherwise writes it directly.

Copyright notices use `Copyright © YEAR BTS contributors`. Individual authorship remains preserved by Git history and contributor records.
