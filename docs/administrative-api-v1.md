# Administrative API v1

This document freezes the resource and wire contract. Core serves discovery,
status, state, terminal, group and addon administration paths below.

## Discovery and versioning

`GET /api` is the unversioned discovery request. It returns `ApiDiscovery`,
including the Core product version and the exact supported administrative API
versions. A consumer selects a version it supports and uses the advertised
`base_path`; it must not guess paths from the product version.

The v1 discovery DTO advertises one `base_path`, for `current`. Consequently,
the initial SDK requires that its supported v1 is both current and present in
the supported set. Advertising a path for a non-current concurrent version
would require an additive discovery contract in a later issue; the SDK does
not invent such a mapping.

Administrative v1 uses `/api/v1/admin`. Its compatibility number comes from
`compatibility.json` through `bts-compat`; documentation is not a second
version source. The existing `/api/v1/state`, event, active-addon and
terminal-runtime routes retain their current contracts and are not
administrative SDK routes.

## Resources and methods

| Method | Path | Request | Success |
| --- | --- | --- | --- |
| `GET` | `/api` | — | `ApiDiscovery` |
| `GET` | `/api/v1/admin/status` | — | `CoreStatusResource` |
| `GET` | `/api/v1/admin/state` | — | `CoreStateResource` |
| `GET` | `/api/v1/admin/addons` | — | `AddonListResource` |
| `GET` | `/api/v1/admin/addons/{addon}` | — | `AddonResource` |
| `PUT` | `/api/v1/admin/addons/{addon}/enabled` | `SetAddonEnabledRequest` | `MutationResponse<AddonResource>` |
| `GET` | `/api/v1/admin/terminals` | — | `TerminalListResource` |
| `GET` | `/api/v1/admin/terminals/{terminal}` | — | `TerminalResource` |
| `PUT` | `/api/v1/admin/terminals/{terminal}/name` | `RenameTerminalRequest` | `MutationResponse<TerminalResource>` |
| `PUT` | `/api/v1/admin/terminals/{terminal}/description` | `SetTerminalDescriptionRequest` | `MutationResponse<TerminalResource>` |
| `PATCH` | `/api/v1/admin/terminals/{terminal}/tags` | `UpdateTerminalTagsRequest` | `MutationResponse<TerminalResource>` |
| `DELETE` | `/api/v1/admin/terminals/{terminal}` | — | `DeletionResponse<TerminalResource>` |
| `GET` | `/api/v1/admin/groups` | — | `GroupListResource` |
| `POST` | `/api/v1/admin/groups` | `CreateGroupRequest` | `GroupResource` with HTTP 201 |
| `GET` | `/api/v1/admin/groups/{group}` | — | `GroupResource` |
| `PUT` | `/api/v1/admin/groups/{group}/name` | `RenameGroupRequest` | `MutationResponse<GroupResource>` |
| `PATCH` | `/api/v1/admin/groups/{group}/members` | `UpdateGroupMembersRequest` | `MutationResponse<GroupResource>` |
| `DELETE` | `/api/v1/admin/groups/{group}` | — | `DeletionResponse<GroupResource>` |

Collection responses are sorted by stable ID. Set-valued DTO fields use their
canonical sorted JSON representation. A terminal's `presence` is absent when
offline; presence is ephemeral and excludes remote network addresses. The
durable definition fields remain available while offline. State responses are
snapshots: `captured_at`, legacy `BtsState` and terminal counts describe one
observation rather than several independently read values.

`AddonResource` contains the last manifest registered during the current Core
run, persistent Core-owned `enabled` policy and ephemeral `registered` status.
Core does not persist manifests or registration: an addon host must register
again after Core restarts. Disabled policy is stored atomically alongside the
terminal registry as `addons.json` and is applied when that ID registers again.
Disabling removes the addon's actions and menu entries from active routing but
does not start, stop, install or remove its host. `/api/v1/addons` continues to
return only enabled, registered manifests for Telephony.

`TerminalResource::presentation` is the current accepted semantic presentation
for that terminal when one exists. It remains available while a terminal is
offline and is removed when the terminal definition is forgotten.

## References

`{terminal}`, `{group}`, `{addon}` and terminal values in
`UpdateGroupMembersRequest` are bounded raw references, percent-encoded by the
SDK when placed in a path. Core resolves
them using one deterministic rule:

1. an exact stable ID match wins;
2. otherwise, an exact case-sensitive display-name match is allowed only when
   it identifies exactly one resource;
3. zero name matches is `not_found`; more than one is
   `ambiguous_reference` with candidates sorted by stable ID.

Names need not be unique and renaming does not change an ID. Automation should
therefore use IDs. Core performs resolution immediately before the operation;
the CLI and SDK never read `terminals.json` or implement their own name lookup.

## Mutation and safety semantics

Name and description `PUT`s are idempotent. Adding an existing tag or group
member, or removing an absent tag or member succeeds with HTTP 200. `changed`
is false when the resulting resource was already in the requested state. Tag
and membership PATCH bodies have sorted `add` and `remove` sets. A value in both
sets is invalid input. The complete request is validated and resolved before an
atomic mutation; one invalid or ambiguous value changes nothing.

Creating an existing group ID is a conflict, even if the supplied name is the
same. Deleting a missing terminal or group is not found. Forgetting an online
terminal is always HTTP 409 with category `conflict` and code
`terminal_online`; v1 has no force query or request field. Confirmation in the
CLI cannot override this Core rule. Deleting a group removes memberships but
never deletes terminals.

Addon enable and disable `PUT`s are idempotent. Enabling an offline addon
changes policy but cannot make it registered; execution remains the addon's
host responsibility. Enabling a registered addon is rejected if its actions or
menu digits conflict with another active addon.

## Errors

All administrative failures use `AdministrativeErrorResponse`. `category` is
stable for program logic; `code` supplies a more specific machine-readable
reason; `message` is British-English operator context and must not be parsed.
Reference failures may include `resource`, `reference` and sorted `candidates`.

| HTTP | Category | Meaning |
| --- | --- | --- |
| 400 | `invalid_input` | Invalid request syntax or validated field |
| 404 | `not_found` | No ID or unambiguous name match |
| 409 | `ambiguous_reference` | More than one exact name match |
| 409 | `conflict` | Current resource state prevents the operation |
| 422 | `rejected` | Well-formed mutation rejected by a domain rule |
| 426 | `incompatible_api` | Requested administrative API is unsupported |
| 500 | `server_failure` | Core could not complete an otherwise valid operation |

An unavailable Core, DNS or connection failure, client timeout and malformed
success/error body are `bts-sdk` errors because no valid server envelope was
received.

The initial stable codes are `invalid_request`, `terminal_not_found`,
`group_not_found`, `addon_not_found`, `ambiguous_terminal_reference`,
`ambiguous_group_reference`, `ambiguous_addon_reference`, `terminal_online`,
`group_already_exists`, `mutation_rejected`, `unsupported_administrative_api`
and `internal`. Codes are
open validated identifiers: later v1 additions do not require a new category,
and SDK consumers must retain an unfamiliar code within its known category.
