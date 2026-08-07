# btscli v1 contract

The `bts-cli` crate implements the administrative grammar below.

Install it independently on an operator machine with:

```sh
sudo bts-install install custom --component cli
```

This installs `/usr/bin/btscli`. The command administers resources owned by
Core; it does not manage systemd, containers, host configuration, Asterisk,
Display processes or operating-system users. Use `bts-install` and the host's
service tools for those responsibilities.

```text
btscli [--core URL] [--output human|json] [--timeout DURATION]
       [--quiet | -v...] [--colour auto|always|never] COMMAND

btscli status
btscli state show

btscli terminal list
btscli terminal show TERMINAL
btscli terminal rename TERMINAL NAME
btscli terminal tag add TERMINAL TAG...
btscli terminal tag remove TERMINAL TAG...
btscli terminal forget TERMINAL

btscli group list
btscli group show GROUP
btscli group create ID --name NAME
btscli group rename GROUP NAME
btscli group add GROUP TERMINAL...
btscli group remove GROUP TERMINAL...
btscli group delete GROUP
```

Every variadic `TAG...` and `TERMINAL...` requires at least one value. Global
options are accepted before or after a subcommand. Named profiles, addon and
session commands, raw event submission and host administration are absent.

## Configuration precedence

The precedence is command-line option, environment variable, then default:

| Option | Environment | Default |
| --- | --- | --- |
| `--core` | `BTS_CORE_URL` | `http://127.0.0.1:3100` |
| `--output` | `BTSCLI_OUTPUT` | `human` |
| `--timeout` | `BTSCLI_TIMEOUT` | `10s` |
| `--colour` | `BTSCLI_COLOUR` | `auto` |

Duration values are positive integers followed by `ms`, `s` or `m`. Empty or
invalid environment values are configuration errors rather than silently
ignored. There is no configuration file or named connection profile in v1.
`NO_COLOR` changes the `auto` default to `never`; an explicit `--colour` or
`BTSCLI_COLOUR` value has higher precedence.

`-v` is repeatable and adds request diagnostics to stderr without changing
stdout; a second occurrence includes the selected Core origin and timeout.
`--quiet` conflicts with `-v`; it suppresses successful human output, not
warnings, prompts or errors. `--quiet` with JSON output is invalid because a
successful machine-readable document must never disappear.

## Output

Human output is concise, uses terminal/group terminology and may use tables.
Lists are ordered by stable ID. Times use the local timezone in human output.
JSON output writes exactly one administrative DTO as compact JSON followed by a
newline on stdout; field names, IDs and timestamps are unchanged from the API.
Errors write exactly one structured error document to stderr. Diagnostics and
prompts never enter stdout.

Local and transport error documents use `error.category`, `error.code` and
`error.message`. Structured Core errors preserve those same fields and any
resource, reference or candidate context supplied by Core. The initial local
codes are `invalid_usage`, `invalid_configuration`, `output_failure`,
`core_unavailable`, `core_timeout` and `malformed_response`; incompatibility uses
`unsupported_administrative_api`.

`--output json` always disables colour. Human `auto` colour is enabled only
when the stream receiving the text is a TTY and `NO_COLOR` is absent. `always`
and `never` override TTY detection for human output. ANSI sequences are never
included in JSON.

For example, a script can inspect registered terminals without parsing the
human table:

```sh
btscli --core http://core.example:3100 --output json terminal list
```

The result is the compact `TerminalListResource` document. A successful
idempotent mutation still exits zero and reports `"changed":false`. Scripts
should branch on exit code plus `error.category` and `error.code`, never on
English prose.

A human inspection is semantic rather than a JSON pretty-printer:

```text
$ btscli terminal show bedroom-display
Terminal: Bedroom (bedroom-display)
Status: online
Implementation: bts-display
Capabilities: render_text
Tags: private, upstairs
Groups: all-displays
```

Machine output retains the API field names and stable identifiers:

```json
{"terminals":[{"id":"bedroom-display","name":"Bedroom","implementation":"bts-display","approved_capabilities":["render_text"],"tags":["private","upstairs"],"groups":["all-displays"]}]}
```

## Confirmation

`terminal forget` and `group delete` are destructive. The CLI first resolves
the reference through the SDK, then prompts with the stable ID, name and impact.
A reply must be exactly `y` or `yes`, case-insensitively after trimming.
Anything else cancels without sending the mutation.

If stdin is not a TTY, output is JSON, or `--quiet` is active, a destructive
command without `--yes` fails before mutation. `--yes` skips only local
confirmation. It does not bypass ambiguity or Core's online-terminal forgetting
conflict. Rename, tag and membership operations do not prompt.

## Bedroom and Dining Room example

Stable IDs are preferred for every mutation; display names are convenient for
interactive reads only because names may be duplicated or changed.

```sh
btscli terminal rename bedroom-display Bedroom
btscli terminal tag add bedroom-display private upstairs
btscli terminal rename dining-display "Dining Room"
btscli terminal tag add dining-display downstairs

btscli group create all-displays --name "All displays"
btscli group add all-displays bedroom-display dining-display
btscli group show all-displays
```

Adding either terminal or tag again is safe and returns `changed: false`.
Deleting `all-displays` removes only the group; it does not forget either
terminal. Forgetting an offline terminal removes its durable definition and
memberships, and the same stable ID may register again later.

## Compatibility

`btscli --version` reports the packaged BTS component version. On each request,
the SDK reads Core's unversioned `/api` discovery document and requires an
administrative API version it supports before using the advertised base path.
A product-version difference is not guessed into an API path. Incompatible or
malformed discovery fails with exit code 4. Addon, terminal-runtime and
telephony protocol versions are separate contracts and are not negotiated by
the administrative CLI.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success, including idempotent `changed: false` |
| 2 | CLI usage, local configuration, invalid input or confirmation refusal |
| 3 | Core unavailable, transport failure or timeout |
| 4 | API incompatibility or malformed server response |
| 5 | Resource not found |
| 6 | Ambiguous reference |
| 7 | Conflict or rejected mutation |
| 8 | Core server failure |

Signals and operating-system launch failures retain conventional shell
behaviour outside this mapping. Error messages are not a scripting interface;
JSON category/code and the exit code are.
