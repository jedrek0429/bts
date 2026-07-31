# Core terminal routing and presentation state

Core resolves a `TerminalTarget` from one atomic terminal-registry snapshot.
Terminal IDs are ordered lexically for repeatable menus, dispatch plans and
reports, but their position is never stored or exposed as identity. Group
membership is explicit and non-nested. Tag queries perform exact comparisons
against normalised tags and support `all` and `any` matching.

The resolver records all matching registered definitions before applying the
requested scope. `online` is the default for immediate presentation actions;
its resolved target contains only terminals with current presence. `registered`
retains offline definitions in the resolved target. In both cases delivery
outcomes include every registered match, so an empty match and an entirely
offline match remain distinct.

Required capabilities are checked against both the durable approved set and
the capabilities declared by the current presence. An online terminal missing
anything is reported as `incompatible`. It remains in the resolved target and
is never silently removed or replaced with another terminal. Offline terminals
are reported as `offline` because Core cannot establish their current runtime
capabilities.

## Bounded delivery

`PresentationManager::begin_dispatch` is transport-neutral: it returns the
protocol dispatch plus a sorted list of terminal and connection IDs for the
future `bts-terminal` transport. It performs no network I/O and never waits.
The default acknowledgement deadline is ten seconds. Core checks deadlines once
per second; tests can advance the supplied monotonic instant directly without
sleeping.

Only the connection which owned a terminal in the dispatch snapshot and still
owns its current registry presence may accept or reject that presentation.
Unknown presentations and terminals, stale connections, duplicate
acknowledgements and acknowledgements after timeout/disconnection have separate,
deterministic dispositions. Reconnection never transfers an outstanding
dispatch to the new connection. A transport must notify the manager when its
owning connection disconnects; otherwise the deadline still bounds the pending
delivery.

A completed result is retained in memory and emitted once as a
`presentation_delivery_completed` Core event. Results distinguish accepted,
explicitly rejected, offline, incompatible, timed-out and disconnected
terminals. Persistence and administrative API exposure are intentionally not
part of this milestone slice.

## Independent semantic state

Core stores the effective accepted `DisplayState` independently for each
`TerminalId`, together with the presentation ID, event source and optional
Addon API v1 addon owner. Only an accepted acknowledgement changes state.
Rejection, incompatibility, offline presence, timeout and disconnection leave
the previous state untouched. A single-terminal action therefore cannot change
another terminal; group and all actions change exactly their accepted
recipients.

State survives a terminal disconnect while Core remains running, allowing the
future terminal runtime to restore it explicitly after reconnect. It is not
persisted across Core restarts because Core cannot assume a terminal retained
the rendered state through that restart. Merely resolving or changing a future
telephony session target is a pure selection operation and does not copy,
clear, dispatch or mutate presentation state.

## Legacy boundary

The existing untargeted `display_requested` wire event is unchanged. The
deprecated Core adapter maps legacy `show` and `update` events to `All` online,
retaining their source and addon owner. Release operations still have no
presentation payload and are not adapted. New callers must submit an explicit
`PresentationRequest`; the compatibility mapping is not the permanent routing
default.
