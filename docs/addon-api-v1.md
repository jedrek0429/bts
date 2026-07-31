# Addon API v1 author guide

Addon API v1 is the internal contract for statically linked BTS services. An addon implements `bts_addons::Addon`, declares everything it may do in one `bts_protocol::AddonManifest`, and communicates only with `bts-core` through `AddonContext`. It must not import or contact `bts-display`, `bts-telephony`, Asterisk, egui or another addon's internals.

## Protocol boundaries

The shared protocol is split by responsibility:

- `bts_protocol::addons` contains addon IDs, manifests, generic action IDs, menu entries and capabilities.
- `bts_protocol::display` contains declarative screens and opaque display leases.
- `bts_protocol::telephony` contains generic voice-input request and result types.
- `bts_protocol::events` contains event envelopes and the Core event stream.
- `bts_protocol::state` contains retained Core state.

Provider clients and render implementations do not belong in `bts-protocol`.

API v1 intentionally changes the pre-v1 internal wire format: the closed `Action` enum is replaced by `ActionRequest` with an opaque ID, and `DisplaySet` is replaced by lease-bearing `DisplayRequested` commands. All repository services migrate together in this pull request; compatibility with independently deployed pre-v1 processes is not provided.

## Manifest

Every addon returns one manifest from `Addon::manifest`. IDs and action IDs must be stable. Action IDs should be namespaced, such as `example.notice.show`. Each action may have a telephone menu entry containing a single ASCII digit, an Asterisk media URI and a deterministic order. Core rejects unsupported API versions, duplicate addon IDs, duplicate actions, duplicate digits, invalid menu entries and undeclared display use.

`capabilities` describes controlled facilities. `screens` lists every protocol screen the addon may show. An addon declaring `ScreenKind::Message`, for example, cannot publish a clock screen.

## Minimal addon

```rust
use anyhow::Result;
use async_trait::async_trait;
use bts_addons::{Addon, AddonContext};
use bts_protocol::{
    ActionId, ActionRegistration, AddonCapability, AddonId, AddonManifest,
    AddonVersion, DisplayState, Event, EventKind, MenuEntry, ScreenKind,
    ADDON_API_VERSION,
};

struct NoticeAddon;

#[async_trait]
impl Addon for NoticeAddon {
    fn manifest(&self) -> AddonManifest {
        AddonManifest {
            api_version: ADDON_API_VERSION,
            id: AddonId::new("example-notice"),
            name: "Notice Service".into(),
            version: AddonVersion::new(1, 0, 0),
            actions: vec![ActionRegistration {
                id: ActionId::new("example.notice.show"),
                description: "Show the example notice".into(),
            }],
            menu: vec![MenuEntry {
                digit: '4',
                prompt: "sound:bts/press-4-notice".into(),
                action: ActionId::new("example.notice.show"),
                order: 40,
            }],
            capabilities: vec![AddonCapability::Display],
            screens: vec![ScreenKind::Message],
        }
    }

    async fn handle_event(&self, context: &AddonContext, event: &Event) -> Result<()> {
        let EventKind::ActionRequested { request } = &event.kind else {
            return Ok(());
        };
        if request.action.as_str() == "example.notice.show" {
            context.show(
                DisplayState::Message {
                    title: "Notice".into(),
                    body: "Example addon".into(),
                },
                10,
            ).await?;
        }
        Ok(())
    }
}
```

Add the implementation to the vector passed to `AddonRegistry::new`. Registration and dispatch after that point are generic; do not add a branch to the host.

## Actions and telephone menus

Actions carry an opaque `ActionId` and JSON parameters. Core resolves and validates the registered owner, while the addon host dispatches the request only to that owner. Telephony retrieves manifests from `GET /api/v1/addons`, orders entries by `order` and digit, plays their prompts, and publishes the selected generic action. It has no addon-specific digit mapping.

## Displays and ownership

`AddonContext::show` requests a new lease and returns its opaque `DisplayLeaseId`. Store that handle for a long-running screen. Use `update` with the same handle and `release` when finished. Core rejects updates from another addon or a stale lease. A higher numeric priority may replace a lower-priority screen; a lower priority cannot replace a higher one. Addon shutdown releases all its leases and unregisters it.

Display data is declarative. If an existing `DisplayState` variant is sufficient, reuse it. If a genuinely new visual contract is required, add a provider-independent variant and `ScreenKind` to `bts-protocol::display`, document its wire representation, add renderer support in `bts-display`, and test both serialization and rendering selection. Never pass arbitrary filesystem paths to a renderer. Declare `AddonCapability::Assets`, upload bytes with `AddonContext::upload_asset`, and place the returned opaque `AssetRef` in a protocol screen model that supports assets.

## Configuration, data and lifecycle

Configuration is read through `AddonContext::configuration`. Environment variables use `BTS_ADDON_<ID>_<KEY>`, with hyphens converted to underscores and the name upper-cased. `data_directory` returns the addon's isolated path under `BTS_ADDON_DATA_ROOT`; addons create only the files they own there.

Use `start` to initialise resources, `handle_event` for requests and `stop` to cancel tasks. The host attributes errors to the manifest ID and continues invoking unrelated addons. Background display updates should stop when Core rejects the lease as stale.

## Deliberate exclusions

API v1 does not provide dynamic libraries, third-party package installation, process sandboxing, arbitrary renderer code, direct container access or a Spotify integration. Capability declarations are validated at Core API boundaries; operating-system sandboxing remains the responsibility of deployment units.
