//! Deterministic natural-language rendering for addon telephone menus.

use bts_protocol::{
    DtmfMenuKey,
    addons::v2::{ActionId, AddonManifest, MenuEntry, MenuSpeechStyle},
};

/// One addon menu choice after ordering and speech rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpokenMenuEntry {
    pub digit: DtmfMenuKey,
    pub action: ActionId,
    pub label: String,
    pub speech: String,
}

/// Render every manifest menu entry once, ordered by `order` and then digit.
///
/// The digit is deliberately rendered here rather than stored in an addon
/// media URI, so changing a mapping always changes the generated speech.
pub fn render_menu(manifests: impl IntoIterator<Item = AddonManifest>) -> Vec<SpokenMenuEntry> {
    let mut entries: Vec<MenuEntry> = manifests
        .into_iter()
        .flat_map(|manifest| manifest.menu)
        .collect();
    entries.sort_by_key(|entry| (entry.order, entry.digit));
    entries.into_iter().map(render_entry).collect()
}

fn render_entry(entry: MenuEntry) -> SpokenMenuEntry {
    let spoken_label = entry
        .spoken_label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| derived_spoken_label(&entry.label));
    let joiner = match entry.speech_style {
        MenuSpeechStyle::Choice => "for",
        MenuSpeechStyle::Instruction => "to",
    };
    let speech = format!(
        "Press {} {joiner} {}.",
        digit_name(entry.digit),
        spoken_label.trim_end_matches(['.', '!', '?'])
    );
    SpokenMenuEntry {
        digit: entry.digit,
        action: entry.action,
        label: entry.label,
        speech,
    }
}

fn derived_spoken_label(label: &str) -> String {
    let label = label.trim();
    let mut characters = label.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    first.to_lowercase().chain(characters).collect()
}

fn digit_name(digit: DtmfMenuKey) -> &'static str {
    match digit.digit() {
        '1' => "one",
        '2' => "two",
        '3' => "three",
        '4' => "four",
        '5' => "five",
        '6' => "six",
        '7' => "seven",
        '8' => "eight",
        '9' => "nine",
        _ => unreachable!("DtmfMenuKey excludes non-addon digits"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bts_protocol::addons::v2::{API_VERSION, ActionRegistration, AddonId, AddonVersion};

    fn manifest(
        id: &str,
        digit: char,
        order: u16,
        label: &str,
        spoken_label: Option<&str>,
        speech_style: MenuSpeechStyle,
    ) -> AddonManifest {
        let action = ActionId::new(format!("{id}.run"));
        AddonManifest {
            api_version: API_VERSION,
            id: AddonId::new(id),
            name: label.to_owned(),
            version: AddonVersion::new(1, 0, 0),
            actions: vec![ActionRegistration {
                id: action.clone(),
                description: format!("Run {label}"),
            }],
            menu: vec![MenuEntry {
                digit: DtmfMenuKey::new(digit).unwrap(),
                label: label.to_owned(),
                spoken_label: spoken_label.map(str::to_owned),
                speech_style,
                action,
                order,
            }],
            capabilities: vec![],
            screens: vec![],
        }
    }

    #[test]
    fn default_addon_menu_has_exact_deterministic_speech() {
        let menu = render_menu(vec![
            manifest(
                "message",
                '4',
                90,
                "Clear display",
                Some("clear the display"),
                MenuSpeechStyle::Instruction,
            ),
            manifest(
                "weather",
                '3',
                30,
                "Weather",
                Some("the weather"),
                MenuSpeechStyle::Choice,
            ),
            manifest(
                "clock",
                '2',
                20,
                "Clock",
                Some("the time"),
                MenuSpeechStyle::Choice,
            ),
        ]);

        assert_eq!(
            menu.iter()
                .map(|entry| entry.speech.as_str())
                .collect::<Vec<_>>(),
            [
                "Press two for the time.",
                "Press three for the weather.",
                "Press four to clear the display.",
            ]
        );
        assert_eq!(menu.len(), 3);
    }

    #[test]
    fn changed_digit_is_rendered_without_a_numbered_asset() {
        let menu = render_menu([manifest(
            "weather",
            '7',
            30,
            "Weather",
            Some("the weather"),
            MenuSpeechStyle::Choice,
        )]);

        assert_eq!(menu[0].speech, "Press seven for the weather.");
        assert_eq!(menu[0].digit.digit(), '7');
    }

    #[test]
    fn spoken_label_defaults_to_normalised_human_label() {
        let menu = render_menu([manifest(
            "radio",
            '5',
            50,
            "Radio news",
            None,
            MenuSpeechStyle::Choice,
        )]);

        assert_eq!(menu[0].speech, "Press five for radio news.");
    }
}
