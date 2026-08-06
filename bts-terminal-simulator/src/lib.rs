//! Reusable headless terminal implementation for development and integration tests.

use std::{env, error::Error, fmt, sync::mpsc::RecvTimeoutError, time::Duration};

use bts_protocol::{
    DisplayState, PresentationId, PresentationRejection, PresentationRejectionCode,
    TerminalCapabilities, TerminalCapability, TerminalId, TerminalImplementationId, TerminalName,
};
use bts_terminal::{
    ConnectionState, RuntimeDiagnostics, TerminalConfiguration, TerminalEvent, TerminalHandle,
    TerminalRuntime,
};
use semver::Version;

const DEFAULT_CORE_URL: &str = "ws://127.0.0.1:3100/api/v1/terminals/ws";

/// How the headless renderer completes a valid presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponsePolicy {
    Accept,
    Reject(PresentationRejection),
    Ignore,
}

impl ResponsePolicy {
    fn from_environment() -> anyhow::Result<Self> {
        Self::parse(
            &env::var("BTS_TERMINAL_SIMULATOR_RESPONSE").unwrap_or_else(|_| "accept".to_owned()),
        )
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "accept" => Ok(Self::Accept),
            "reject" => Ok(Self::Reject(PresentationRejection {
                code: PresentationRejectionCode::new(PresentationRejectionCode::BUSY)
                    .expect("the built-in rejection code is valid"),
                detail: Some("Headless terminal was configured to reject presentations".to_owned()),
            })),
            "ignore" => Ok(Self::Ignore),
            value => anyhow::bail!(
                "BTS_TERMINAL_SIMULATOR_RESPONSE must be accept, reject or ignore; received {value:?}"
            ),
        }
    }
}

/// Fully resolved simulator configuration.
#[derive(Debug, Clone)]
pub struct SimulatorConfiguration {
    pub terminal: TerminalConfiguration,
    pub response_policy: ResponsePolicy,
}

impl SimulatorConfiguration {
    pub fn new(terminal: TerminalConfiguration, response_policy: ResponsePolicy) -> Self {
        Self {
            terminal,
            response_policy,
        }
    }

    /// Loads the component-scoped environment used by `scripts/bts-dev`.
    pub fn from_environment() -> anyhow::Result<Self> {
        let terminal_id = env::var("BTS_TERMINAL_ID")
            .map_err(|_| anyhow::anyhow!("BTS_TERMINAL_ID is not set"))?;
        let terminal_name = env::var("BTS_TERMINAL_NAME")
            .map_err(|_| anyhow::anyhow!("BTS_TERMINAL_NAME is not set"))?;
        let capabilities = env::var("BTS_TERMINAL_CAPABILITIES")
            .unwrap_or_else(|_| TerminalCapability::RENDER_TEXT.to_owned())
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(TerminalCapability::new)
            .collect::<Result<Vec<_>, _>>()?;
        let terminal = TerminalConfiguration::new(
            env::var("BTS_CORE_WS_URL").unwrap_or_else(|_| DEFAULT_CORE_URL.to_owned()),
            TerminalId::new(terminal_id)?,
            TerminalName::new(terminal_name)?,
            TerminalImplementationId::new("bts-terminal-simulator")?,
            Version::parse(env!("CARGO_PKG_VERSION"))?,
            TerminalCapabilities::new(capabilities),
        )?
        .with_runtime_diagnostics(RuntimeDiagnostics::new([
            ("platform".to_owned(), env::consts::OS.to_owned()),
            ("runtime".to_owned(), "headless-simulator".to_owned()),
        ])?);
        Ok(Self::new(terminal, ResponsePolicy::from_environment()?))
    }
}

/// Observable result after the simulator handles one runtime event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimulatorEvent {
    ConnectionStateChanged(ConnectionState),
    RegistrationRejected(bts_protocol::RegistrationRejection),
    PresentationAccepted {
        presentation_id: PresentationId,
        display: DisplayState,
    },
    PresentationRejected {
        presentation_id: PresentationId,
        rejection: PresentationRejection,
    },
    PresentationIgnored {
        presentation_id: PresentationId,
    },
    Runtime(TerminalEvent),
}

