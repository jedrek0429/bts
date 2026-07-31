# Developing BTS

## Prerequisites

Install a current stable Rust toolchain. Building `bts-display` also requires Wayland, EGL, OpenGL, udev, fontconfig and keyboard development libraries.

On Debian-family systems:

```sh
sudo apt-get install \
  pkg-config \
  libwayland-dev \
  libxkbcommon-dev \
  libegl1-mesa-dev \
  libgl1-mesa-dev \
  libudev-dev \
  libfontconfig1-dev \
  zstd
```

## Build and test

Build every workspace crate:

```sh
cargo build --workspace
```

Run the same Rust checks required before a pull request:

```sh
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Build optimised binaries with:

```sh
cargo build --workspace --release
```

The binaries are written to `target/debug/` or `target/release/`. Building from source is a development workflow; deployed systems should use [`bts-install`](installer-v2.md) and tagged release bundles.

Product, API and schema versions and the automated release flow are defined in [Versioning and releases](versioning.md).

## Build an installable development release

Build native release binaries and the same bundles, manifest and checksums used by GitHub Releases:

```sh
scripts/build-release all
```

Output is written to `target/bts-release/VERSION/`, where `VERSION` comes from `[workspace.package].version`. The command refuses crates with independent versions. It requires `tar`, `zstd` and the normal build dependencies.

Install that verified local release on a development machine:

```sh
version=$(scripts/release-version.py workspace-version)
sudo target/release/bts-install \
  --release-dir "target/bts-release/$version" \
  install display \
  --core-url ws://CORE:3100/api/v1/events/ws
```

Use the actual workspace version in the path. Local assets pass the normal manifest, checksum, archive and activation checks. `--release-dir` is accepted by `install`, `add` and `upgrade`; pass it again when upgrading from another local build. Use a VM or disposable host for service-level testing. `--root` is reserved for isolated tests and recovery and must not be treated as a container.

CI uses the same entry point in stages:

```sh
scripts/build-release component COMPONENT ARCH BINARY OUTPUT_DIRECTORY
scripts/build-release installer BINARY OUTPUT_DIRECTORY
scripts/build-release assemble OUTPUT_DIRECTORY
```

`component` creates one deterministic portable bundle. `installer` adds the installer and GPL licence. `assemble` creates and verifies the release manifest and checksum files. Do not reproduce this logic in workflow YAML.

## Run components manually

Run each process in a separate terminal:

```sh
cargo run -p bts-core
cargo run -p bts-addons
cargo run -p bts-telephony
cage -- cargo run -p bts-display
```

Components connect to Core through environment variables and do not need to run on the same host:

```env
BTS_CORE_URL=http://127.0.0.1:3100
BTS_CORE_HTTP_URL=http://127.0.0.1:3100
BTS_CORE_WS_URL=ws://127.0.0.1:3100/api/v1/events/ws
BTS_ARI_URL=http://127.0.0.1:8088
BTS_ARI_USERNAME=bts
BTS_ARI_PASSWORD=CHANGE_ME
BTS_ADDON_DATA_ROOT=/var/lib/bts/addons
```

Do not commit real ARI credentials.

## Reusable tmux session

Copy the development environment template:

```sh
mkdir -p ~/.config/bts
cp deploy/bts-dev.env.example ~/.config/bts/dev.env
```

Edit the copy, then start the session:

```sh
./scripts/bts-tmux
```

The launcher starts Core, Addons and Telephony. It can maintain an optional SSH tunnel to a remote ARI endpoint. It deliberately does not start Display. If `BTS_ARI_PASSWORD` is absent, the Telephony pane requests it without echoing or saving it.

## Core development endpoints

Core listens on port 3100 by default:

- `GET /health`
- `GET /api/v1/state`
- `GET /api/v1/addons`
- `GET /api/v1/events/ws`
- `GET /api/v1/assets/{asset_id}`
- `POST /api/v1/events`
- `POST /api/v1/assets`

Addon development is documented in [Addon API v1](addon-api-v1.md).

## Deployment-file checks

CI additionally runs ShellCheck, release-asset consistency tests and `systemd-analyze verify`. Relevant local commands are:

```sh
shellcheck \
  scripts/build-release \
  scripts/build-component-bundle \
  scripts/test-release-assets.sh \
  scripts/generate-voice-prompts.sh
scripts/test-release-assets.sh
```

Use an isolated staged root when validating systemd units. Tests and development checks must not alter the host's real services, tty1, users, groups or `/etc` configuration.
