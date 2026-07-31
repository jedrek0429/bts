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
rejection. Core also settles a pending outcome as `timed_out` or `disconnected`.
Acknowledgements carry terminal, connection and presentation identity; a stale
connection cannot acknowledge work selected for a previous owner. Core emits a
`presentation_delivery_completed` event when every eligible recipient has a
bounded outcome.

## Legacy display migration

The existing `display_requested` event remains wire-compatible and untargeted.
During migration, `Event::legacy_presentation_request` converts legacy `show`
and `update` events into immediate presentations for `All` online terminals.
The adapter is deprecated so new protocol clients choose an explicit
`TerminalTarget`. Legacy `release` and `release_all` commands contain no
presentation content and are therefore not converted.

## Reserved DTMF controls

The platform owns these controls globally:

- `0`: session configuration
- `*`: cancel or go back
- `#`: confirm or complete input

`DtmfMenuKey` rejects these controls, including during deserialisation, so addon
menu entries cannot bind them. The built-in clear-display action uses `4` on
this release line.
