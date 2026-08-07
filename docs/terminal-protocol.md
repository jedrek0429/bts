# Terminal protocol foundation

The terminal protocol keeps a terminal's stable machine identity separate from
its user-facing name. Machine identifiers use lower-case ASCII letters, digits,
`.`, `_` and `-`, are limited to 64 bytes, and must start and end with a letter
or digit. Names are validated separately and may change without changing the
stable identity.

Capabilities are open functional identifiers such as `render_text` and
`play_audio`. Consumers retain valid unknown capability identifiers and ignore
those they do not understand. Hardware, operating-system and room details are
not routing capabilities. Registration may acquire optional fields in later
protocol versions, but routing decisions must use only typed, validated routing
fields.

Terminals connect to the dedicated `/api/v1/terminals/ws` WebSocket endpoint.
Registration reports a bounded semantic implementation version and up to 32
bounded runtime diagnostic key/value pairs. Diagnostics such as `platform`,
`architecture`, `renderer` and `display.resolution` are retained only with live
presence. They never affect identity, approved capabilities, routing, tags or
groups, and they are never written to the terminal registry file.

Terminal administration and delivery completion remain off the adjacent-version
event stream. Observers use `/api/v1/terminals/events/ws`, whose messages carry
the independent terminal event stream version.

## Target resolution

`TerminalTarget` represents an unresolved request for one terminal, a group, a
tag query or every terminal. `ResolvedTarget` is a distinct, non-empty set of
concrete terminal IDs selected by Core. Immediate presentation targets resolve
against online terminals by default. A caller must select the `registered`
scope explicitly for persistent operations which may include offline registered
terminals.

Core reports zero matches, offline terminals and unsupported required
capabilities with typed delivery outcomes. `PresentationDeliveryResult` retains
the original selector, its optional scope-resolved target and a deterministic
map of outcomes for every registered match. An empty outcome map means no
registered definition matched; registered-but-offline matches remain visible
as `offline`. Capability checks do not narrow the resolved target. A
presentation dispatch carries both the original selector and the concrete
scope-resolved terminals, and the two selectors must agree when the message is
decoded.

Online, compatible recipients acknowledge either acceptance or an explicit
rejection. Core also settles a pending outcome as `timed_out`, `superseded` or
`disconnected`. A dispatch contains a connection-specific, terminal-local
generation and validity period. A terminal must discard older generations and
expired work. Acknowledgements carry terminal, connection and presentation
identity; Core validates both the planned and current connection before
classifying them. Completion is published on versioned terminal event stream 1,
not the adjacent-compatible general event stream.

## Legacy display migration

The existing `display_requested` event remains wire-compatible and untargeted.
During migration, Core expands its show, update, release and release-all
operations into terminal-specific lease changes and dispatches. Addon shutdown
uses the same release-all path. `Event::legacy_presentation_request` remains a
deprecated content-only convenience, while the Core lifecycle adapter owns the
complete priority and restoration semantics. New protocol clients choose an
explicit `TerminalTarget`.

The operator sequence for assigning identities and running adjacent versions is
documented in [Single-display migration and rolling
compatibility](terminal-migration.md).

## Reserved DTMF controls

The platform owns these controls globally:

- `0`: session configuration
- `*`: cancel or go back
- `#`: confirm or complete input

`DtmfMenuKey` rejects these controls, including during deserialisation, so addon
menu entries cannot bind them. The built-in clear-display action uses `4` on
this release line.
