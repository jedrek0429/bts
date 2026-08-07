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

Schema 2 adds descriptions, first/last-seen metadata and stable group resources.
Schema 1 files written by issue #28 are migrated atomically on load. Existing
terminal group IDs become non-nested groups whose initial display name is the
ID; no terminal or membership is discarded. Migrated definitions have unknown
first/last-seen values until their next accepted registration because the older
schema did not record a trustworthy wall-clock time.

On first registration, Core records the terminal's suggested name,
implementation and initially approved capabilities. Later registrations reuse
that definition: a new suggested name cannot overwrite it, the implementation
must match, and newly requested capabilities require future administrative
approval. Tags and groups are persisted with the definition but administration
is always performed by Core. Descriptions are optional, limited to 500
characters, trimmed, and may not contain control characters.

Tags are trimmed, converted to lower-case ASCII and then validated with the
protocol `TerminalTag` rules: 1–64 lower-case ASCII letters, digits, `.`, `_` or
`-`, beginning and ending with a letter or digit. Tags have no built-in meaning.
Adding or removing an already-present or absent tag is an idempotent no-op.

Groups have a stable `GroupId`, a separately editable display name and an
explicit set of terminal IDs. Membership is stored consistently on the group
and terminal definition, persists while a terminal is offline, and is
idempotent to add or remove. Groups cannot contain other groups. Deleting a
group removes membership references but never deletes or otherwise changes a
terminal.

Core exposes in-process operations for terminal rename and description/tag
changes, plus group creation, rename, deletion and membership changes. Applied
changes emit `terminal_metadata_changed` or `terminal_group_changed` events on
the existing Core event stream. No-op mutations emit no event. Administrative
events are Core-owned and rejected if submitted through the client event
endpoint. Administrative HTTP, SDK and CLI exposure is intentionally deferred
to Milestone 4.

A connection remains healthy through the configured 90-second presence timeout,
including the exact timeout boundary. Presence becomes stale only when the last
authenticated activity is older than that timeout. A healthy owner rejects a
second connection with `duplicate_terminal_id`; a stale owner may be replaced.
Heartbeat and disconnect operations must carry the owning connection ID.

Definition `first_seen` is the first accepted registration observed by a schema
which records timestamps. Definition `last_seen` follows the latest accepted
registration or authenticated live activity. Heartbeats update it in memory;
Core checkpoints it atomically when presence disconnects or expires, avoiding a
state-file replacement for every heartbeat. A crash may therefore retain the
last completed registration/checkpoint rather than the final heartbeat.

Core checks for stale presence every 30 seconds. Expiry and disconnect remove
only presence, never the durable terminal definition. The current implicit
single display is not guessed or seeded because it has no stable identity; its
first future registration will create the initial definition through the same
validated flow.
