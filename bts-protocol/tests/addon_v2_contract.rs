use bts_protocol::{
    DtmfMenuKey,
    addons::v2::{API_VERSION, ActionId, MenuEntry, MenuSpeechStyle},
};
use serde_json::json;

#[test]
fn semantic_menu_entry_replaces_asterisk_media_uris() {
    assert_eq!(API_VERSION, 2);
    assert!(
        serde_json::from_value::<MenuEntry>(json!({
            "digit": "4",
            "prompt": "sound:test",
            "action": "test.run",
            "order": 40
        }))
        .is_err()
    );

    let entry: MenuEntry = serde_json::from_value(json!({
        "digit": "4",
        "label": "Clear display",
        "spoken_label": "clear the display",
        "speech_style": "instruction",
        "action": "display.blank",
        "order": 90
    }))
    .unwrap();

    assert_eq!(entry.digit, DtmfMenuKey::new('4').unwrap());
    assert_eq!(entry.label, "Clear display");
    assert_eq!(entry.speech_style, MenuSpeechStyle::Instruction);
    assert_eq!(entry.action, ActionId::new("display.blank"));
}

#[test]
fn spoken_label_is_optional_and_defaults_to_choice_speech() {
    let entry: MenuEntry = serde_json::from_value(json!({
        "digit": "7",
        "label": "Weather",
        "action": "weather.show",
        "order": 30
    }))
    .unwrap();

    assert_eq!(entry.spoken_label, None);
    assert_eq!(entry.speech_style, MenuSpeechStyle::Choice);
}
