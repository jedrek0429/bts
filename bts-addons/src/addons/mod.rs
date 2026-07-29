pub(crate) mod clock;
pub(crate) mod message;
pub(crate) mod weather;

use anyhow::Result;
use bts_protocol::{Action, DisplayState, Event, EventKind};

use crate::AddonContext;

pub(crate) struct Addons {
    clock: clock::ClockAddon,
    weather: weather::WeatherAddon,
}

impl Addons {
    pub(crate) fn new() -> Self {
        Self {
            clock: clock::ClockAddon::new(),
            weather: weather::WeatherAddon::new(),
        }
    }

    pub(crate) async fn handle(&self, context: &AddonContext, event: &Event) -> Result<()> {
        match &event.kind {
            EventKind::PhoneDtmfReceived { digit, .. } => {
                if let Some(action) = action_for_digit(digit) {
                    context
                        .publish(EventKind::ActionRequested { action })
                        .await?;
                }

                Ok(())
            }
            EventKind::ActionRequested { action } => self.handle_action(context, action).await,
            _ => Ok(()),
        }
    }

    async fn handle_action(&self, context: &AddonContext, action: &Action) -> Result<()> {
        match action {
            Action::Clock => self.clock.show(context).await,
            Action::Weather => self.weather.show(context).await,
            Action::Message { title, body } => message::show(context, title, body).await,
            Action::Blank => {
                context
                    .publish(EventKind::DisplaySet {
                        display: DisplayState::Blank,
                    })
                    .await
            }
        }
    }
}

fn action_for_digit(digit: &str) -> Option<Action> {
    match digit {
        "2" => Some(Action::Clock),
        "3" => Some(Action::Weather),
        "0" => Some(Action::Blank),
        _ => None,
    }
}
