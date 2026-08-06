# BTS Rust SDK

`bts-sdk` is the reusable typed HTTP client for Core's administrative API. It
depends only on `bts-protocol` and general HTTP/data libraries; it does not
depend on `bts-core`, `bts-terminal`, display, telephony or addons. SDK use
cannot register a terminal, send a heartbeat or participate in presentation
routing.

## Configuration and use

Each `CoreApi` has explicit process-local configuration. The base URL must be
an HTTP or HTTPS origin without credentials, a path prefix, query or fragment.
The default request timeout is ten seconds and can be replaced with a non-zero
duration. No environment variable or process-global client is read by the
SDK; choosing configuration sources belongs to the caller and, later, the
CLI.

```rust
use std::time::Duration;
use bts_sdk::{CoreApi, CoreApiConfiguration};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let configuration = CoreApiConfiguration::new("http://127.0.0.1:8080/")?
    .with_request_timeout(Duration::from_secs(5))?;
let core = CoreApi::new(configuration)?;

let compatibility = core.discover().await?;
let status = core.status().await?;
let state = core.state().await?;
# Ok(())
# }
```

`discover` reads the unversioned `/api` document; `discover_compatible` also
validates it for use by this SDK. `status` and `state` perform that validation
before joining the advertised base path with the resource name. The SDK sends
its crate version and supported API version as request metadata. Its public
`SdkMetadata` exposes the same versions to embedding applications.

The resource-oriented `CoreApi` can grow terminal and group methods without
changing construction or transport policy. Those methods are deliberately not
present until their owning issues implement the matching Core resources.

## Errors

`SdkError` keeps policy out of the library while preserving distinctions a
caller needs:

- invalid local configuration;
- transport failure and request timeout;
- discovery incompatibility;
- malformed success or error content;
- structured invalid request, not found, ambiguous reference, conflict,
  rejection, incompatible-API response and server failure.

For structured Core failures, `administrative_error()` returns the original
wire error, including its stable category, open error code and reference
context. Output wording, colour, prompts and process exit codes belong to
`bts-cli`, not the SDK.
