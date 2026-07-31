use std::{collections::BTreeSet, fmt, str::FromStr};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Component {
    Core,
    Display,
    Telephony,
    Addons,
}

impl Component {
    pub const ALL: [Self; 4] = [Self::Core, Self::Display, Self::Telephony, Self::Addons];

    pub fn binary(self) -> &'static str {
        match self {
            Self::Core => "bts-core",
            Self::Display => "bts-display",
            Self::Telephony => "bts-telephony",
            Self::Addons => "bts-addons",
        }
    }

    pub fn unit(self) -> &'static str {
        match self {
            Self::Core => "bts-core.service",
            Self::Display => "bts-display.service",
            Self::Telephony => "bts-telephony.service",
            Self::Addons => "bts-addons.service",
        }
    }

    pub fn config_name(self) -> &'static str {
        match self {
            Self::Core => "core.env",
            Self::Display => "display.env",
            Self::Telephony => "telephony.env",
            Self::Addons => "addons.env",
        }
    }

    pub fn runtime_dependencies(self) -> &'static [&'static str] {
        match self {
            Self::Core => &["ca-certificates"],
            Self::Display => &["ca-certificates", "cage", "seatd", "font-cabin"],
            Self::Telephony => &["ca-certificates"],
            Self::Addons => &["ca-certificates"],
        }
    }
}

impl fmt::Display for Component {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Core => "core",
            Self::Display => "display",
            Self::Telephony => "telephony",
            Self::Addons => "addons",
        })
    }
}

impl FromStr for Component {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "core" => Ok(Self::Core),
            "display" => Ok(Self::Display),
            "telephony" => Ok(Self::Telephony),
            "addons" => Ok(Self::Addons),
            _ => bail!("Unknown component '{value}'. Expected core, display, telephony or addons."),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Full,
    Server,
    Display,
    Custom,
}

impl Role {
    pub fn components(self) -> BTreeSet<Component> {
        match self {
            Self::Full => Component::ALL.into_iter().collect(),
            Self::Server => [Component::Core, Component::Telephony, Component::Addons]
                .into_iter()
                .collect(),
            Self::Display => [Component::Display].into_iter().collect(),
            Self::Custom => BTreeSet::new(),
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Full => "full",
            Self::Server => "server",
            Self::Display => "display",
            Self::Custom => "custom",
        })
    }
}

impl FromStr for Role {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "full" => Ok(Self::Full),
            "server" => Ok(Self::Server),
            "display" => Ok(Self::Display),
            "custom" => Ok(Self::Custom),
            _ => bail!("Unknown role '{value}'. Expected full, server, display or custom."),
        }
    }
}

pub fn resolve_install_selection(
    role: Option<Role>,
    explicit: &[Component],
) -> Result<(Option<Role>, BTreeSet<Component>)> {
    if role.is_some_and(|value| value != Role::Custom) && !explicit.is_empty() {
        bail!(
            "A role cannot be combined with --component. Use role 'custom' with --component selections."
        );
    }
    match role {
        Some(Role::Custom) if explicit.is_empty() => {
            bail!("Role 'custom' requires at least one --component selection.")
        }
        Some(Role::Custom) => Ok((Some(Role::Custom), explicit.iter().copied().collect())),
        Some(value) => Ok((Some(value), value.components())),
        None if explicit.is_empty() => bail!("Choose a role or at least one --component."),
        None => Ok((Some(Role::Custom), explicit.iter().copied().collect())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_resolve_deterministically() {
        assert_eq!(Role::Full.components().len(), 4);
        assert_eq!(Role::Server.components().len(), 3);
        assert_eq!(Role::Display.components(), [Component::Display].into());
    }

    #[test]
    fn explicit_components_create_custom_role() {
        let selected = [Component::Core, Component::Telephony];
        let (role, components) = resolve_install_selection(None, &selected).unwrap();
        assert_eq!(role, Some(Role::Custom));
        assert_eq!(components, selected.into());
    }

    #[test]
    fn rejects_role_and_components() {
        assert!(resolve_install_selection(Some(Role::Server), &[Component::Core]).is_err());
        assert_eq!(
            resolve_install_selection(Some(Role::Custom), &[Component::Core])
                .unwrap()
                .1,
            [Component::Core].into()
        );
    }
}
