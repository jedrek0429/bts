use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    model::{Component, Role, resolve_install_selection},
    platform::Platform,
    state::InstallerState,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum Action {
    InstallPackage { package: String },
    Download { component: Component },
    Stage { component: Component },
    WriteConfiguration { component: Component },
    CreateAccount { account: String },
    StopService { unit: String },
    Activate { component: Component },
    EnableService { unit: String },
    StartService { unit: String },
    ReserveTty1,
    DisableService { unit: String },
    RemoveComponent { component: Component, purge: bool },
    RestoreTty1,
    SaveState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationPlan {
    pub role: Option<Role>,
    pub before: BTreeSet<Component>,
    pub after: BTreeSet<Component>,
    pub actions: Vec<Action>,
}

impl InstallationPlan {
    pub fn install(
        state: Option<&InstallerState>,
        role: Option<Role>,
        explicit: &[Component],
        platform: Platform,
        no_start: bool,
    ) -> Result<Self> {
        let (role, desired) = resolve_install_selection(role, explicit)?;
        let before = state
            .map(|value| value.installed_components.clone())
            .unwrap_or_default();
        reconcile(before, desired, role, platform, false, no_start)
    }

    pub fn add(
        state: &InstallerState,
        components: &[Component],
        platform: Platform,
        no_start: bool,
    ) -> Result<Self> {
        if components.is_empty() {
            bail!("At least one component is required.");
        }
        let before = state.installed_components.clone();
        let mut desired = before.clone();
        desired.extend(components.iter().copied());
        reconcile(
            before,
            desired,
            Some(Role::Custom),
            platform,
            false,
            no_start,
        )
    }

    pub fn remove(
        state: &InstallerState,
        components: &[Component],
        platform: Platform,
        purge: bool,
    ) -> Result<Self> {
        if components.is_empty() {
            bail!("At least one component is required.");
        }
        let before = state.installed_components.clone();
        let mut desired = before.clone();
        for component in components {
            desired.remove(component);
        }
        reconcile(before, desired, Some(Role::Custom), platform, purge, false)
    }
}

fn reconcile(
    before: BTreeSet<Component>,
    after: BTreeSet<Component>,
    role: Option<Role>,
    platform: Platform,
    purge: bool,
    no_start: bool,
) -> Result<InstallationPlan> {
    let added: Vec<_> = after.difference(&before).copied().collect();
    let removed: Vec<_> = before.difference(&after).copied().collect();
    let old_dependencies = dependency_references(&before);
    let new_dependencies = dependency_references(&after);
    let mut actions = Vec::new();

    for dependency in new_dependencies
        .keys()
        .filter(|value| !old_dependencies.contains_key(*value))
    {
        for package in platform.packages_for(dependency)? {
            actions.push(Action::InstallPackage {
                package: (*package).into(),
            });
        }
    }
    if added
        .iter()
        .any(|value| !matches!(value, Component::Display | Component::Cli))
    {
        actions.push(Action::CreateAccount {
            account: "bts".into(),
        });
    }
    if added.contains(&Component::Display) {
        actions.push(Action::CreateAccount {
            account: "bts-display".into(),
        });
        actions.push(Action::ReserveTty1);
    }
    for component in &added {
        actions.push(Action::Download {
            component: *component,
        });
        actions.push(Action::Stage {
            component: *component,
        });
        if component.config_name().is_some() {
            actions.push(Action::WriteConfiguration {
                component: *component,
            });
        }
        actions.push(Action::Activate {
            component: *component,
        });
        if let Some(unit) = component.unit() {
            actions.push(Action::EnableService { unit: unit.into() });
            if !no_start {
                actions.push(Action::StartService { unit: unit.into() });
            }
        }
    }
    for component in &removed {
        if let Some(unit) = component.unit() {
            actions.push(Action::StopService { unit: unit.into() });
            actions.push(Action::DisableService { unit: unit.into() });
        }
        actions.push(Action::RemoveComponent {
            component: *component,
            purge,
        });
        if *component == Component::Display {
            actions.push(Action::RestoreTty1);
        }
    }
    if !actions.is_empty() {
        actions.push(Action::SaveState);
    }
    Ok(InstallationPlan {
        role,
        before,
        after,
        actions,
    })
}

pub fn dependency_references(components: &BTreeSet<Component>) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for dependency in components
        .iter()
        .flat_map(|component| component.runtime_dependencies())
    {
        *counts.entry(*dependency).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Architecture;

    fn state(components: impl IntoIterator<Item = Component>) -> InstallerState {
        let mut state = InstallerState::new("0.3.0", Platform::Debian, Architecture::X86_64);
        state.installed_components.extend(components);
        state
    }

    #[test]
    fn dry_run_plan_is_deterministic_and_component_selective() {
        let plan =
            InstallationPlan::install(None, Some(Role::Display), &[], Platform::Debian, false)
                .unwrap();
        assert_eq!(plan.after, [Component::Display].into());
        assert!(
            plan.actions.iter().any(
                |value| matches!(value, Action::InstallPackage { package } if package == "cage")
            )
        );
        assert!(!format!("{plan:?}").contains("core.service"));
    }

    #[test]
    fn reconciliation_is_idempotent() {
        let state = state([Component::Display]);
        let plan = InstallationPlan::install(
            Some(&state),
            Some(Role::Display),
            &[],
            Platform::Debian,
            false,
        )
        .unwrap();
        assert!(plan.actions.is_empty());
    }

    #[test]
    fn cli_is_an_independent_service_less_component() {
        let plan = InstallationPlan::install(
            None,
            Some(Role::Custom),
            &[Component::Cli],
            Platform::Debian,
            false,
        )
        .unwrap();
        assert_eq!(plan.after, [Component::Cli].into());
        assert!(plan.actions.contains(&Action::Download {
            component: Component::Cli
        }));
        assert!(plan.actions.contains(&Action::Activate {
            component: Component::Cli
        }));
        assert!(!plan.actions.iter().any(|action| matches!(
            action,
            Action::CreateAccount { .. }
                | Action::WriteConfiguration { .. }
                | Action::EnableService { .. }
                | Action::StartService { .. }
        )));
    }

    #[test]
    fn removal_only_disables_selected_service_and_restores_tty() {
        let state = state([Component::Core, Component::Display]);
        let plan = InstallationPlan::remove(&state, &[Component::Display], Platform::Debian, false)
            .unwrap();
        assert!(plan.actions.contains(&Action::DisableService {
            unit: "bts-display.service".into()
        }));
        assert!(plan.actions.contains(&Action::RestoreTty1));
        assert!(!plan.actions.contains(&Action::DisableService {
            unit: "bts-core.service".into()
        }));
    }

    #[test]
    fn shared_dependency_references_are_counted() {
        let components = [Component::Core, Component::Display, Component::Addons].into();
        assert_eq!(dependency_references(&components).get("seatd"), Some(&1));
        assert_eq!(
            dependency_references(&components).get("ca-certificates"),
            Some(&3)
        );
    }
}
