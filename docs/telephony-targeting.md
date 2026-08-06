# Telephony session targeting

Each active ARI channel owns one in-memory `TelephonySession`. It retains caller
identity, an optional unresolved online `TerminalTarget`, the current menu or
addon context, a return stack and a reserved session-settings slot. Sessions are
removed before call-end publication, so late DTMF and target responses cannot
start a new action for an ended channel. No caller-to-terminal mapping is
persisted.

Core builds `GET /api/v1/telephony/targets` from one terminal-registry routing
snapshot. A suitable terminal is online and has the approved and currently
declared `render_text` capability. Suitable terminals are ordered by stable
terminal ID, available groups by stable group ID, and the all-online option follows them. Telephony assigns
temporary menu codes in that order. Codes use digits 1–9 in bijective base nine,
so variable-length selections never consume the globally reserved `0`; `#`
completes the code. Menu codes are discarded with the current prompt and are
never identities.

At call start, zero online terminals produce an explanatory prompt, one online
terminal is selected automatically, and several terminals open the target menu.
An individual selection remains that exact terminal even after disconnection.
Groups and all-online targets retain their unresolved selectors so Core resolves
their current membership at presentation dispatch time.

The platform handles reserved controls before addon digits:

- `0` suspends the current context and opens session configuration;
- `*` cancels the current selection or restores the previous context;
- `#` confirms the current variable-length target code.

Confirmation validates the original target against a fresh Core catalogue. A
disconnected choice is rejected and the menu is refreshed; it is never silently
replaced. Addon dispatch also validates the stored target immediately before
publishing the action. Core remains authoritative for a disconnect racing the
subsequent presentation dispatch.

Target confirmation mutates only `TelephonySession::selected_target`, announces
the result and restores the suspended menu/addon context. It does not publish an
action or presentation, copy a screen, clear a terminal or invoke an addon.

Real Asterisk playback, caller ID, telephone DTMF timing, audio quality and
hardware disconnect behaviour require manual verification.
