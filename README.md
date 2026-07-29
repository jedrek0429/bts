# Bansleben Telephone Services (BTS)
BTS is a phone-controlled home information system written in Rust. It uses Asterisk for calls, routes telephone events through a central service, and updates a dedicated graphical display via independent addons.
------------------------------
# 🏗️ System Architecture
The system consists of small, independent processes communicating via HTTP and WebSockets.

* bts-core: The central event bus. It assigns IDs, timestamps, broadcasts via WebSocket, and retains the current display state. It contains no application logic.
* bts-telephony: The Asterisk bridge. It connects via ARI, translates calls and DTMF inputs into system events, and pushes them to bts-core.
* bts-addons: The application layer. Independent modules (clock, weather, messages) process events and publish full display states back to bts-core.
* bts-display: A pure, full-screen graphical renderer. It displays the state retained by bts-core without making any logical decisions.
* bts-protocol: Shared event and state types.
* bts-cli: The planned command-line management tool.

```text
[ Telephone ] ──► [ Asterisk ] ──► [ bts-telephony ]
        │ (HTTP)
        ▼
[ bts-display ] ◄── (WS) ─────────── [ bts-core ] ◄── (HTTP) ── [ bts-addons ]
```

------------------------------
# 🛠️ Development Commands# Build & Test

```sh
cargo build --workspace          # Build development version
cargo build --workspace --release  # Build optimized release
cargo test --workspace           # Run all tests
```

# Quality Control

```sh
cargo check --workspace          # Fast compilation check
cargo fmt --all --check          # Check code formatting
cargo clippy --workspace --all-targets -- -D warnings  # Run linter
```

# Running the System
Start each service in a separate terminal:

```sh
cargo run -p bts-core
cargo run -p bts-telephony
cargo run -p bts-addons
```

# Running display

Run `bts-display` using `cage`.

------------------------------
# ⚙️ Configuration & API
Configure components using environment variables:

BTS_CORE_HTTP_URL=http://127.0.0.1:3100
BTS_CORE_WS_URL=ws://127.0.0.1:3100/api/v1/events/ws
BTS_ARI_PASSWORD=CHANGE_ME

# Core API Endpoints (Port 3100)

* POST /api/v1/events — Publish a new event
* GET /api/v1/events/ws — WebSocket event subscription stream
* GET /api/v1/state — Fetch current canonical display state
* GET /health — Health check endpoint

------------------------------
# 🎯 Project Status & Roadmap# Current Status

* ✅ Core event bus operational
* ✅ Display state abstraction complete
* ✅ Asterisk ARI integration working
* ✅ Modular addon system ready
* ✅ Live background updates (Clock/Weather) functional

# Next Steps

1. bts-cli — Build the control utility to monitor health, state, and trigger actions.
2. Voice System — Implement cached, calm British TTS announcements for the interactive menu.
3. Deployment — Create systemd service units with proper startup dependencies and restart policies.

