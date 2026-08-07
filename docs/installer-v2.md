# `bts-install`

`bts-install` is the supported BTS deployment manager. It installs portable release bundles rather than building source or installing native BTS packages.

## Install the installer

Download `bts-install` and its checksum from the same tagged release. Verify the checksum before running or installing the binary:

```sh
release=v0.3.0
curl --fail --location --remote-name \
  "https://github.com/jedrek0429/bts/releases/download/$release/bts-install"
curl --fail --location --remote-name \
  "https://github.com/jedrek0429/bts/releases/download/$release/bts-install.sha256"
sha256sum --check bts-install.sha256
chmod +x bts-install
sudo install -m 0755 bts-install /usr/local/bin/bts-install
```

The installer reads public GitHub Releases. Private-release authentication is not supported.

## Quick start

Install a complete single-host system:

```sh
sudo bts-install install full
```

Install Core, Telephony and Addons without a local Display:

```sh
sudo bts-install install server
```

Install an ARM64 Display that connects to Core on another host:

```sh
sudo bts-install install display \
  --core-url ws://192.168.1.50:3100/api/v1/terminals/ws \
  --terminal-id bedroom-display \
  --terminal-name Bedroom
```

Install explicit components:

```sh
sudo bts-install install \
  --component core \
  --component addons
```

Install only the operator CLI on an administration machine:

```sh
sudo bts-install install custom --component cli
```

For a non-interactive Display installation, supply the endpoint, stable identity, suggested name and confirmation:

```sh
sudo bts-install install display \
  --core-url ws://192.168.1.50:3100/api/v1/terminals/ws \
  --terminal-id bedroom-display \
  --terminal-name Bedroom \
  --yes
```

## Roles and components

Roles are shortcuts resolved before planning:

| Role | Components |
| --- | --- |
| `full` | Core, Display, Telephony, Addons, CLI |
| `server` | Core, Telephony, Addons, CLI |
| `display` | Display |
| `custom` | Explicit `--component` selections |

Core, Display, Telephony, Addons and CLI remain independently deployable. The
runtime components have their own service, account requirements, configuration
and activation link. CLI is service-less and publishes `/usr/bin/btscli`; it
does not install Display, Cage, Asterisk or Telephony dependencies. A remote
Display reads `BTS_CORE_WS_URL`; it neither requires nor orders itself after a
local Core service.

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

Common options:

| Option | Purpose |
| --- | --- |
| `--component COMPONENT` | Select a component for a custom installation |
| `--core-url URL` | Set Display's remote Core WebSocket URL |
| `--core-http-url URL` | Set the remote Core HTTP URL for Addons or Telephony |
| `--core-ws-url URL` | Set the remote Core WebSocket URL for Display or Addons |
| `--terminal-id ID` | Set Display's stable terminal identity |
| `--terminal-name NAME` | Set Display's suggested terminal name |
| `--cage-args ARGS` | Override Cage arguments for Display |
| `--repository OWNER/REPO` | Select the public release repository |
| `--channel stable` | Use the newest published compatible release |
| `--channel vVERSION` | Use a specific stable or prerelease tag |
| `--release-dir PATH` | Use a locally built, verified release directory |
| `--dry-run` | Print the plan without changing the machine |
| `--yes` | Confirm changes without a prompt |
| `--no-start` | Install and enable without starting services |
| `--json` | Print stable machine-readable output where supported |
| `--quiet` | Suppress normal output |
| `--purge` | Remove installer-owned configuration during removal |

`--root` exists for isolated testing and recovery. It is not a container or chroot deployment interface.

## Persistent state

Installer state is stored at `/var/lib/bts-install/state.json` with mode `0600`. It records schema and installer versions, the overall and per-component BTS versions, selected role, authoritative component set, release source, platform/architecture, timestamps and whether tty1 was installer-managed. Local source paths and credentials are never stored. Writes use a fully synced temporary file, atomic rename and parent-directory sync. Schema 1 is migrated to schema 2 when read; newer schemas are rejected.

## Configuration

Each service loads only its authoritative component file:

```text
/etc/bts/core.env
/etc/bts/display.env
/etc/bts/telephony.env
/etc/bts/addons.env
```

