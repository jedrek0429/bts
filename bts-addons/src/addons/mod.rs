pub(crate) mod clock;
pub(crate) mod message;
pub(crate) mod weather;

pub(crate) fn requested_digit(event: &bts_protocol::Event) -> Option<&str> {
    match &event.kind {
        bts_protocol::EventKind::PhoneDtmfReceived { digit, .. } => Some(digit.as_str()),
        _ => None,
    }
}
