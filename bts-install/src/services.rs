use std::{collections::BTreeSet, path::Path};

use anyhow::{Result, ensure};

use crate::{
    model::Component,
    system::{SystemAdapter, systemctl},
};

/// Runtime dependency order for BTS server services.
const SERVICE_ORDER: [Component; 4] = [
    Component::Core,
    Component::Addons,
    Component::Telephony,
    Component::Display,
];

/// Reconcile desired BTS services after component activation.
///
/// Changed active services are restarted so that they execute the activated
/// binary. Inactive desired services are started, while unchanged active
/// services are deliberately left alone.
pub fn reconcile<S: SystemAdapter>(
    system: &mut S,
    root: &Path,
    desired: &BTreeSet<Component>,
    changed: &BTreeSet<Component>,
    no_start: bool,
) -> Result<()> {
    if no_start || root != Path::new("/") {
        return Ok(());
    }

    for component in SERVICE_ORDER {
        if !desired.contains(&component) {
            continue;
        }
        let unit = component.unit().expect("service order contains services");
        let active = is_active(system, root, unit);
        match (changed.contains(&component), active) {
            (true, true) => systemctl(system, root, "restart", &[unit])?,
            (_, false) => systemctl(system, root, "start", &[unit])?,
            (false, true) => {}
        }
        ensure!(
            is_active(system, root, unit),
            "{component} service did not remain active after reconciliation."
        );
    }
    Ok(())
}

fn is_active<S: SystemAdapter>(system: &mut S, root: &Path, unit: &str) -> bool {
    let mut arguments = Vec::new();
    if root != Path::new("/") {
        arguments.push(format!("--root={}", root.display()));
    }
    arguments.extend(["is-active".into(), "--quiet".into(), unit.into()]);
    system.output("systemctl", &arguments).is_ok()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anyhow::{Result, bail};

    use super::*;

    #[derive(Default)]
    struct ServiceSystem {
        active: BTreeSet<String>,
        commands: Vec<Vec<String>>,
    }

    impl SystemAdapter for ServiceSystem {
        fn run(&mut self, program: &str, arguments: &[String]) -> Result<()> {
            assert_eq!(program, "systemctl");
            self.commands.push(arguments.to_vec());
            if let [verb, unit] = arguments
                && (verb == "start" || verb == "restart")
            {
                self.active.insert(unit.clone());
            }
            Ok(())
        }

        fn output(&mut self, program: &str, arguments: &[String]) -> Result<String> {
            assert_eq!(program, "systemctl");
            let unit = arguments.last().expect("systemctl unit");
            if self.active.contains(unit) {
                Ok("active".into())
            } else {
                bail!("inactive")
            }
        }

        fn exists(&self, _path: &Path) -> bool {
            false
        }
    }

    fn components(values: &[Component]) -> BTreeSet<Component> {
        values.iter().copied().collect()
    }

    #[test]
    fn replaced_binary_restarts_an_already_active_old_service() {
        let mut system = ServiceSystem {
            ..Default::default()
        };
        system.active.insert("bts-core.service".into());
        reconcile(
            &mut system,
            Path::new("/"),
            &components(&[Component::Core]),
            &components(&[Component::Core]),
            false,
        )
        .unwrap();
        assert_eq!(system.commands, vec![vec!["restart", "bts-core.service"]]);
    }

    #[test]
    fn idempotent_reconciliation_does_not_restart_healthy_services() {
        let mut system = ServiceSystem::default();
        system.active.insert("bts-core.service".into());
        reconcile(
            &mut system,
            Path::new("/"),
            &components(&[Component::Core]),
            &BTreeSet::new(),
            false,
        )
        .unwrap();
        assert!(system.commands.is_empty());
    }

    #[test]
    fn no_start_never_starts_or_restarts_services() {
        let mut system = ServiceSystem::default();
        system.active.insert("bts-core.service".into());
        reconcile(
            &mut system,
            Path::new("/"),
            &components(&[Component::Core, Component::Telephony]),
            &components(&[Component::Core, Component::Telephony]),
            true,
        )
        .unwrap();
        assert!(system.commands.is_empty());
    }

    #[test]
    fn changed_services_reconcile_in_dependency_order() {
        let mut system = ServiceSystem::default();
        let all = components(&[
            Component::Display,
            Component::Telephony,
            Component::Addons,
            Component::Core,
        ]);
        reconcile(&mut system, Path::new("/"), &all, &all, false).unwrap();
        assert_eq!(
            system.commands,
            ["core", "addons", "telephony", "display"]
                .map(|name| vec!["start".into(), format!("bts-{name}.service")])
        );
    }
}