Every unit sets `RUST_LOG=info` before loading its component file, so `RUST_LOG` in that file may override the safe default. Core addresses live only with the consuming component. No service loads `/etc/bts/bts.env`.

Installer-written files use mode `0640` in the traversable, non-writable `/etc/bts` directory. Server component files are owned by `root:bts`; Display configuration is owned by `root:bts-display`, so a display-only host needs no unrelated service account. Interactive Telephony configuration reads and confirms the ARI password without echoing it. Automation uses `--secret-file` (with no group or other access) or `--secret-fd`; password command arguments are deliberately unsupported. ARI validation distinguishes unreachable endpoints, rejected authentication, malformed configuration and successful responses before configuration replacement or service restart.

### Migration from `/etc/bts/bts.env`

The first mutating installer operation plans a component-scoped migration before replacing a service unit. Uniquely owned settings move into the installed component's file without overwriting a different existing value. `BTS_CORE_WS_URL` is assigned only when its versioned path identifies either the Display terminal stream or the Addons event stream. The legacy file is removed only after all authoritative files have been written with their service ownership and permissions.

Ambiguous values are never copied broadly. The old packaged `RUST_LOG=info` is consumed because it exactly matches every new unit default; a customised shared `RUST_LOG` with several installed components, an unknown setting, an endpoint without a recognised path, a setting for an uninstalled component, or a conflict with an existing component file stops the operation and retains `bts.env`. Move the value explicitly to each intended component file and retry. `bts-install doctor` reports both migratable and ambiguous legacy configuration. A single-component installation can migrate a customised `RUST_LOG` unambiguously.

Removal without `--purge` preserves the removed component's file. Purging removes only that component file; it never removes another component's settings. Legacy configuration is migrated before removal so uninstalling one component cannot discard settings belonging to another.

Configure Telephony interactively:

```sh
sudo bts-install configure telephony
```

For automation, prepare a root-owned environment file containing `BTS_ARI_PASSWORD` and pass it without placing the password in command arguments:

```sh
sudo bts-install configure telephony \
  --secret-file /root/bts-telephony.env
```

The file must not be accessible to group or other users. It may also contain `BTS_ARI_URL`, `BTS_ARI_USERNAME` and `BTS_CORE_URL`.

Display configuration requires the dedicated `/api/v1/terminals/ws` endpoint, a stable terminal ID and a suggested name. Interactive installation prompts for all three; non-interactive installation requires `--core-url`, `--terminal-id` and `--terminal-name`. The ID is written to `/etc/bts/display.env` before the service first starts and must remain unchanged across process restarts, upgrades, DHCP changes and Core restarts. It is never derived from the hostname, address or graphics connector.

The name is only the suggestion used when Core first registers the ID. Reconfiguring the name does not overwrite an administrator-managed name in Core. Changing the ID creates a different terminal definition and leaves the old definition offline.

Connectivity may be temporarily unavailable: Display keeps the last successfully applied presentation when it has one, shows a subdued disconnected indicator and reconnects automatically. With no usable presentation it shows a calm full-screen connection state. Registration failures, including a duplicate ID, are shown explicitly.

Cage defaults to `-m last`. Supply an alternative during installation or configuration:

```sh
sudo bts-install install display \
  --core-url ws://192.168.1.50:3100/api/v1/terminals/ws \
  --terminal-id bedroom-display \
  --terminal-name Bedroom \
  --cage-args "-m extend"

sudo bts-install configure display \
  --cage-args "-m extend"
```

The value is stored as `BTS_CAGE_ARGS` in `/etc/bts/display.env`. The service uses systemd's unbraced `$BTS_CAGE_ARGS` form, which splits the value into arguments before the fixed `-- bts-display` command delimiter without invoking a shell. Do not include the `--` delimiter in the custom value. Run `cage --help` for arguments supported by the installed Cage version.

Before upgrading an installation from the earlier implicit single-display model, migrate its configuration explicitly:

```sh
sudo bts-install configure display \
  --core-url ws://192.168.1.50:3100/api/v1/terminals/ws \
  --terminal-id bedroom-display \
  --terminal-name Bedroom
sudo bts-install upgrade display
```

