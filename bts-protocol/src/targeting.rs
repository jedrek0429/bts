//! Unresolved terminal selectors and concrete routing results.

use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer, Serialize};

use crate::{GroupId, TerminalCapabilities, TerminalId};

crate::terminal::identifier!(TerminalTag, "terminal tag");

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetScope {
    /// Resolve only terminals which are online when routing occurs.
    #[default]
    Online,
    /// Resolve registered terminals whether or not they are currently online.
    Registered,
}

impl TargetScope {
    fn is_online(&self) -> bool {
        matches!(self, Self::Online)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagMatch {
    All,
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagQuery {
    #[serde(rename = "match")]
    pub match_kind: TagMatch,
    pub tags: BTreeSet<TerminalTag>,
}

impl TagQuery {
    pub fn new(
        match_kind: TagMatch,
        tags: impl IntoIterator<Item = TerminalTag>,
    ) -> Result<Self, TagQueryError> {
        let tags = tags.into_iter().collect::<BTreeSet<_>>();
        if tags.is_empty() {
            Err(TagQueryError)
        } else {
            Ok(Self { match_kind, tags })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagQueryError;

impl std::fmt::Display for TagQueryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a tag query must contain at least one tag")
    }
}

impl std::error::Error for TagQueryError {}

impl<'de> Deserialize<'de> for TagQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireTagQuery {
            #[serde(rename = "match")]
            match_kind: TagMatch,
            tags: BTreeSet<TerminalTag>,
        }

        let query = WireTagQuery::deserialize(deserializer)?;
        Self::new(query.match_kind, query.tags).map_err(serde::de::Error::custom)
    }
}

/// A selector which must be resolved by Core before dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum TerminalTarget {
    Terminal {
        id: TerminalId,
        #[serde(default, skip_serializing_if = "TargetScope::is_online")]
        scope: TargetScope,
    },
    Group {
        id: GroupId,
        #[serde(default, skip_serializing_if = "TargetScope::is_online")]
        scope: TargetScope,
    },
    Tags {
        query: TagQuery,
        #[serde(default, skip_serializing_if = "TargetScope::is_online")]
        scope: TargetScope,
    },
    All {
        #[serde(default, skip_serializing_if = "TargetScope::is_online")]
        scope: TargetScope,
    },
}

impl TerminalTarget {
    /// The default immediate-presentation target: every currently online terminal.
    pub const fn all() -> Self {
        Self::All {
            scope: TargetScope::Online,
        }
    }

    pub const fn scope(&self) -> TargetScope {
        match self {
            Self::Terminal { scope, .. }
            | Self::Group { scope, .. }
            | Self::Tags { scope, .. }
            | Self::All { scope } => *scope,
        }
    }
}

impl Default for TerminalTarget {
    fn default() -> Self {
        Self::all()
    }
}

/// The concrete, non-empty set selected by Core for an unresolved target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedTarget {
    pub requested: TerminalTarget,
    pub terminals: BTreeSet<TerminalId>,
}

impl ResolvedTarget {
    pub fn new(
        requested: TerminalTarget,
        terminals: impl IntoIterator<Item = TerminalId>,
    ) -> Result<Self, ResolvedTargetError> {
        let terminals = terminals.into_iter().collect::<BTreeSet<_>>();
        if terminals.is_empty() {
            Err(ResolvedTargetError)
        } else {
            Ok(Self {
                requested,
                terminals,
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedTargetError;

impl std::fmt::Display for ResolvedTargetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a resolved target must contain at least one terminal")
    }
}

impl std::error::Error for ResolvedTargetError {}

impl<'de> Deserialize<'de> for ResolvedTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireResolvedTarget {
            requested: TerminalTarget,
            terminals: BTreeSet<TerminalId>,
        }

        let target = WireResolvedTarget::deserialize(deserializer)?;
        Self::new(target.requested, target.terminals).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum RoutingError {
    NoMatches {
        target: TerminalTarget,
    },
    OfflineTerminals {
        terminals: BTreeSet<TerminalId>,
    },
    UnsupportedCapabilities {
        terminals: BTreeSet<TerminalId>,
        required: TerminalCapabilities,
    },
}
