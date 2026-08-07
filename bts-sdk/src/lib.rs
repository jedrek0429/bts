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

pub use bts_protocol::{
    AdministrativeError, AdministrativeErrorCategory, ApiDiscovery, CoreOperationalStatus,
    CoreStateResource, CoreStatusResource, CreateGroupRequest, DeletionResponse, DisplayState,
    GroupId, GroupListResource, GroupName, GroupReference, GroupResource, MutationResponse,
    RenameGroupRequest, RenameTerminalRequest, SetTerminalDescriptionRequest, TerminalDescription,
    TerminalId, TerminalListResource, TerminalName, TerminalPresentationResource,
    TerminalReference, TerminalResource, TerminalTag, UpdateGroupMembersRequest,
    UpdateTerminalTagsRequest,
};
