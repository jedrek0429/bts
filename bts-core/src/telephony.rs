//! Core-owned target discovery for telephony control sessions.

use bts_protocol::{
    TargetScope, TelephonyTargetOption, TelephonyTargets, TerminalCapability, TerminalId,
    TerminalTarget,
};

use crate::terminals::TerminalRoutingSnapshot;

/// Builds a deterministic menu catalogue from one atomic registry snapshot.
pub fn target_catalogue(snapshot: &TerminalRoutingSnapshot) -> TelephonyTargets {
    let terminals = snapshot
        .definitions
        .iter()
        .filter(|(terminal_id, _)| is_suitable(snapshot, terminal_id))
        .map(|(terminal_id, definition)| TelephonyTargetOption {
            target: TerminalTarget::Terminal {
                id: terminal_id.clone(),
                scope: TargetScope::Online,
            },
            name: definition.identity.name.as_str().to_owned(),
        })
        .collect::<Vec<_>>();

    let groups = snapshot
        .groups
        .iter()
        .filter(|(_, group)| {
            group
                .members
                .iter()
                .any(|terminal_id| is_suitable(snapshot, terminal_id))
        })
        .map(|(group_id, group)| TelephonyTargetOption {
            target: TerminalTarget::Group {
                id: group_id.clone(),
                scope: TargetScope::Online,
            },
            name: group.identity.name.as_str().to_owned(),
        })
        .collect::<Vec<_>>();

    let all = (!terminals.is_empty()).then(|| TelephonyTargetOption {
        target: TerminalTarget::all(),
        name: "All available terminals".to_owned(),
    });

    TelephonyTargets {
        terminals,
        groups,
        all,
    }
}

fn is_suitable(snapshot: &TerminalRoutingSnapshot, terminal_id: &TerminalId) -> bool {
    let render_text = TerminalCapability::new(TerminalCapability::RENDER_TEXT)
        .expect("the built-in capability identifier is valid");
    snapshot
        .definitions
        .get(terminal_id)
        .is_some_and(|definition| definition.approved_capabilities.contains(&render_text))
        && snapshot
            .presences
            .get(terminal_id)
            .is_some_and(|presence| presence.declared_capabilities.contains(&render_text))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bts_protocol::{
        GroupId, GroupIdentity, GroupName, ProtocolVersion, TerminalCapabilities,
        TerminalConnectionId, TerminalId, TerminalIdentity, TerminalImplementationId, TerminalName,
        TerminalRegistration,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::terminals::TerminalRegistry;

    fn register(registry: &TerminalRegistry, id: &str, name: &str) -> TerminalId {
        register_with_capabilities(
            registry,
            id,
            name,
            TerminalCapabilities::new([
                TerminalCapability::new(TerminalCapability::RENDER_TEXT).unwrap()
            ]),
        )
    }

    fn register_with_capabilities(
        registry: &TerminalRegistry,
        id: &str,
        name: &str,
        capabilities: TerminalCapabilities,
    ) -> TerminalId {
        let terminal_id = TerminalId::new(id).unwrap();
        registry
            .register(
                TerminalRegistration {
                    identity: TerminalIdentity {
                        id: terminal_id.clone(),
                        name: TerminalName::new(name).unwrap(),
                    },
                    implementation: TerminalImplementationId::new("test-terminal").unwrap(),
                    protocol_version: ProtocolVersion::CURRENT,
                    capabilities,
                },
                TerminalConnectionId::new(),
                None,
                std::time::Instant::now(),
            )
            .unwrap();
        terminal_id
    }

    fn registry() -> (TempDir, TerminalRegistry) {
        let directory = tempfile::tempdir().unwrap();
        let registry = TerminalRegistry::load(
            directory.path().join("terminals.json"),
            Duration::from_secs(90),
        )
        .unwrap();
        (directory, registry)
    }

    #[test]
    fn catalogue_is_sorted_and_excludes_offline_targets() {
        let (_directory, registry) = registry();
        let bravo = register(&registry, "bravo", "Bravo");
        let alpha = register(&registry, "alpha", "Alpha");
        let charlie = register(&registry, "charlie", "Charlie");
        let audio_only = register_with_capabilities(
            &registry,
            "audio-only",
            "Audio only",
            TerminalCapabilities::default(),
        );
        let charlie_presence = registry.presence(&charlie).unwrap();
        registry
            .disconnect(&charlie, charlie_presence.connection_id)
            .unwrap();

        registry
            .create_group(GroupIdentity {
                id: GroupId::new("online-group").unwrap(),
                name: GroupName::new("Online group").unwrap(),
            })
            .unwrap();
        registry
            .add_group_member(&GroupId::new("online-group").unwrap(), &bravo)
            .unwrap();
        registry
            .create_group(GroupIdentity {
                id: GroupId::new("offline-group").unwrap(),
                name: GroupName::new("Offline group").unwrap(),
            })
            .unwrap();
        registry
            .add_group_member(&GroupId::new("offline-group").unwrap(), &charlie)
            .unwrap();

        let catalogue = target_catalogue(&registry.routing_snapshot(std::time::Instant::now()));
        assert_eq!(
            catalogue
                .terminals
                .iter()
                .map(|option| option.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha", "Bravo"]
        );
        assert_eq!(
            catalogue
                .groups
                .iter()
                .map(|option| option.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Online group"]
        );
        assert!(catalogue.all.is_some());
        assert!(catalogue.contains(&TerminalTarget::Terminal {
            id: alpha,
            scope: TargetScope::Online,
        }));
        assert!(!catalogue.contains(&TerminalTarget::Terminal {
            id: charlie,
            scope: TargetScope::Online,
        }));
        assert!(!catalogue.contains(&TerminalTarget::Terminal {
            id: audio_only,
            scope: TargetScope::Online,
        }));
    }

    #[test]
    fn empty_catalogue_has_no_all_target() {
        let (_directory, registry) = registry();
        let catalogue = target_catalogue(&registry.routing_snapshot(std::time::Instant::now()));
        assert!(catalogue.terminals.is_empty());
        assert!(catalogue.groups.is_empty());
        assert!(catalogue.all.is_none());
    }
}