/// Headless renderer which delegates every lifecycle concern to `bts-terminal`.
pub struct HeadlessTerminal {
    handle: Option<TerminalHandle>,
    response_policy: ResponsePolicy,
    current_presentation: Option<DisplayState>,
}

impl HeadlessTerminal {
    pub fn spawn(configuration: SimulatorConfiguration) -> Result<Self, SimulatorError> {
        let handle =
            TerminalRuntime::spawn(configuration.terminal).map_err(SimulatorError::Runtime)?;
        Ok(Self {
            handle: Some(handle),
            response_policy: configuration.response_policy,
            current_presentation: None,
        })
    }

    pub fn current_presentation(&self) -> Option<&DisplayState> {
        self.current_presentation.as_ref()
    }

    pub fn next_event_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<SimulatorEvent, SimulatorError> {
        let event = self
            .handle()
            .next_event_timeout(timeout)
            .map_err(SimulatorError::Receive)?;
        self.process(event)
    }

    pub fn shutdown(mut self, reason: Option<String>) -> Result<(), SimulatorError> {
        self.handle
            .take()
            .expect("a live simulator always owns its runtime")
            .shutdown(reason)
            .map_err(SimulatorError::Runtime)
    }

    fn process(&mut self, event: TerminalEvent) -> Result<SimulatorEvent, SimulatorError> {
        match event {
            TerminalEvent::ConnectionStateChanged(state) => {
                Ok(SimulatorEvent::ConnectionStateChanged(state))
            }
            TerminalEvent::RegistrationRejected(rejection) => {
                Ok(SimulatorEvent::RegistrationRejected(rejection))
            }
            TerminalEvent::PresentationReceived(work) => {
                let presentation_id = work.presentation().request.id;
                if !work.is_applicable() {
                    return Ok(SimulatorEvent::PresentationIgnored { presentation_id });
                }
                match &self.response_policy {
                    ResponsePolicy::Accept => {
                        let display = work.presentation().request.display.clone();
                        self.handle()
                            .accept_presentation(work.completion().clone())
                            .map_err(SimulatorError::Runtime)?;
                        self.current_presentation = Some(display.clone());
                        Ok(SimulatorEvent::PresentationAccepted {
                            presentation_id,
                            display,
                        })
                    }
                    ResponsePolicy::Reject(rejection) => {
                        self.handle()
                            .reject_presentation(work.completion().clone(), rejection.clone())
                            .map_err(SimulatorError::Runtime)?;
                        Ok(SimulatorEvent::PresentationRejected {
                            presentation_id,
                            rejection: rejection.clone(),
                        })
                    }
                    ResponsePolicy::Ignore => {
                        Ok(SimulatorEvent::PresentationIgnored { presentation_id })
                    }
                }
            }
            other => Ok(SimulatorEvent::Runtime(other)),
        }
    }

    fn handle(&self) -> &TerminalHandle {
        self.handle
            .as_ref()
            .expect("a live simulator always owns its runtime")
    }
}

#[derive(Debug)]
pub enum SimulatorError {
    Runtime(bts_terminal::HandleError),
    Receive(RecvTimeoutError),
}

impl fmt::Display for SimulatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(error) => error.fmt(formatter),
            Self::Receive(error) => error.fmt(formatter),
        }
    }
}

impl Error for SimulatorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Runtime(error) => Some(error),
            Self::Receive(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_policy_values_are_strict() {
        assert_eq!(
            ResponsePolicy::parse("accept").unwrap(),
            ResponsePolicy::Accept
        );
        assert!(matches!(
            ResponsePolicy::parse("reject").unwrap(),
            ResponsePolicy::Reject(_)
        ));
        assert_eq!(
            ResponsePolicy::parse("ignore").unwrap(),
            ResponsePolicy::Ignore
        );
        assert!(ResponsePolicy::parse("sometimes").is_err());
    }
}
