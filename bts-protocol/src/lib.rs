//! Shared, implementation-independent BTS wire contracts.

pub mod addons;
pub mod assets;
pub mod core;
pub mod display;
pub mod events;
pub mod state;
pub mod telephony;

pub use assets::*;
pub use display::*;
pub use events::*;
pub use state::*;
pub use telephony::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::v1::*;

    #[test]
    fn addon_manifest_round_trips_through_json() {
        let manifest = AddonManifest {
            api_version: API_VERSION,
            id: AddonId::new("example"),
            name: "Example".into(),
            version: AddonVersion::new(1, 2, 3),
            actions: vec![ActionRegistration {
                id: ActionId::new("example.run"),
                description: "Run".into(),
            }],
            menu: vec![MenuEntry {
                digit: '4',
                prompt: "sound:example".into(),
                action: ActionId::new("example.run"),
                order: 40,
            }],
            capabilities: vec![AddonCapability::Display],
            screens: vec![ScreenKind::Message],
        };
        let json = serde_json::to_string(&manifest).unwrap();
        assert_eq!(
            serde_json::from_str::<AddonManifest>(&json).unwrap(),
            manifest
        );
    }

    #[test]
    fn display_command_round_trips_with_opaque_lease() {
        let command = DisplayCommand::Update {
            addon_id: AddonId::new("example"),
            lease_id: DisplayLeaseId::new(),
            display: DisplayState::Blank,
        };
        let json = serde_json::to_string(&command).unwrap();
        assert!(matches!(
            serde_json::from_str::<DisplayCommand>(&json).unwrap(),
            DisplayCommand::Update { .. }
        ));
    }
}
