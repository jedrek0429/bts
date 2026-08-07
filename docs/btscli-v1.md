# btscli v1 contract

The `bts-cli` crate implements the administrative grammar below.

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

## Confirmation

`terminal forget` and `group delete` are destructive. The CLI first resolves
the reference through the SDK, then prompts with the stable ID, name and impact.
A reply must be exactly `y` or `yes`, case-insensitively after trimming.
Anything else cancels without sending the mutation.

If stdin is not a TTY, output is JSON, or `--quiet` is active, a destructive
command without `--yes` fails before mutation. `--yes` skips only local
confirmation. It does not bypass ambiguity or Core's online-terminal forgetting
conflict. Rename, tag and membership operations do not prompt.

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
