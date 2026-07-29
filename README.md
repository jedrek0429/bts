# Bansleben Telephone Services

Bansleben Telephone Services is a phone-controlled home information system.

Calls enter through Asterisk. Telephone events are distributed through a central event service, interpreted by independent addons and shown on a dedicated display.

The system is written in Rust and designed as a collection of small, independent processes communicating over the network.

## Components

### `bts-core`

The central event service.

It:

- receives events over HTTP;
- assigns event IDs and timestamps;
- broadcasts events over WebSocket;
- retains the current canonical display state;
- provides the current state to newly connected clients.

Core does not interpret DTMF digits or decide which service should run.

### `bts-telephony`

The connection between Asterisk and BTS.

It:

- connects to Asterisk through ARI;
- receives call and DTMF events;
- publishes telephone events to core;
- will later provide spoken telephone menus and responses.

### `bts-addons`

The application layer.

Each addon lives in its own module and independently reacts to events received from core.

Current addons include:

- clock;
- weather;
- messages.

Addons may publish complete display states back to core. DTMF mappings and service behaviour belong here, not in core.

### `bts-display`

The fullscreen graphical client.

It:

- receives the retained display state from core;
- renders clock, weather, message and blank screens;
- contains presentation logic only.

It does not interpret telephone input or fetch external data.

### `bts-protocol`

Shared event and state types used by all components.

### `bts-cli`

Planned command-line client for inspecting and controlling BTS.

## Event flow

```text
Telephone
    │
FRITZ!Box
    │
Asterisk
    │ ARI
    ▼
bts-telephony
    │ HTTP
    ▼
bts-core
    │ WebSocket
    ├──────────────► bts-display
    │
    └──────────────► bts-addons
                         │
                         │ HTTP
                         └────────► bts-core
```
For example:
```
DTMF 2
    │
    ▼
bts-telephony publishes PhoneDtmfReceived
    │
    ▼
bts-core broadcasts the event unchanged
    │
    ▼
clock addon handles digit 2
    │
    ▼
clock addon publishes DisplaySet
    │
    ▼
bts-core retains and broadcasts the new display state
    │
    ▼
bts-display renders the clock
```
Architecture

All communication between runtime components takes place through the BTS HTTP and WebSocket interfaces.

bts-core acts as an event bus and retained-state service. It knows how to store a complete display state, but does not know why that state was selected.

Application behaviour lives in bts-addons. This includes DTMF routing, weather retrieval, clock updates, messages and future services.

bts-display renders complete display states without making application decisions.

## Building

Build the complete workspace:

```bash
cargo build --workspace
```

Build an optimised release:

```bash
cargo build --workspace --release
```

Check the workspace:

```bash
cargo check --workspace
```

Run formatting and lint checks:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Run tests:

```bash
cargo test --workspace
```

## Running

Start each service separately:

```bash
cargo run -p bts-core
cargo run -p bts-display
cargo run -p bts-telephony
cargo run -p bts-addons
```

By default, core listens on port 3100.

The event endpoints are:

```
POST /api/v1/events
GET  /api/v1/events/ws
GET  /api/v1/state
GET  /health
```

## Configuration

Components may be configured through environment variables.

Typical values include:

```bash
BTS_CORE_HTTP_URL=http://127.0.0.1:3100
BTS_CORE_WS_URL=ws://127.0.0.1:3100/api/v1/events/ws
```

Telephony additionally requires access to the configured Asterisk ARI service.

Current milestone

The current milestone is complete addon-controlled display operation.

This includes:

addons receiving telephone events from core;
addons selecting actions from DTMF input;
addons publishing complete display states;
the clock remaining live and showing seconds correctly;
weather updating automatically while its screen is active;
display state surviving client reconnection;
core remaining independent of DTMF mappings and addon behaviour.

The clock and weather services should continue updating only while their respective screen is active. Switching to another screen should stop the previous addon from publishing display updates.

Roadmap
1. Live addon-controlled display
complete the generic DisplaySet protocol;
update core to retain complete display states;
make display a pure renderer;
keep the clock updated every second;
refresh weather periodically;
stop inactive addons from overwriting the selected screen;
verify reconnection and retained-state behaviour.
2. bts-cli

Provide a btscli command for:

checking core health;
inspecting current state;
watching events;
requesting clock, weather and message actions;
blanking the display.

CLI requests should use the same addon logic as telephone requests.

3. Telephone speech

Add spoken prompts and responses to bts-telephony.

The initial menu should follow this form:

Welcome to Bansleben Telephone Services.
For the clock, press 2.
For the weather, press 3.

Fixed prompts should be generated in advance and cached. The voice should be a licensed, calm British announcement voice suitable for a transport-style information system.

4. Deployment
systemd service units;
automatic startup;
dependency ordering;
restart policies;
production configuration;
logging and diagnostics.
Status

BTS is under active development. Its internal protocol may change before the first stable release. 
                        
