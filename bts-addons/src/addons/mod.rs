pub(crate) mod clock;
pub(crate) mod message;
pub(crate) mod weather;

use anyhow::Result;
use bts_protocol::Event;

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
        match requested_digit(event) {
            Some(clock::DIGIT) => self.clock.handle(context, event).await,
            Some(weather::DIGIT) => self.weather.handle(context, event).await,
            _ => Ok(()),
        }
    }
}

pub(crate) fn requested_digit(event: &Event) -> Option<&str> {
    match &event.kind {
        bts_protocol::EventKind::PhoneDtmfReceived { digit, .. } => Some(digit.as_str()),
        _ => None,
    }
}
