# Bansleben Telephone Services (BTS)

BTS is a phone-controlled home information system written in Rust. Asterisk handles calls, BTS routes telephone events through a central service, and independent addons update a dedicated graphical display.

Every component is independently deployable. Components communicate only through the versioned `bts-protocol` contracts via `bts-core`; endpoint environment variables allow any service or third-party addon to run on another host without code changes.

The processes communicate over HTTP and WebSockets:

* `bts-core` retains display state and distributes events.
* `bts-telephony` bridges Asterisk ARI calls and DTMF to BTS.
* `bts-addons` supplies clock, weather and message behaviour.
* `bts-display` renders the current state full-screen.
* `bts-protocol` contains shared event and state types.

Addon authors should start with the [Addon API v1 author guide](docs/addon-api-v1.md).

## Installation

`bts-install` is the canonical v0.3 installation and upgrade path. Normal installation uses checksummed portable release bundles and needs neither a repository checkout, Rust toolchain nor native BTS package. Download the installer and its checksum from the same tagged release, verify it, then install it:

```sh
release=v0.3.0
curl --fail --location --remote-name \
  "https://github.com/jedrek0429/bts/releases/download/$release/bts-install"
curl --fail --location --remote-name \
  "https://github.com/jedrek0429/bts/releases/download/$release/bts-install.sha256"
sha256sum --check bts-install.sha256
chmod +x bts-install
sudo install -m 0755 bts-install /usr/local/bin/bts-install
sudo bts-install install full
```

Choose what the machine does with a role or explicit components:

```sh
sudo bts-install install server
sudo bts-install install display \
  --core-url ws://192.168.1.50:3100/api/v1/events/ws
sudo bts-install install --component core --component addons
sudo bts-install install --component addons \
  --core-http-url http://192.168.1.50:3100 \
  --core-ws-url ws://192.168.1.50:3100/api/v1/events/ws
sudo bts-install add telephony
sudo bts-install remove display
```

`full` selects Core, Telephony, Addons and Display; `server` omits Display; `display` installs only the remote-core display appliance. Components remain the source of truth and communicate only through published `bts-protocol` endpoints. The installer never adds a component because credentials are missing and never assumes separately deployed components share filesystems.

Configure secrets without command arguments:

```sh
sudo bts-install configure telephony
sudo bts-install configure display
sudo bts-install status
sudo bts-install status --json
sudo bts-install doctor
sudo bts-install upgrade
```

Configuration is preserved during upgrades and removal. `remove COMPONENT` stops and disables only that component; add `--purge` to remove its installer-owned configuration. `uninstall` removes all managed components while preserving configuration unless explicitly purged. Removing Display restores `getty@tty1` if the installer took control of it.

The complete CLI, state/configuration layout, rollback model and supported deployment flows are documented in [Installer v2](docs/installer-v2.md). Release publishers and alternative download clients should read the [release manifest and bundle specification](docs/release-manifest-v1.md).

### Sony TV and DRM output

Cage normally selects the connected DRM output automatically. Where a host has several graphics devices, set `WLR_DRM_DEVICES=/dev/dri/card0` in `/etc/bts/bts.env`. Output mode and overscan remain compositor/DRM concerns; BTS itself fills the surface supplied by Cage.

### Continuous integration and delivery

Every pull request runs formatting, compilation, Clippy, tests, release-asset consistency checks, ShellCheck and systemd unit validation. Tags matching `v0.3.*` build the installer and independent portable component bundles, then publish `bts-install`, `bts-install.sha256`, `release-manifest.json`, `SHA256SUMS`, `LICENSE` and every supported checksummed bundle. The installer selects assets only from that manifest.

The v0.3 installer supports public GitHub Releases. Private-release authentication is not part of this milestone.

## Voice prompts

BTS uses a local Kokoro service with the British `bf_emma` voice. The welcome and each menu option are generated separately in Asterisk's sounds directory and played as one playlist. Calls work without internet access after initial setup.

Start Kokoro:

```sh
docker run -d \
    --name kokoro \
    --restart unless-stopped \
    -p 127.0.0.1:8880:8880 \
    ghcr.io/remsky/kokoro-fastapi-cpu:v0.6.0
```

Generate prompts:

```sh
sudo -E /usr/lib/bts/generate-voice-prompts
```

Supported prompt settings:

```env
BTS_KOKORO_URL=http://127.0.0.1:8880/v1/audio/speech
BTS_ASTERISK_SOUNDS_DIR=/var/lib/asterisk/sounds/en/bts
BTS_KOKORO_VOICE=bf_emma
BTS_KOKORO_SPEED=1.05
```

## Development

```sh
cargo build --workspace
cargo test --workspace
cargo check --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Run the processes manually in separate terminals:

```sh
cargo run -p bts-core
cargo run -p bts-telephony
cargo run -p bts-addons
cage -- cargo run -p bts-display
```

For a reusable development session with Core, Addons, Telephony and an optional
remote-ARI SSH tunnel, copy `deploy/bts-dev.env.example` to
`~/.config/bts/dev.env` and run:

```sh
./scripts/bts-tmux
```

The launcher deliberately does not start Display. If the ARI password is not in
the environment, the Telephony window requests it without echoing or saving it.

## Development configuration

```env
BTS_CORE_URL=http://127.0.0.1:3100
BTS_CORE_HTTP_URL=http://127.0.0.1:3100
BTS_CORE_WS_URL=ws://127.0.0.1:3100/api/v1/events/ws
BTS_ARI_URL=http://127.0.0.1:8088
BTS_ARI_USERNAME=bts
BTS_ARI_PASSWORD=CHANGE_ME
BTS_ADDON_DATA_ROOT=/var/lib/bts/addons
```

Core listens on port 3100:

* `POST /api/v1/events`
* `GET /api/v1/events/ws`
* `GET /api/v1/state`
* `GET /api/v1/addons`
* `POST /api/v1/assets`
* `GET /api/v1/assets/{asset_id}`
* `GET /health`

## Licence

BTS is free software licensed under the GNU General Public License version 3 or, at your option, any later version. See [`LICENSE`](LICENSE).

Copyright © 2026 BTS contributors. Individual authorship remains recorded in Git history.

## Roadmap

- [x] Event bus (bts-core)
- [x] Display application (bts-display)
- [x] Telephony integration (bts-telephony)
- [x] Basic addon framework (bts-addons)
- [x] Basic voice system
- [x] systemd services (bts-install)
- [x] Partial CI/CD
- [x] Full CI/CD
- [x] Full Addon API
- [ ] `btscli` administration interface
- [ ] Full voice integration
- [ ] Display polish and additional screens
- [ ] Home Assistant addon
- [ ] Jarvis AI board
