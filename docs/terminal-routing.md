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

Every compatible live recipient receives a generation from a monotonic counter
owned by that terminal. Counters are independent, so work for different
terminals remains concurrent. Planning generation N immediately marks any
pending lower generation for that terminal as `superseded`; a reversed or late
acknowledgement is classified as `late` and cannot replace state from N. The
wire dispatch carries the planned connection ID, generation and validity period.
A terminal runtime must accept only its connection entry, remember the greatest
generation observed, discard lower generations and stop applying a dispatch
after its validity period. These rules cover both delayed rendering and delayed
transport without requiring synchronised wall clocks.

Only the connection which owned a terminal in the dispatch snapshot and still
owns its current registry presence may accept or reject that presentation.
Planned and current connection ownership is checked before a known
acknowledgement is called accepted, duplicate, late or stale. Unknown
presentations and terminals, stale connections, duplicate acknowledgements and
acknowledgements after timeout, supersession or disconnection therefore have
separate deterministic dispositions. Reconnection never transfers an
outstanding dispatch to the new connection. A transport must notify the manager
when its owning connection disconnects; otherwise the deadline still bounds the
pending delivery.

A completed result is emitted once on terminal event stream version 1. The
general Addon API event stream is unchanged and never contains terminal or
delivery variants, allowing an adjacent release's closed event enum to continue
decoding it. Results distinguish accepted, explicitly rejected, offline,
incompatible, timed-out, superseded and disconnected terminals.

Pending records are never reclaimed. By default Core retains up to 256
completed records and 256 additional compact tombstones. Current state
is stored separately and is never removed with delivery history. A tombstone
retains planned connection ownership and final outcomes, so an acknowledgement
after full-result eviction is still duplicate, late, stale or unexpected. Once
the independently bounded tombstone window is exhausted, the presentation is
deterministically unknown and its opaque ID may be reused. Persistence and
administrative API exposure are intentionally not part of this milestone slice.

## Independent semantic state

Core stores the effective accepted `DisplayState` independently for each
`TerminalId`, together with generation, presentation ID, event source, optional
Addon API v1 addon owner and optional legacy lease. Only a current-generation
accepted acknowledgement changes state.
Rejection, incompatibility, offline presence, timeout and disconnection leave
the previous state untouched. A single-terminal action therefore cannot change
another terminal; group and all actions change exactly their accepted
recipients.

State survives a terminal disconnect while Core remains running. A terminal may
preserve already applied local content during a transient disconnect, but Core
does not replay effective state on registration in terminal transport v1. A
reconnect receives only later explicit dispatches, with a new connection ID and
a generation greater than earlier work. State is not
persisted across Core restarts because Core cannot assume a terminal retained
the rendered state through that restart. Merely resolving or changing a future
telephony session target is a pure selection operation and does not copy,
clear, dispatch or mutate presentation state.

## Legacy boundary

The existing untargeted `display_requested` wire event is unchanged. The
deprecated Core adapter expands it into independently owned terminal dispatches.
Each online terminal has its own legacy lease stack, ordered by numeric priority
and then show order. Updates change every matching lease but dispatch only where
that lease is effective. Release, release-all and addon shutdown remove only the
matching addon's leases; removing an effective overlay restores that terminal's
next lease or explicitly dispatches blank. Offline released state is removed
from Core immediately so reconnect cannot restore stale ownership.

An explicitly targeted accepted presentation takes direct ownership of that
terminal and clears its legacy lease stack. It does not affect other terminals.
A later legacy show can take ownership again; releasing it restores another
retained legacy lease or blank, never an older targeted presentation. This gives
mixed callers a deterministic boundary without silently reviving stale content.
The release-line `BtsState.display` remains a best-effort single-display
projection for adjacent consumers; terminal state and lease authority live in
`PresentationManager`. New callers submit `presentation_requested` through the
existing event ingress; its `TerminalTarget` is passed to
`PresentationManager::begin_dispatch` and does not change the global legacy
`BtsState.display` projection. New callers must submit an explicit
`PresentationRequest`; the compatibility mapping is not the permanent routing
default.
