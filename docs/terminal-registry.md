# Core terminal registry

Core stores durable terminal definitions separately from live connection
presence. Definitions are written to `/var/lib/bts/terminals.json` by default;
`BTS_CORE_TERMINAL_STATE_PATH` may select another location. The systemd service
creates `/var/lib/bts` for the `bts` account.

The JSON file has a versioned schema and is replaced atomically after the new
file and its directory have been synchronised. A missing file creates an empty
registry. Malformed JSON, an unsupported schema or duplicate terminal IDs stop
Core startup rather than silently discarding definitions. Live presence,
remote addresses and heartbeat times are never serialised or restored.

On first registration, Core records the terminal's suggested name,
implementation and initially approved capabilities. Later registrations reuse
that definition: a new suggested name cannot overwrite it, the implementation
must match, and newly requested capabilities require future administrative
approval. Tags and groups are persisted with the definition but administration
of them belongs to issue #29.

A connection remains healthy through the configured 90-second presence timeout,
including the exact timeout boundary. Presence becomes stale only when the last
authenticated activity is older than that timeout. A healthy owner rejects a
second connection with `duplicate_terminal_id`; a stale owner may be replaced.
Heartbeat and disconnect operations must carry the owning connection ID.

Core checks for stale presence every 30 seconds. Expiry and disconnect remove
only presence, never the durable terminal definition. The current implicit
single display is not guessed or seeded because it has no stable identity; its
first future registration will create the initial definition through the same
validated flow.
