//! Human-oriented terminal output kept separate from machine-readable schemas.

use std::collections::BTreeSet;

use crate::{
    model::Component,
    plan::{Action, InstallationPlan},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    enabled: bool,
}

impl Palette {
    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub fn success(self, text: &str) -> String {
        self.paint("32", text)
    }

    pub fn warning(self, text: &str) -> String {
        self.paint("33", text)
    }

    pub fn error(self, text: &str) -> String {
        self.paint("31", text)
    }

    pub fn accent(self, text: &str) -> String {
        self.paint("36", text)
    }

    pub fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\u{1b}[{code}m{text}\u{1b}[0m")
        } else {
            text.to_owned()
        }
    }
}

pub fn human_plan(plan: &InstallationPlan, version: &str, palette: Palette) -> String {
    if plan.actions.is_empty() {
        return format!(
            "{}\nNo installation changes are required.",
            palette.accent(&format!("BTS {version}"))
        );
    }

    let mut output = palette.accent(&format!("BTS {version}"));
    if let Some(role) = plan.role {
        output.push_str(&format!("\nInstall role: {role}"));
    }
    output.push_str("\nComponents");
    if plan.after.is_empty() {
        output.push_str("\n  None");
    } else {
        for component in &plan.after {
            output.push_str(&format!("\n  {}", component_title(*component)));
        }
    }

    let changes = system_changes(plan);
    if !changes.is_empty() {
        output.push_str("\nSystem changes");
        for change in changes {
            output.push_str(&format!("\n  {change}"));
        }
    }
    output
}

fn system_changes(plan: &InstallationPlan) -> BTreeSet<&'static str> {
    plan.actions
        .iter()
        .filter_map(|action| match action {
            Action::InstallPackage { .. } => Some("Install required packages"),
            Action::CreateAccount { .. } => Some("Create BTS service accounts"),
            Action::ReserveTty1 => Some("Reserve tty1 for Display"),
            Action::EnableService { .. } | Action::StartService { .. } => {
                Some("Enable and reconcile BTS services")
            }
            Action::StopService { .. }
            | Action::DisableService { .. }
            | Action::RemoveComponent { .. }
            | Action::RestoreTty1 => Some("Remove selected BTS components"),
            Action::Download { .. }
            | Action::Stage { .. }
            | Action::WriteConfiguration { .. }
            | Action::Activate { .. }
            | Action::SaveState => None,
        })
        .collect()
}

fn component_title(component: Component) -> &'static str {
    match component {
        Component::Core => "Core",
        Component::Display => "Display",
        Component::Telephony => "Telephony",
        Component::Addons => "Addons",
        Component::Cli => "CLI",
    }
}

#[cfg(test)]
mod tests {
    use crate::{model::Role, platform::Platform};

    use super::*;

    #[test]
    fn ordinary_plan_is_compact_and_hides_internal_debug_actions() {
        let plan =
            InstallationPlan::install(None, Some(Role::Server), &[], Platform::Debian, false)
                .unwrap();
        let text = human_plan(&plan, "0.3.0-rc.3", Palette::new(false));
        assert!(text.contains("BTS 0.3.0-rc.3"));
        assert!(text.contains("Install role: server"));
        assert!(text.contains("  Core"));
        assert!(text.contains("Enable and reconcile BTS services"));
        assert!(!text.contains("InstallPackage"));
        assert!(!text.contains("Activate {"));
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn restrained_colour_is_opt_in() {
        assert_eq!(Palette::new(false).success("✓ Ready"), "✓ Ready");
        assert_eq!(
            Palette::new(true).success("✓ Ready"),
            "\u{1b}[32m✓ Ready\u{1b}[0m"
        );
    }
}
