use std::time::Duration;

use bts_terminal_simulator::{HeadlessTerminal, SimulatorConfiguration, SimulatorEvent};
use serde_json::json;

fn main() -> anyhow::Result<()> {
    let configuration = SimulatorConfiguration::from_environment()?;
    let mut terminal = HeadlessTerminal::spawn(configuration)?;

    loop {
        match terminal.next_event_timeout(Duration::from_secs(60)) {
            Ok(SimulatorEvent::ConnectionStateChanged(state)) => {
                println!(
                    "{}",
                    json!({ "event": "connection_state", "state": format!("{state:?}") })
                );
            }
            Ok(SimulatorEvent::RegistrationRejected(rejection)) => {
                println!(
                    "{}",
                    json!({ "event": "registration_rejected", "rejection": format!("{rejection:?}") })
                );
                anyhow::bail!("Core rejected the headless terminal registration");
            }
            Ok(SimulatorEvent::PresentationAccepted {
                presentation_id,
                display,
            }) => println!(
                "{}",
                json!({
                    "event": "presentation_accepted",
                    "presentation_id": presentation_id.as_uuid(),
                    "display": display,
                })
            ),
            Ok(SimulatorEvent::PresentationRejected {
                presentation_id,
                rejection,
            }) => println!(
                "{}",
                json!({
                    "event": "presentation_rejected",
                    "presentation_id": presentation_id.as_uuid(),
                    "rejection": rejection,
                })
            ),
            Ok(SimulatorEvent::PresentationIgnored { presentation_id }) => println!(
                "{}",
                json!({
                    "event": "presentation_ignored",
                    "presentation_id": presentation_id.as_uuid(),
                })
            ),
            Ok(SimulatorEvent::Runtime(event)) => println!(
                "{}",
                json!({ "event": "runtime", "detail": format!("{event:?}") })
            ),
            Err(bts_terminal_simulator::SimulatorError::Receive(
                std::sync::mpsc::RecvTimeoutError::Timeout,
            )) => continue,
            Err(error) => return Err(error.into()),
        }
    }
}
