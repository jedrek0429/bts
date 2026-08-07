//! Typed Rust access to BTS Core's administrative HTTP API.
//!
//! The SDK is not a terminal runtime. Constructing or using [`CoreApi`] never
//! registers a terminal, sends heartbeats or participates in presentations.

mod configuration;
mod core;
mod error;

pub use configuration::{ConfigurationError, CoreApiConfiguration};
pub use core::{CoreApi, SdkMetadata};
pub use error::SdkError;

pub use bts_protocol::addons::v1::{
    ActionId, ActionRegistration, AddonCapability, AddonId, AddonManifest, AddonVersion, MenuEntry,
};
pub use bts_protocol::{
    AddonListResource, AddonReference, AddonResource, AdministrativeError,
    AdministrativeErrorCategory, ApiDiscovery, CoreOperationalStatus, CoreStateResource,
    CoreStatusResource, CreateGroupRequest, DeletionResponse, DisplayState, GroupId,
    GroupListResource, GroupName, GroupReference, GroupResource, MutationResponse,
    RenameGroupRequest, RenameTerminalRequest, ScreenKind, SetAddonEnabledRequest,
    SetTerminalDescriptionRequest, TerminalDescription, TerminalId, TerminalListResource,
    TerminalName, TerminalPresentationResource, TerminalReference, TerminalResource, TerminalTag,
    UpdateGroupMembersRequest, UpdateTerminalTagsRequest,
};