The installer refuses to activate a new Display release while the endpoint or identity is missing or still points at the legacy event stream. This leaves the existing display release running until its identity is visible and editable in `/etc/bts/display.env`.

Upgrade Core before any Display. Repeat the configuration step separately on
every display-only host and choose a different ID each time; never copy a
universal `display` or `default` identity into several machines. A cloned image
must be assigned a new ID before first service start. Detailed mixed-version
behaviour and rollback guidance are in [Single-display migration and rolling
compatibility](terminal-migration.md).

Remote Addons and Telephony hosts use `--core-http-url`; Addons additionally accepts `--core-ws-url`. Non-interactive installation of a Core client without a local Core requires its applicable endpoint flags, so the installer never silently assumes a local service.

## Installation and reconciliation

The installer detects Debian-family or Arch Linux through `/etc/os-release` and normalises `amd64`/`x86_64` and `arm64`/`aarch64`. Only distribution-specific runtime dependency names and commands live in platform adapters. Native BTS packages are out of scope.

The selected release manifest provides every asset name and checksum. Assets are downloaded, SHA-256 verified, checked for compatible manifest/bundle schemas, and inspected before extraction. Absolute paths, parent traversal, special members and escaping links are rejected. Re-running an already satisfied component plan performs no work. Adding or removing one component does not restart unrelated services; dependency references are calculated from the complete desired component set.

`stable` selects the highest published semantic version with an Installer v2 manifest and excludes drafts and prereleases. Select a public candidate explicitly, for example `--channel v0.4.0-rc.1`. Developers can build the same asset set locally and pass it with `--release-dir`; local files receive the same manifest and checksum validation, and `doctor` stays offline for their availability check. Branches and Actions artefacts are not installation sources.

Display installation creates the kiosk account, installs Cage/seatd/font runtime dependencies, grants display device groups, discloses tty1 takeover, and enables only Display. ARM64 DRM, Cage, seatd and tty behaviour must still be accepted on real supported hardware.

### Display hardware

Cage normally selects the connected DRM output. If a host has several graphics devices, set the required device in `/etc/bts/display.env`:

```env
WLR_DRM_DEVICES=/dev/dri/card0
BTS_CAGE_ARGS=-m extend
```

Display owns tty1 while installed. Removing Display through `bts-install` restores the login service when tty1 was installer-managed.

## Activation, upgrades and rollback

Complete component releases are staged beneath:

```text
/usr/lib/bts/components/<component>/releases/<version>/
/usr/lib/bts/components/<component>/current -> releases/<version>
```

The `current` link is replaced atomically only after bundle validation. An upgrade defaults to the installed component set and rejects explicit uninstalled components. Only affected services are stopped and started. If required startup fails, activation links are restored to their previous targets and rollback success is reported. Configuration and installer state are not replaced by a failed activation.

## Status and doctor

`status` reports installer/BTS versions, role, component installation, unit enablement/activity and configured remote endpoints without secrets. `doctor` checks state, component configuration permissions, activated binaries and service activity, and supplies a concrete recovery command for errors. Hardware-specific graphics, seat, tty, Asterisk and ARI behaviour requires real-host acceptance and is not claimed by automated tests.

```sh
bts-install status
bts-install status --json
sudo bts-install doctor
sudo bts-install doctor --json
```

## Removal

Removal stops and disables only selected services, removes their activation link, updates state and preserves configuration. `--purge` additionally removes the selected installer-owned component configuration. Display removal unmasks and restores the tty1 login service. User data and release history are retained because they may not be solely installer-owned.

```sh
sudo bts-install remove display
sudo bts-install uninstall
sudo bts-install uninstall --purge
```

## Legal interface

Interactive mutating entry points show the program/version, `GPL-3.0-or-later` identifier, no-warranty notice and `licence` command. Help, version, licence and warranty work offline. The full GPL text is installed at `/usr/share/licenses/bts/LICENSE`; `licence` honours `$PAGER` and otherwise writes it directly.

Copyright notices use `Copyright © YEAR BTS contributors`. Individual authorship remains preserved by Git history and contributor records.

Release publishers should also read the [release manifest and portable bundle specification](release-manifest-v1.md).
