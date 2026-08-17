use std::collections::HashMap;

use bts_protocol::addons::v2::{ActionId, ActionRequest};
use bts_protocol::{TelephonyTargetOption, TelephonyTargets, TerminalTarget};

const CONFIGURATION_PROMPT: &str =
    "speech:Configuration.,speech:Press one to change terminal.,speech:Press star to return.";
const NO_TERMINALS_PROMPT: &str =
    "speech:No terminals are online.,speech:Press zero for configuration.";
const SELECT_TARGET_PROMPT: &str = "speech:Select a terminal target.";
const TARGET_UNAVAILABLE_PROMPT: &str =
    "speech:The selected target is unavailable.,speech:Press zero for configuration.";
const INVALID_SELECTION_PROMPT: &str = "speech:That selection is not valid.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerIdentity {
    pub number: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionSettings;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuContext {
    NoTargets,
    MainMenu,
    Addon {
        action: ActionId,
    },
    Configuration,
    TargetSelection {
        choices: Vec<TargetChoice>,
        input: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetChoice {
    pub code: String,
    pub option: TelephonyTargetOption,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionOutcome {
    pub media: Vec<MediaItem>,
    pub action: Option<ActionRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaItem {
    Uri(String),
    Speech(String),
}

impl SessionOutcome {
    fn media(value: impl Into<String>) -> Self {
        Self {
            media: media_items(&value.into()),
            action: None,
        }
    }

    fn items(media: Vec<MediaItem>) -> Self {
        Self {
            media,
            action: None,
        }
    }

    fn action(request: ActionRequest) -> Self {
        Self {
            media: Vec::new(),
            action: Some(request),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TelephonySession {
    #[allow(dead_code)] // Retained for caller-scoped settings and auditing.
    pub caller: CallerIdentity,
    pub selected_target: Option<TerminalTarget>,
    pub current_context: MenuContext,
    pub return_stack: Vec<MenuContext>,
    #[allow(dead_code)] // Issue #32 establishes the settings slot before settings are added.
    pub settings: SessionSettings,
    main_menu_media: Vec<MediaItem>,
}

impl TelephonySession {
    pub fn new(
        caller: CallerIdentity,
        targets: &TelephonyTargets,
        main_menu_media: Vec<MediaItem>,
    ) -> (Self, SessionOutcome) {
        let mut session = Self {
            caller,
            selected_target: None,
            current_context: MenuContext::NoTargets,
            return_stack: Vec::new(),
            settings: SessionSettings,
            main_menu_media,
        };

        let outcome = match targets.terminals.as_slice() {
            [] => SessionOutcome::media(NO_TERMINALS_PROMPT),
            [only] => {
                session.selected_target = Some(only.target.clone());
                session.current_context = MenuContext::MainMenu;
                // Automatic selection is an implementation detail, not a caller action.
                SessionOutcome::items(session.current_prompt())
            }
            _ => {
                session.current_context = MenuContext::TargetSelection {
                    choices: target_choices(targets),
                    input: String::new(),
                };
                SessionOutcome::items(session.current_prompt())
            }
        };
        (session, outcome)
    }

    pub fn handle_dtmf(
        &mut self,
        digit: &str,
        fresh_targets: &TelephonyTargets,
        actions: &HashMap<String, ActionId>,
    ) -> SessionOutcome {
        match digit {
            "0" => return self.open_configuration(),
            "*" => return self.cancel_or_back(),
            "#" => return self.confirm(fresh_targets),
            _ => {}
        }

        match &mut self.current_context {
            MenuContext::Configuration => {
                if digit == "1" {
                    self.current_context = MenuContext::TargetSelection {
                        choices: target_choices(fresh_targets),
                        input: String::new(),
                    };
                    if fresh_targets.terminals.is_empty() {
                        SessionOutcome::items(prompt_then(
                            NO_TERMINALS_PROMPT,
                            media_items(CONFIGURATION_PROMPT),
                        ))
                    } else {
                        SessionOutcome::items(self.current_prompt())
                    }
                } else {
                    SessionOutcome::items(prompt_then(
                        INVALID_SELECTION_PROMPT,
                        media_items(CONFIGURATION_PROMPT),
                    ))
                }
            }
            MenuContext::TargetSelection { choices, input } => {
                if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() && digit != "0" {
                    let candidate = format!("{input}{digit}");
                    if choices
                        .iter()
                        .any(|choice| choice.code.starts_with(&candidate))
                    {
                        *input = candidate;
                        SessionOutcome {
                            media: Vec::new(),
                            action: None,
                        }
                    } else {
                        SessionOutcome::items(prompt_then(
                            INVALID_SELECTION_PROMPT,
                            self.current_prompt(),
                        ))
                    }
                } else {
                    SessionOutcome::items(prompt_then(
                        INVALID_SELECTION_PROMPT,
                        self.current_prompt(),
                    ))
                }
            }
            MenuContext::MainMenu | MenuContext::Addon { .. } => {
                self.invoke_action(digit, fresh_targets, actions)
            }
            MenuContext::NoTargets => SessionOutcome::media(NO_TERMINALS_PROMPT),
        }
    }

    fn open_configuration(&mut self) -> SessionOutcome {
        match self.current_context {
            MenuContext::Configuration => {}
            MenuContext::TargetSelection { .. } if !self.return_stack.is_empty() => {
                self.current_context = MenuContext::Configuration;
            }
            _ => {
                self.return_stack.push(self.current_context.clone());
                self.current_context = MenuContext::Configuration;
            }
        }
        SessionOutcome::media(CONFIGURATION_PROMPT)
    }

    fn cancel_or_back(&mut self) -> SessionOutcome {
        match self.current_context {
            MenuContext::Configuration | MenuContext::TargetSelection { .. } => {
                if let Some(previous) = self.return_stack.pop() {
                    self.current_context = previous;
                } else if self.selected_target.is_some() {
                    self.current_context = MenuContext::MainMenu;
                }
            }
            MenuContext::Addon { .. } => self.current_context = MenuContext::MainMenu,
            MenuContext::NoTargets | MenuContext::MainMenu => {}
        }
        SessionOutcome::items(self.current_prompt())
    }

    fn confirm(&mut self, fresh_targets: &TelephonyTargets) -> SessionOutcome {
        let MenuContext::TargetSelection { choices, input } = &self.current_context else {
            return SessionOutcome::items(self.current_prompt());
        };
        let selected = choices
            .iter()
            .find(|choice| choice.code == *input)
            .map(|choice| choice.option.clone());
        let Some(selected) = selected else {
            return SessionOutcome::items(prompt_then(
                INVALID_SELECTION_PROMPT,
                self.current_prompt(),
            ));
        };

        if !fresh_targets.contains(&selected.target) {
            self.current_context = MenuContext::TargetSelection {
                choices: target_choices(fresh_targets),
                input: String::new(),
            };
            return SessionOutcome::items(prompt_then(
                TARGET_UNAVAILABLE_PROMPT,
                self.current_prompt(),
            ));
        }

        self.selected_target = Some(selected.target.clone());
        self.current_context = match self.return_stack.pop() {
            Some(MenuContext::MainMenu) => MenuContext::MainMenu,
            Some(context @ MenuContext::Addon { .. }) => context,
            Some(
                MenuContext::NoTargets
                | MenuContext::Configuration
                | MenuContext::TargetSelection { .. },
            )
            | None => MenuContext::MainMenu,
        };
        SessionOutcome::items(self.selected_target_media(&selected))
    }

    fn invoke_action(
        &mut self,
        digit: &str,
        fresh_targets: &TelephonyTargets,
        actions: &HashMap<String, ActionId>,
    ) -> SessionOutcome {
        let Some(action) = actions.get(digit).cloned() else {
            return SessionOutcome::items(prompt_then(
                INVALID_SELECTION_PROMPT,
                self.current_prompt(),
            ));
        };
        let Some(target) = self.selected_target.clone() else {
            return SessionOutcome::media(NO_TERMINALS_PROMPT);
        };
        if !fresh_targets.contains(&target) {
            return SessionOutcome::media(TARGET_UNAVAILABLE_PROMPT);
        }
        self.current_context = MenuContext::Addon {
            action: action.clone(),
        };
        SessionOutcome::action(ActionRequest {
            action,
            parameters: serde_json::Value::Null,
            target: Some(target),
        })
    }

    fn selected_target_media(&self, option: &TelephonyTargetOption) -> Vec<MediaItem> {
        let mut media = vec![MediaItem::Speech(format!("{} selected.", option.name))];
        media.extend(self.current_prompt());
        media
    }

    fn current_prompt(&self) -> Vec<MediaItem> {
        match &self.current_context {
            MenuContext::NoTargets => media_items(NO_TERMINALS_PROMPT),
            MenuContext::MainMenu => self.main_menu_media.clone(),
            MenuContext::Addon { .. } => {
                vec![MediaItem::Speech(
                    "Returning to the previous service.".into(),
                )]
            }
            MenuContext::Configuration => media_items(CONFIGURATION_PROMPT),
            MenuContext::TargetSelection { choices, .. } => target_menu_media(choices),
        }
    }
}

fn target_choices(targets: &TelephonyTargets) -> Vec<TargetChoice> {
    targets
        .options()
        .cloned()
        .enumerate()
        .map(|(index, option)| TargetChoice {
            code: bijective_base_nine(index),
            option,
        })
        .collect()
}

fn bijective_base_nine(index: usize) -> String {
    let mut value = index + 1;
    let mut digits = Vec::new();
    while value != 0 {
        value -= 1;
        digits.push(char::from(b'1' + (value % 9) as u8));
        value /= 9;
    }
    digits.into_iter().rev().collect()
}

fn target_menu_media(choices: &[TargetChoice]) -> Vec<MediaItem> {
    let mut media = media_items(SELECT_TARGET_PROMPT);
    for choice in choices {
        media.push(MediaItem::Speech(format!(
            "Press {} for {}.",
            spoken_digits(&choice.code),
            choice.option.name
        )));
    }
    media.push(MediaItem::Speech("Press hash to confirm.".into()));
    media
}

fn spoken_digits(code: &str) -> String {
    code.chars()
        .filter_map(|digit| {
            Some(match digit {
                '0' => "zero",
                '1' => "one",
                '2' => "two",
                '3' => "three",
                '4' => "four",
                '5' => "five",
                '6' => "six",
                '7' => "seven",
                '8' => "eight",
                '9' => "nine",
                _ => return None,
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn media_items(value: &str) -> Vec<MediaItem> {
    value
        .split(',')
        .filter(|value| !value.is_empty())
        .map(|value| {
            value.strip_prefix("speech:").map_or_else(
                || MediaItem::Uri(value.to_owned()),
                |text| MediaItem::Speech(text.to_owned()),
            )
        })
        .collect()
}

fn prompt_then(prompt: &str, rest: Vec<MediaItem>) -> Vec<MediaItem> {
    let mut media = media_items(prompt);
    media.extend(rest);
    media
}

#[cfg(test)]
mod tests {
    use bts_protocol::{GroupId, TargetScope, TerminalId};

    use super::*;

    const MAIN: &str = "sound:bts/main";

    fn main_media() -> Vec<MediaItem> {
        media_items(MAIN)
    }

    fn terminal(id: &str, name: &str) -> TelephonyTargetOption {
        TelephonyTargetOption {
            target: TerminalTarget::Terminal {
                id: TerminalId::new(id).unwrap(),
                scope: TargetScope::Online,
            },
            name: name.to_owned(),
        }
    }

    fn group(id: &str, name: &str) -> TelephonyTargetOption {
        TelephonyTargetOption {
            target: TerminalTarget::Group {
                id: GroupId::new(id).unwrap(),
                scope: TargetScope::Online,
            },
            name: name.to_owned(),
        }
    }

    fn targets(terminals: Vec<TelephonyTargetOption>) -> TelephonyTargets {
        TelephonyTargets {
            all: (!terminals.is_empty()).then(|| TelephonyTargetOption {
                target: TerminalTarget::all(),
                name: "All available terminals".to_owned(),
            }),
            terminals,
            groups: Vec::new(),
        }
    }

    fn caller() -> CallerIdentity {
        CallerIdentity {
            number: Some("201".to_owned()),
            name: Some("Caller".to_owned()),
        }
    }

    fn actions() -> HashMap<String, ActionId> {
        HashMap::from([("2".to_owned(), ActionId::new("clock.show"))])
    }

    #[test]
    fn none_one_and_many_terminals_have_distinct_initial_states() {
        let (none, outcome) = TelephonySession::new(caller(), &targets(vec![]), main_media());
        assert_eq!(none.selected_target, None);
        assert_eq!(none.current_context, MenuContext::NoTargets);
        assert_eq!(outcome.media, media_items(NO_TERMINALS_PROMPT));

        let one_target = terminal("bedroom", "Bedroom");
        let (one, outcome) =
            TelephonySession::new(caller(), &targets(vec![one_target.clone()]), main_media());
        assert_eq!(one.selected_target, Some(one_target.target));
        assert_eq!(one.current_context, MenuContext::MainMenu);
        assert_eq!(outcome.media, vec![MediaItem::Uri(MAIN.into())]);

        let (many, outcome) = TelephonySession::new(
            caller(),
            &targets(vec![terminal("alpha", "Alpha"), terminal("bravo", "Bravo")]),
            main_media(),
        );
        assert!(many.selected_target.is_none());
        assert!(matches!(
            many.current_context,
            MenuContext::TargetSelection { .. }
        ));
        assert!(
            outcome
                .media
                .contains(&MediaItem::Speech("Press one for Alpha.".into()))
        );
        assert!(outcome.media.iter().all(|item| {
            !matches!(item, MediaItem::Uri(uri) if uri.starts_with("characters:"))
        }));
    }

    #[test]
    fn temporary_numbers_are_deterministic_and_never_use_zero() {
        let catalogue = TelephonyTargets {
            terminals: (1..=10)
                .map(|index| {
                    terminal(
                        &format!("terminal-{index:02}"),
                        &format!("Terminal {index}"),
                    )
                })
                .collect(),
            groups: vec![group("downstairs", "Downstairs")],
            all: Some(TelephonyTargetOption {
                target: TerminalTarget::all(),
                name: "All available terminals".to_owned(),
            }),
        };
        let choices = target_choices(&catalogue);
        assert_eq!(choices[0].code, "1");
        assert_eq!(choices[8].code, "9");
        assert_eq!(choices[9].code, "11");
        assert!(choices.iter().all(|choice| !choice.code.contains('0')));
    }

    #[test]
    fn changing_target_inside_addon_returns_without_dispatching_an_action() {
        let initial = targets(vec![terminal("alpha", "Alpha"), terminal("bravo", "Bravo")]);
        let (mut session, _) = TelephonySession::new(caller(), &initial, main_media());
        session.selected_target = Some(initial.terminals[0].target.clone());
        session.current_context = MenuContext::Addon {
            action: ActionId::new("weather.show"),
        };

        session.handle_dtmf("0", &initial, &actions());
        session.handle_dtmf("1", &initial, &actions());
        session.handle_dtmf("2", &initial, &actions());
        let outcome = session.handle_dtmf("#", &initial, &actions());

        assert_eq!(
            session.selected_target,
            Some(initial.terminals[1].target.clone())
        );
        assert_eq!(
            session.current_context,
            MenuContext::Addon {
                action: ActionId::new("weather.show")
            }
        );
        assert!(outcome.action.is_none());
    }

    #[test]
    fn cancel_restores_addon_and_old_target() {
        let catalogue = targets(vec![terminal("alpha", "Alpha"), terminal("bravo", "Bravo")]);
        let (mut session, _) = TelephonySession::new(caller(), &catalogue, main_media());
        let original = catalogue.terminals[0].target.clone();
        session.selected_target = Some(original.clone());
        session.current_context = MenuContext::Addon {
            action: ActionId::new("clock.show"),
        };
        session.handle_dtmf("0", &catalogue, &actions());
        session.handle_dtmf("1", &catalogue, &actions());
        let outcome = session.handle_dtmf("*", &catalogue, &actions());
        assert_eq!(session.selected_target, Some(original));
        assert!(matches!(session.current_context, MenuContext::Addon { .. }));
        assert!(outcome.action.is_none());
    }

    #[test]
    fn terminal_disconnect_before_confirm_refreshes_without_replacement() {
        let initial = targets(vec![terminal("alpha", "Alpha"), terminal("bravo", "Bravo")]);
        let fresh = targets(vec![initial.terminals[0].clone()]);
        let (mut session, _) = TelephonySession::new(caller(), &initial, main_media());
        session.handle_dtmf("2", &fresh, &actions());
        let outcome = session.handle_dtmf("#", &fresh, &actions());
        assert!(session.selected_target.is_none());
        assert_eq!(
            outcome.media.first(),
            media_items(TARGET_UNAVAILABLE_PROMPT).first()
        );
    }

    #[test]
    fn unavailable_selected_terminal_never_redirects_an_action() {
        let initial = targets(vec![terminal("alpha", "Alpha")]);
        let (mut session, _) = TelephonySession::new(caller(), &initial, main_media());
        let outcome = session.handle_dtmf("2", &targets(vec![]), &actions());
        assert!(outcome.action.is_none());
        assert_eq!(
            session.selected_target,
            Some(initial.terminals[0].target.clone())
        );
        assert_eq!(outcome.media, media_items(TARGET_UNAVAILABLE_PROMPT));
    }

    #[test]
    fn group_and_all_targets_flow_through_action_context() {
        let mut catalogue = targets(vec![terminal("alpha", "Alpha"), terminal("bravo", "Bravo")]);
        catalogue.groups.push(group("downstairs", "Downstairs"));
        let (mut session, _) = TelephonySession::new(caller(), &catalogue, main_media());

        for (code, expected) in [
            ("3", catalogue.groups[0].target.clone()),
            ("4", TerminalTarget::all()),
        ] {
            session.current_context = MenuContext::TargetSelection {
                choices: target_choices(&catalogue),
                input: String::new(),
            };
            session.handle_dtmf(code, &catalogue, &actions());
            session.handle_dtmf("#", &catalogue, &actions());
            let outcome = session.handle_dtmf("2", &catalogue, &actions());
            assert_eq!(outcome.action.unwrap().target, Some(expected));
        }
    }

    #[test]
    fn reserved_keys_are_interpreted_before_addon_digits() {
        let catalogue = targets(vec![terminal("alpha", "Alpha")]);
        let (mut session, _) = TelephonySession::new(caller(), &catalogue, main_media());
        let reserved = HashMap::from([
            ("0".to_owned(), ActionId::new("bad.zero")),
            ("*".to_owned(), ActionId::new("bad.star")),
            ("#".to_owned(), ActionId::new("bad.hash")),
        ]);
        assert!(
            session
                .handle_dtmf("0", &catalogue, &reserved)
                .action
                .is_none()
        );
        assert!(
            session
                .handle_dtmf("*", &catalogue, &reserved)
                .action
                .is_none()
        );
        assert!(
            session
                .handle_dtmf("#", &catalogue, &reserved)
                .action
                .is_none()
        );
    }

    #[test]
    fn target_selection_rejects_non_matching_prefix_digits() {
        let catalogue = targets(vec![terminal("alpha", "Alpha"), terminal("bravo", "Bravo")]);
        let (mut session, _) = TelephonySession::new(caller(), &catalogue, main_media());

        let outcome = session.handle_dtmf("9", &catalogue, &actions());
        assert_eq!(
            outcome.media.first(),
            media_items(INVALID_SELECTION_PROMPT).first()
        );
        assert_eq!(
            session.current_context,
            MenuContext::TargetSelection {
                choices: target_choices(&catalogue),
                input: String::new(),
            }
        );
    }

    #[test]
    fn explicit_target_selection_speaks_the_natural_name() {
        let catalogue = targets(vec![
            terminal("bedroom", "Yendreck's Bedroom"),
            terminal("kitchen", "Kitchen"),
        ]);
        let (mut session, _) = TelephonySession::new(caller(), &catalogue, main_media());
        session.handle_dtmf("1", &catalogue, &actions());
        let outcome = session.handle_dtmf("#", &catalogue, &actions());
        assert!(
            outcome
                .media
                .contains(&MediaItem::Speech("Yendreck's Bedroom selected.".into()))
        );
        assert!(outcome.media.iter().all(|item| {
            !matches!(item, MediaItem::Uri(uri) if uri.starts_with("characters:"))
        }));
    }
}
