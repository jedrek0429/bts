# Terminal platform architecture and operation

One BTS Core may coordinate several independently controlled terminals. A
terminal is a registered semantic presentation or interaction endpoint; it is
not a generic name for an Addon API consumer, an administrator or a process
which merely reads the Core event stream.

## Component boundaries

| Layer | Responsibility |
| --- | --- |
| `bts-protocol` | Stable identity, capabilities, target selectors and wire messages |
| `bts-core` | Durable definitions, ephemeral presence, groups, routing, delivery outcomes and terminal-local presentation state |
| `bts-terminal` | Registration, heartbeat, reconnect, filtering, generation safety and acknowledgements |
| `bts-terminal-simulator` | Headless development renderer built on the production runtime |
| `bts-display` | Graphical configuration, egui application and display diagnostics |
| Physical display hardware | DRM output, Raspberry Pi, panel, cables and input devices |

`bts-terminal` never renders content and `bts-display` does not implement a
second connection lifecycle. The simulator exercises the same runtime and Core
WebSocket endpoint as Display but accepts semantic presentations without egui,
Wayland or display hardware.

## Identity, definition and presence

`TerminalId` is a stable machine identity chosen for one installation. It must
not be derived from a changing address, DHCP lease, connector, menu number or
user-facing name. On first registration, Core creates a durable definition from
the suggested name, implementation and initially approved functional
capabilities. Later registration with the same ID reuses that definition;
changing the suggestion does not rename it administratively.

Presence is a live connection owned by a Core-issued connection ID. It carries
the currently reported protocol and implementation versions, capabilities and
runtime diagnostics. Presence disappears on disconnect, timeout or Core
restart. Definitions, descriptions, tags and non-nested group membership are
stored in `terminals.json` and remain while a terminal is offline. Two healthy
connections cannot own one ID; the second receives `duplicate_terminal_id`.

Capabilities describe behaviour which a terminal can honour, such as
`render_text`. Diagnostics describe the current process or hardware, such as
platform, renderer or resolution. Diagnostics are never approved routing data
and are not persisted.

## Targets and presentation state

Core resolves terminal, group, tag-query and all selectors from one registry
snapshot. Immediate actions use online scope by default. Matching definitions
remain visible in the delivery result even when they are offline or lack a
required capability. Core reports accepted, rejected, offline, incompatible,
timed-out, superseded and disconnected recipients without redirecting a target.

Each accepted terminal has its own effective semantic presentation and owner.
A direct action cannot overwrite another terminal. Group and all actions update
only recipients which accept them. Changing a telephony session target is only
a selection: it does not copy, clear or dispatch a presentation. Core does not
persist effective presentation state across restart and terminal transport v1
does not replay it after reconnect.

## Example configuration

The following is an operator-selected example, not a protocol model of rooms:

```text
Core: bansleben

Terminal: bedroom-display
Name: Bedroom
Tags: bedroom, private, upstairs

Terminal: dining-display
Name: Dining Room
Tags: dining-room, public, downstairs

Group: all-displays
Members: bedroom-display, dining-display
```

Each display host has its own `/etc/bts/display.env`:

```env
BTS_CORE_WS_URL=ws://192.168.1.50:3100/api/v1/terminals/ws
BTS_TERMINAL_ID=bedroom-display
BTS_TERMINAL_NAME=Bedroom
```

The second host must use its own ID, for example `dining-display`. Room names,
tags and the `all-displays` group are user configuration. Core contains no
Bedroom or Dining Room special cases.

Milestone 3 deliberately has no administrative HTTP API or terminal CLI. Tests
exercise Core's in-process metadata operations, but operators must not edit
`terminals.json` by hand. Supported remote group and tag administration belongs
to Milestone 4.

## Telephony controls

One call owns one mutable unresolved target. Temporary menu numbers are sorted
deterministically and discarded with the prompt; they are never terminal IDs.
The platform handles reserved controls before addon input:

- `0` opens session configuration;
- `*` cancels or returns;
- `#` confirms a variable-length selection.

An addon receives the target in its invocation context. Only a later addon
presentation uses a newly selected target.

## Manual acceptance boundary

Automated tests use loopback networking, an in-process production Core and the
headless simulator. They do not verify a physical display, Raspberry Pi, DRM,
Cage, seatd, egui rendering, Asterisk, audio, a telephone or real DTMF timing.
Those behaviours require manual testing on the intended hardware.
