# Bansleben Telephone Services

Bansleben Telephone Services (BTS) is a local-first communications and information platform.

It is designed as a collection of small services which communicate using a common protocol. Services may run on the same machine or across a network without changing the application architecture.

## Workspace

```
bts-core
bts-display
bts-telephony
bts-cli
bts-addons
bts-protocol
```

## Components

### bts-protocol

Shared protocol definitions used by every BTS component.

Defines commands, events, retained state and serialisation.

### bts-core

Receives protocol messages, maintains retained state and distributes events to connected clients.

The core deliberately contains no service-specific logic.

### bts-display

Display client responsible for presenting retained state.

### bts-telephony

Telephone interface.

Connects to Asterisk and translates telephone activity into BTS protocol messages.

### bts-cli

Command-line client for administration, testing and diagnostics.

### bts-addons

Optional functionality which does not belong in the core.

Examples include clocks, weather, calendars and external integrations.

## Architecture

Every component communicates using `bts-protocol`.

Service crates do not depend directly on one another. Communication takes place over the BTS network interface.

## Building

Check the workspace:

```bash
cargo check --workspace
```

Build debug binaries:

```bash
cargo build --workspace
```

Build release binaries:

```bash
cargo build --workspace --release
```

Run tests:

```bash
cargo test --workspace
```

Format source:

```bash
cargo fmt --all
```

Lint:

```bash
cargo clippy --workspace --all-targets
```

## Services

The intended deployment is:

```
bts-core.service
bts-display.service
bts-telephony.service
bts-addons.service
```

## Roadmap

1. Complete `bts-telephony`.
2. Create `bts-cli` and the `btscli` executable.
3. Create `bts-addons` and move optional functionality into addon modules.
4. Package BTS as a set of systemd services.

## Design

BTS is intended to feel dependable rather than impressive.

Interfaces should be calm, restrained and functional. New functionality should normally be implemented as a client of BTS rather than by expanding `bts-core`.
