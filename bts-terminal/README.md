# bts-terminal

`bts-terminal` is the renderer-neutral endpoint runtime for a registered BTS
terminal. It owns WebSocket connection, registration readiness, heartbeats,
reconnect, dispatch validation, acknowledgements and graceful shutdown. BTS
Core remains authoritative for terminal definitions, online presence, target
resolution and presentation state.

The crate depends on `bts-protocol`; it does not depend on a renderer, egui,
`bts-display`, `bts-core`, an administrative SDK or a CLI framework.

## Consumer boundary

`TerminalRuntime::spawn` runs network work on one background thread and returns
a `TerminalHandle`. A concrete implementation polls `TerminalEvent` on the
thread where it performs local work. For a graphical implementation, this means
presentation application and repaint requests remain on the UI main thread.

The normal presentation flow is:

1. Wait until `ConnectionState::Registered` is observed. This is the readiness
   boundary; the runtime does not deliver presentations before Core has
   acknowledged registration.
2. Receive `TerminalEvent::PresentationReceived` and inspect its
   `PresentationWork`. The work carries Core's connection ID, terminal-local
   generation, validity deadline and single-use completion identity.
3. Check `PresentationWork::is_applicable` immediately before applying its
   semantic `DisplayState` locally. A newer generation, expiry or connection
   loss can invalidate work which was already queued to the UI.
4. Pass the work's completion identity to `accept_presentation` only after
   application succeeds, or to `reject_presentation` with a structured
   protocol rejection. Reusing a completion identity has no effect.
5. Continue polling connection-state and invalidation events so local status
   can reflect disconnects and retries.

No renderer trait is required. The runtime neither calls rendering code nor
owns repaint behaviour.

## Configuration and identity

`TerminalConfiguration::new` requires all of the following explicitly:

- the Core WebSocket URL;
- a validated stable `TerminalId`;
- a suggested user-facing `TerminalName`;
- an implementation identifier and semantic implementation version;
- functional protocol capabilities.

The consuming application decides how these values are loaded. The library has
no default terminal ID and never derives authoritative identity from a
hostname, IP address, connector, room name or physical output.

Bounded diagnostic key/value pairs may be attached with
`RuntimeDiagnostics`. Implementation version and diagnostics are retained in
the typed local configuration and sent as bounded fields during registration.
Core retains them only in live terminal presence; they do not affect identity,
routing or persisted administrative state.

## Lifecycle safety

Core supplies the connection ID when registration succeeds. The runtime uses
that exact ID for heartbeats, disconnect and presentation acknowledgements and
requires the same ID in a dispatch's per-terminal delivery context; pending
work is never transferred to a replacement connection. Dispatches whose
resolved recipients do not contain this terminal, omit its delivery context or
belong to another connection are ignored without an acknowledgement.

The runtime retains the greatest Core generation observed across reconnects.
A newer generation invalidates every older pending item before the newer work
is published, and the shared work status prevents an older item already queued
to a renderer from being treated as applicable. Recently observed presentation
IDs are also retained in a bounded cache so stale replays are not applied
again. Core supplies an epoch during registration; when that epoch changes, the
runtime clears its generation and replay history so a restarted Core can safely
begin a new generation sequence. Core does not replay presentations in v1.

Reconnect delay is deterministic exponential backoff capped by
`ReconnectPolicy::maximum_delay`. A successfully registered connection resets
the next delay to the initial value. Pending work has a bounded queue and uses
the validity duration supplied by Core rather than an unrelated local timeout.
At expiry the runtime invalidates the work and sends no terminal-side rejection;
Core independently settles the same delivery as timed out. This avoids a local
deadline rejecting work while a renderer still considers it applicable.

Call `TerminalHandle::shutdown` during normal application shutdown. If the
terminal is registered, the runtime sends the protocol disconnect message and
closes the WebSocket. Dropping the handle also signals shutdown and joins the
worker so it does not leave an orphaned task.

The production WebSocket connector uses Core's dedicated versioned terminal
endpoint at `/api/v1/terminals/ws`. The legacy event stream is a separate,
compatible transport and is not used for terminal registration or delivery.
