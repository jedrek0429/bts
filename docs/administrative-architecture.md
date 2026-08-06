# Administrative architecture

Milestone 4 adds an operator-facing administrative HTTP API, a typed Rust SDK
and a thin command-line interface. These roles are distinct from terminals,
addons and telephony sessions.

```text
bts-protocol
     ↑
  bts-sdk
     ↑
  bts-cli  (installed executable: btscli)
```

- `bts-protocol` owns validated resource identifiers, administrative wire DTOs,
  endpoint constants and structured server-error categories. It contains no
  HTTP client, argument parsing, prompts, output formatting or exit codes.
- `bts-core` remains authoritative for resource state, validation, reference
  resolution, persistence, idempotency and mutation safety. It currently serves
  discovery, process status and current-state resources; later issues add the
  frozen terminal and group operations.
- `bts-sdk` owns HTTP transport, timeouts, discovery and compatibility
  negotiation, URL construction, typed operations and decoding server errors.
  It must not contain terminal registration, heartbeat, presentation or
  renderer behaviour.
- `bts-cli` will depend on `bts-sdk`, not construct paths or duplicate DTOs. It
  owns Clap grammar, environment configuration, prompts, human formatting,
  colour and process exit behaviour.

The workspace dependency graph is enforced by
`scripts/test-administrative-boundaries.py`: `bts-sdk` depends on
`bts-protocol` without depending on Core or any runtime component. When
`bts-cli` is added, it must depend on `bts-sdk` without bypassing it.

The administrative surface manages BTS resources only. It does not start or
stop systemd services or containers, edit host or Asterisk configuration,
install addons, inject arbitrary events or DTMF, terminate telephony sessions,
or provide graphical administration. Addon administration remains deferred
until Addon API v1 work explicitly introduces it.

## Stage 1 assumptions

- Authentication, authorisation and TLS deployment are not specified by #36.
  A later transport policy may add headers or a secure origin, but must not make
  the v1 resource DTOs or SDK/CLI boundary depend on host administration.
- V1 list resources are unpaginated and deterministically ordered because the
  expected household registry is bounded. Introducing pagination would be a
  separately versioned contract decision.
- V1 has no ETag or caller-supplied revision. Core serialises mutations and
  returns the resulting resource; stale-state conflict handling remains a
  possible later additive transport feature.

See [Administrative API v1](administrative-api-v1.md) for the HTTP and DTO
contract and [btscli v1 contract](btscli-v1.md) for operator behaviour.
The implemented Rust client is described in [bts-sdk](bts-sdk.md).
