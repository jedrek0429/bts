# Bansleben Telephone Services

BTS is a phone-controlled home information system written in Rust. Asterisk handles calls, `bts-core` routes events and terminal-specific presentation state, independent addons provide behaviour, and each `bts-display` instance renders its registered terminal's presentation.

Components are independently deployable and communicate through the versioned contracts in `bts-protocol`.

## Documentation

- [Install and operate BTS](docs/installer-v2.md)
- [Build and run a development environment](docs/development.md)
- [Versioning and releases](docs/versioning.md)
- [Write addons with Addon API v1](docs/addon-api-v1.md)
- [Generate voice prompts](docs/voice-prompts.md)
- [Release manifest and bundle format](docs/release-manifest-v1.md)
- [Project roadmap](docs/roadmap.md)

## Development

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

Start only the native components needed in a reusable development session:

```sh
mkdir -p ~/.config/bts/dev
cp deploy/dev/core.env.example ~/.config/bts/dev/core.env
./scripts/bts-dev up core
```

Use the `voice` profile for Core, Addons and Telephony, or launch a named native Display separately. The [development guide](docs/development.md) covers component files, profiles, migration, local release bundles and hardware boundaries.

## Licence

BTS is licensed under GPL-3.0-or-later. See [LICENSE](LICENSE).

Copyright © 2026 BTS contributors.
