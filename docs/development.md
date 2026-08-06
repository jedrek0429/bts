# Developing BTS

## Prerequisites

Install a current stable Rust toolchain. The development launcher also needs tmux, curl and Python 3; SSH is required only for its optional ARI tunnel. Building `bts-display` requires Wayland, EGL, OpenGL, udev, fontconfig and keyboard development libraries.

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
  --core-url ws://CORE:3100/api/v1/terminals/ws \
  --terminal-id development-display \
  --terminal-name "Development Display"
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
BTS_CORE_WS_URL=ws://127.0.0.1:3100/api/v1/terminals/ws \
BTS_TERMINAL_ID=development-display \
BTS_TERMINAL_NAME="Development Display" \
  cage -- cargo run -p bts-display
```

Components connect to Core through their own environment variables and do not need to run on the same host. Do not source these as one shared block; `BTS_CORE_WS_URL`, in particular, names different endpoints for Addons and Display:

```env
BTS_CORE_URL=http://127.0.0.1:3100
BTS_CORE_HTTP_URL=http://127.0.0.1:3100
BTS_CORE_WS_URL=ws://127.0.0.1:3100/api/v1/events/ws # Addons event stream
BTS_ARI_URL=http://127.0.0.1:8088
BTS_ARI_USERNAME=bts
BTS_ARI_PASSWORD=CHANGE_ME
BTS_ADDON_DATA_ROOT=/var/lib/bts/addons
```

Do not commit real ARI credentials.

## Reusable native sessions

Create component-specific development files as needed:

```sh
mkdir -p ~/.config/bts/dev
cp deploy/dev/core.env.example ~/.config/bts/dev/core.env
cp deploy/dev/addons.env.example ~/.config/bts/dev/addons.env
cp deploy/dev/telephony.env.example ~/.config/bts/dev/telephony.env
cp deploy/dev/display.env.example ~/.config/bts/dev/display-bedroom.env
cp deploy/dev/terminal.env.example ~/.config/bts/dev/terminal-bedroom-display.env
```

Each tmux pane sources only its own file. Missing Core, Addons and Telephony files use loopback development defaults; Display requires explicit identity in a named file. State is isolated under `${XDG_STATE_HOME:-~/.local/state}/bts/dev/SESSION`, including Core's terminal registry and Addons data.

Select individual components or a reusable profile:

```sh
scripts/bts-dev up core
scripts/bts-dev up core addons
scripts/bts-dev up telephony
scripts/bts-dev up voice
scripts/bts-dev up two-terminals
scripts/bts-dev up terminal:bedroom-display
scripts/bts-dev up display:bedroom
scripts/bts-dev status core
```

`core` never reads Telephony configuration and needs no Asterisk or ARI credentials. Addons and Telephony may be selected independently and wait for their configured Core endpoint. Only a Telephony selection reads ARI settings, prompts for an omitted password, or creates the optional SSH tunnel configured by `BTS_ARI_SSH_HOST`. Readiness is ordered deterministically as Core, Addons, ARI tunnel, Telephony, headless terminals and named Displays.

The launcher reports the resolved components, configuration files and state directory. A matching existing session is reused; a session with a mismatched or unknown selection is rejected. Kill a session with `tmux kill-session -t SESSION` before applying configuration changes. `scripts/bts-tmux` remains a compatibility entry point and selects the `all` profile when invoked without arguments.

Profiles are plain component lists in `deploy/dev/profiles/*.components`. The
`two-terminals` profile starts one isolated Core plus
`terminal:bedroom-display` and `terminal:dining-display`. It uses the production
`bts-terminal` lifecycle through `bts-terminal-simulator` and does not start
Display, Telephony, Asterisk or ARI:

```sh
scripts/bts-dev up two-terminals
```

Without component files, the selector names are stable IDs and suggested names.
To use the Bedroom and Dining Room example, create independent files:

```sh
cp deploy/dev/terminal.env.example ~/.config/bts/dev/terminal-bedroom-display.env
cp deploy/dev/terminal.env.example ~/.config/bts/dev/terminal-dining-display.env
sed -i 's/bedroom-display/dining-display/; s/Bedroom/Dining Room/' \
  ~/.config/bts/dev/terminal-dining-display.env
```

Each simulator waits for Core's health endpoint, registers through
`/api/v1/terminals/ws`, accepts presentations and emits JSON lines for lifecycle
and presentation observations. `BTS_TERMINAL_CAPABILITIES` is a comma-separated
functional set. `BTS_TERMINAL_SIMULATOR_RESPONSE` may be `accept`, `reject` or
`ignore` for reproducible manual delivery experiments. Core registry state is
isolated beneath the session state directory.

Run the automated coherent-system coverage with:

```sh
cargo test -p bts-core --test multi_terminal
scripts/test-bts-tmux.sh
```

`BTS_DEV_PROFILE_DIR`, `BTS_DEV_CONFIG_DIR`, `BTS_DEV_STATE_DIR` and
`BTS_DEV_SESSION` provide explicit test or parallel-worktree isolation.

The old `~/.config/bts/dev.env` is deliberately not loaded because values such as `RUST_LOG` and `BTS_CORE_WS_URL` cannot be copied safely to every component. To migrate, move Core values to `core.env`, Addons HTTP/event-stream values to `addons.env`, ARI and `BTS_CORE_URL` values to `telephony.env`, and terminal identity plus the terminal WebSocket endpoint to a named `display-NAME.env`; then remove `dev.env`. The launcher reports this migration whenever the legacy file remains.

Display is still a native graphical process. Running it through the launcher does not verify Cage, DRM, Wayland, TTY, input devices or physical display hardware; those checks remain manual. The headless profile also does not verify Asterisk, audio, telephone hardware or real DTMF input.

## Core development endpoints

Core listens on port 3100 by default:

- `GET /health`
- `GET /api/v1/state`
- `GET /api/v1/addons`
- `GET /api/v1/events/ws`
- `GET /api/v1/terminals/ws`
- `GET /api/v1/terminals/events/ws`
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
  scripts/bts-dev \
  scripts/bts-tmux \
  scripts/test-bts-tmux.sh \
  scripts/test-release-assets.sh \
  scripts/generate-voice-prompts.sh
scripts/test-release-assets.sh
```

Use an isolated staged root when validating systemd units. Tests and development checks must not alter the host's real services, tty1, users, groups or `/etc` configuration.
