# Bansleben Telephone Services

BTS is a phone-controlled home information system written in Rust. Asterisk handles calls, `bts-core` routes events, independent addons provide behaviour, and `bts-display` renders the shared state.

Components are independently deployable and communicate through the versioned contracts in `bts-protocol`.

## Documentation

- [Install and operate BTS](docs/installer-v2.md)
- [Build and run a development environment](docs/development.md)
- [Versioning and releases](docs/versioning.md)
- [Write addons with Addon API v1](docs/addon-api-v1.md)
- [Generate voice prompts](docs/voice-prompts.md)
- [Release manifest and bundle format](docs/release-manifest-v1.md)
- [Project roadmap](docs/roadmap.md)

## Development build

Install a current stable Rust toolchain and the native libraries required by the Display crate, then build the workspace:

```sh
cargo build --workspace
```

Run the required checks:

```sh
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Start Core, Addons and Telephony in a reusable development session:

```sh
mkdir -p ~/.config/bts
cp deploy/bts-dev.env.example ~/.config/bts/dev.env
./scripts/bts-tmux
```

The launcher does not start Display. See the [development guide](docs/development.md) for prerequisites, configuration, individual component commands and Display setup.

## Licence

BTS is licensed under GPL-3.0-or-later. See [LICENSE](LICENSE).

Copyright © 2026 BTS contributors.
