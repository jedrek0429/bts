# Bansleben Telephone Services (BTS)
BTS is a phone-controlled home information system written in Rust. It uses Asterisk for calls, routes telephone events through a central service, and updates a dedicated graphical display via independent addons.
------------------------------
# 🏗️ System Architecture
The system consists of small, independent processes communicating via HTTP and WebSockets.

* bts-core: The central event bus. It assigns IDs, timestamps, broadcasts via WebSocket, and retains the current display state. It contains no application logic.
* bts-telephony: The Asterisk bridge. It connects via ARI, translates calls and DTMF inputs into system events, plays telephone prompts, and pushes events to bts-core.
* bts-addons: The application layer. Independent modules (clock, weather, messages) process events and publish full display states back to bts-core.
* bts-display: A pure, full-screen graphical renderer. It displays the state retained by bts-core without making any logical decisions.
* bts-protocol: Shared event and state types.
* bts-cli: The planned command-line management tool.

------------------------------
# 🛠️ Development Commands

## Build & Test

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
# 🔊 Voice prompts

BTS uses the local Kokoro service with the British `bf_emma` voice. The welcome and each menu option are generated as separate files in Asterisk's sounds directory, then played as one playlist. Calls work without internet access after setup.

Kokoro does not expose a direct emotion setting. BTS synthesises each phrase separately at a slightly brisker speed so Emma sounds clearer and less subdued.

Start Kokoro on the BTS server:

```sh
docker run -d \
    --name kokoro \
    --restart unless-stopped \
    -p 127.0.0.1:8880:8880 \
    ghcr.io/remsky/kokoro-fastapi-cpu:v0.6.0
```

Install `curl` and `ffmpeg`, then generate the prompts:

```sh
sudo -E bash scripts/generate-voice-prompts.sh
```

The first Kokoro image download requires internet access. Prompt generation and telephone playback are local after that.

------------------------------
# ⚙️ Configuration & API
Configure components using environment variables:

```env
BTS_CORE_HTTP_URL=http://127.0.0.1:3100
BTS_CORE_WS_URL=ws://127.0.0.1:3100/api/v1/events/ws
BTS_ARI_PASSWORD=CHANGE_ME
BTS_MENU_MEDIA_URIS=sound:bts/welcome,sound:bts/press-2-time,sound:bts/press-3-weather,sound:bts/press-0-clear
```

`scripts/generate-voice-prompts.sh` also accepts:

```env
BTS_KOKORO_URL=http://127.0.0.1:8880/v1/audio/speech
BTS_ASTERISK_SOUNDS_DIR=/var/lib/asterisk/sounds/en/bts
BTS_KOKORO_VOICE=bf_emma
BTS_KOKORO_SPEED=1.05
```

# Core API Endpoints (Port 3100)

* POST /api/v1/events — Publish a new event
* GET /api/v1/events/ws — WebSocket event subscription stream
* GET /api/v1/state — Fetch current canonical display state
* GET /health — Health check endpoint

------------------------------
# 🎯 Project Status & Roadmap
## Current Status

* ✅ Core event bus operational
* ✅ Display state abstraction complete
* ✅ Asterisk ARI integration working
* ✅ Modular addon system ready
* ✅ Live background updates (Clock/Weather) functional
* ✅ Local Kokoro menu prompt playback

## Next Steps

1. bts-cli — Build the control utility to monitor health, state, and trigger actions.
2. Voice System — Add cached dynamic announcements and interruptible playback.
3. Deployment — Create systemd service units with proper startup dependencies and restart policies.

