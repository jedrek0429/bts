# Single-display migration and rolling compatibility

Older BTS installations used one implicit shared display and therefore have no
stable terminal identity. Upgrade each display host explicitly; never assign a
universal value such as `display`, `default` or `primary` to several machines.

## Upgrade procedure

1. Upgrade Core first. The newer Core retains the adjacent-version legacy event
   stream while adding the dedicated terminal endpoint.
2. On each display machine, choose a unique stable ID and a suggested name.
   Record which physical installation owns the ID.
3. Before upgrading that Display, write and inspect its component-scoped
   configuration:

   ```sh
   sudo bts-install configure display \
     --core-url ws://192.168.1.50:3100/api/v1/terminals/ws \
     --terminal-id bedroom-display \
     --terminal-name Bedroom
   ```

4. Upgrade that Display only after `/etc/bts/display.env` contains the intended
   endpoint, ID and name. Repeat with a different ID on every other host.
5. Confirm first registration in Core before relying on explicit targeting.
   Reconnects must reuse the same definition rather than create another one.

The installer refuses a Display upgrade when the terminal endpoint, ID or name
is missing. Non-interactive migration requires all three explicit options. It
does not generate a shared default, use the hostname or silently start an
unidentified display. A cloned system image must be reconfigured with a new ID
before its Display service first starts.

Changing `BTS_TERMINAL_NAME` after first registration only changes the future
suggestion. Changing `BTS_TERMINAL_ID` creates a different durable definition
and leaves the previous one offline; it is not a rename procedure.

## Rolling-version expectations

- A new Core can continue publishing the adjacent-compatible general event
  stream for an old Display while other displays migrate.
- An old Core has no `/api/v1/terminals/ws` endpoint, so a new Display cannot
  register until Core is upgraded.
- Legacy untargeted display actions continue through the compatibility adapter
  and are expanded independently for registered terminals.
- Explicit targeted presentations are terminal-protocol traffic and are not
  projected onto an old Display's shared screen.
- Pre-1.0 terminal protocol compatibility requires the same minor protocol
  line. An incompatible registration is rejected rather than guessed.
- Definitions and administrative metadata survive Core restart; live presence
  and effective presentation state do not. Terminals reconnect automatically,
  and a later explicit action establishes new presentation state.

Rollback must preserve `/etc/bts/display.env` and Core's `terminals.json`. An
older Display ignores the new identity fields, while a later upgrade can reuse
them. Do not delete or hand-edit the Core registry as part of rollback.

Physical upgrade activation, Raspberry Pi startup, display output and real
network interruption remain manual acceptance checks.
