# Addon API v2 author guide

Addon API v2 is available as the next BTS network contract at `bts_protocol::addons::v2`. It is introduced alongside v1 so producers and consumers can migrate in a separate, reviewable change. An addon implements `Addon`, declares everything it may do in one `AddonManifest`, and communicates only with `bts-core` through the transport-neutral `AddonContext` interface. It must not depend on `bts-addons`, import or contact `bts-display` or `bts-telephony`, use Asterisk or egui APIs, or access another component's filesystem.

`bts-addons` is one host implementation for the built-in addons. It does not own the API. A third-party host may implement `AddonContext` over the published Core HTTP and WebSocket endpoints and can run on any machine that can reach Core.

## Protocol boundaries

The shared protocol is split by responsibility:

- `bts_protocol::addons::v2` contains the complete versioned addon contract: traits, IDs, manifests, generic actions, semantic menu entries and capabilities.
- `bts_protocol::display` contains declarative screens and opaque display leases.
- `bts_protocol::telephony` contains generic voice-input request and result types.
- `bts_protocol::events` contains event envelopes and the Core event stream.
- `bts_protocol::state` contains retained Core state.

Provider clients and render implementations do not belong in `bts-protocol`.

API v2 intentionally replaces the v1 menu entry's Asterisk `prompt` URI with
`label`, optional `spoken_label`, and `speech_style`. This is a breaking schema
change: addons and hosts must upgrade together. It keeps media rendering inside
Telephony, so addons do not depend on Asterisk paths or encode a menu digit in a
static filename.

## Manifest

Every addon returns one manifest from `Addon::manifest`. IDs and action IDs must be stable. Action IDs should be namespaced, such as `example.notice.show`. Each action may have a telephone menu entry containing a single ASCII digit, a human label, an optional natural spoken label, a speech style and a deterministic order. Core rejects unsupported API versions, duplicate addon IDs, duplicate actions, duplicate digits, invalid menu entries and undeclared display use.

`capabilities` describes controlled facilities. `screens` lists every protocol screen the addon may show. An addon declaring `ScreenKind::Message`, for example, cannot publish a clock screen.

## Minimal addon

```rust
use anyhow::Result;
use async_trait::async_trait;
use bts_protocol::addons::v2::{
    API_VERSION, ActionId, ActionRegistration, Addon, AddonCapability,
    AddonContext, AddonId, AddonManifest, AddonVersion, MenuEntry,
    MenuSpeechStyle,
};
use bts_protocol::{
    DisplayState, DtmfMenuKey, Event, EventKind, ScreenKind,
};

struct NoticeAddon;

#[async_trait]
impl Addon for NoticeAddon {
    fn manifest(&self) -> AddonManifest {
        AddonManifest {
            api_version: API_VERSION,
            id: AddonId::new("example-notice"),
            name: "Notice Service".into(),
            version: AddonVersion::new(1, 0, 0),
            actions: vec![ActionRegistration {
                id: ActionId::new("example.notice.show"),
                description: "Show the example notice".into(),
            }],
            menu: vec![MenuEntry {
                digit: DtmfMenuKey::new('4').expect("4 is an addon DTMF key"),
                label: "Notice".into(),
                spoken_label: Some("show the notice".into()),
                speech_style: MenuSpeechStyle::Instruction,
                action: ActionId::new("example.notice.show"),
                order: 40,
            }],
            capabilities: vec![AddonCapability::Display],
            screens: vec![ScreenKind::Message],
        }
    }

    async fn handle_event(&self, context: &dyn AddonContext, event: &Event) -> Result<()> {
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

An in-process host such as `bts-addons` registers the implementation with its generic registry. A separately deployed addon host performs the equivalent registration and event dispatch over Core's published endpoints. Neither approach requires an addon-specific branch in Core, Display or Telephony.

## Actions and telephone menus

Actions carry an opaque `ActionId` and JSON parameters. During adoption, Core resolves and validates the registered owner, while the addon host dispatches the request only to that owner. Telephony will retrieve manifests from `GET /api/v1/addons`, order entries by `order` and digit, render the current digit and semantic speech data through the configured BTS voice, and publish the selected generic action. It has no addon-specific digit mapping. `MenuSpeechStyle::Choice` produces “Press three for the weather”; `Instruction` produces “Press four to clear the display”. When `spoken_label` is absent, Telephony derives it from `label`.

A telephony action carries its session's unresolved `TerminalTarget`. The addon
host binds that value to the invocation's `AddonContext`; addons read
`AddonContext::selected_target` rather than keeping a second current-terminal
setting. In the built-in HTTP context, `show` and `update` become explicit
targeted presentations when such a target is present. Cloned contexts retain
the same immutable invocation target for background updates. Untargeted
lifecycle and non-telephony contexts continue to use the existing lease path.
An invocation-scoped `release` is intentionally a no-op because an explicit
presentation has no restorable lease; it remains until a later presentation
replaces it.

Changing the telephony target emits no addon or presentation event. Only a
subsequent action can cause the scoped context to submit a presentation.

## Displays and ownership

`AddonContext::show` requests a new lease and returns its opaque `DisplayLeaseId`. Store that handle for a long-running screen. Use `update` with the same handle and `release` when finished. Core rejects updates from another addon or a stale lease. Lease ownership and priority are evaluated independently on every terminal. A higher numeric priority replaces a lower-priority screen on each affected terminal; updating a hidden lease changes what will later be restored. Releasing a visible lease restores the next terminal-local lease or blank, while release-all and addon shutdown remove only that addon's leases. An explicitly targeted presentation takes direct ownership of its accepted terminals and clears their legacy lease stacks, without changing unrelated terminals.

Display data is declarative. If an existing `DisplayState` variant is sufficient, reuse it. If a genuinely new visual contract is required, add a provider-independent variant and `ScreenKind` to `bts-protocol::display`, document its wire representation, add renderer support in `bts-display`, and test both serialization and rendering selection. Never pass arbitrary filesystem paths to a renderer. Declare `AddonCapability::Assets`, upload bytes with `AddonContext::upload_asset`, and place the returned opaque `AssetRef` in a protocol screen model that supports assets.

## Configuration, data and lifecycle

Configuration and persistent storage are host responsibilities, because local paths cannot be meaningful across machines. The built-in host supplies environment configuration using `BTS_ADDON_<ID>_<KEY>` and isolated local storage under `BTS_ADDON_DATA_ROOT`; these are `bts-addons` conveniences, not Addon API network guarantees. Portable addons receive configuration from their own deployment environment and exchange shared data only through published Core services such as assets.

Use `start` to initialise resources, `handle_event` for requests and `stop` to cancel tasks. The host attributes errors to the manifest ID and continues invoking unrelated addons. Background display updates should stop when Core rejects the lease as stale.

Runtime enablement is separate from hosting. `btscli addon disable` records
Core policy and removes that addon's actions and menus from active routing; it
does not stop its host process. Registration is ephemeral, while the enabled
policy survives Core restarts and is reapplied when the host registers again.
Installation, service supervision and future third-party packaging belong to
`bts-install` and the reusable addon host, not to the administrative API.

## Deliberate exclusions

API v2 does not provide dynamic libraries, third-party package installation, process sandboxing, arbitrary renderer code, direct container access or a Spotify integration. Those broader third-party distribution and split-renderer designs remain tracked separately in Milestone 5; they are not prerequisites for semantic telephone menus. Capability declarations are validated at Core API boundaries; operating-system sandboxing remains the responsibility of deployment units.
