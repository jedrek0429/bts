//! Renderer-neutral runtime for BTS terminal endpoints.
//!
//! `bts-terminal` owns the endpoint side of the terminal protocol: connection,
//! registration readiness, heartbeat, reconnect, recipient validation,
//! acknowledgements and graceful shutdown. Core remains authoritative for the
//! terminal registry, presence, routing and presentation state.
//!
//! The consumer boundary is deliberately channel based. A UI such as egui can
//! keep rendering on its main thread, poll [`TerminalHandle::try_next_event`],
//! verify and apply a [`TerminalEvent::PresentationReceived`] locally, and then call
//! [`TerminalHandle::accept_presentation`] or
//! [`TerminalHandle::reject_presentation`]. The runtime never calls rendering
//! code or owns a repaint mechanism.

mod config;
mod runtime;
mod transport;

pub use config::{ConfigurationError, ReconnectPolicy, RuntimeDiagnostics, TerminalConfiguration};
pub use runtime::{
    ConnectionState, HandleError, IgnoredCommandReason, IgnoredDispatchReason,
    PresentationCompletion, PresentationInvalidationReason, PresentationStatus, PresentationWork,
    TerminalCommand, TerminalEvent, TerminalHandle, TerminalRuntime,
};
