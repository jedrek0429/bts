use crate::AddonContext;

pub(crate) async fn show(
    context: &AddonContext,
    title: impl Into<String>,
    body: impl Into<String>,
) -> Result<()> {
    context
        .publish(EventKind::DisplaySet {
            display: DisplayState::Message {
                title: title.into(),
                body: body.into(),
            },
        })
        .await
}
