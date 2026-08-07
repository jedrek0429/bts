use std::collections::HashMap;

use bts_protocol::addons::v1::{ActionId, ActionRequest};
use bts_protocol::{TelephonyTargetOption, TelephonyTargets, TerminalTarget};

const CONFIGURATION_PROMPT: &str =
    "sound:bts/configuration,sound:bts/press-1-change-terminal,sound:bts/press-star-return";
const NO_TERMINALS_PROMPT: &str = "sound:bts/no-terminals-online,sound:bts/press-0-configuration";
const SELECT_TARGET_PROMPT: &str = "sound:bts/select-terminal,sound:bts/press-hash-confirm";
const TARGET_SELECTED_PROMPT: &str = "sound:bts/target-selected";
const TARGET_UNAVAILABLE_PROMPT: &str =
    "sound:bts/target-unavailable,sound:bts/press-0-configuration";
const INVALID_SELECTION_PROMPT: &str = "sound:bts/invalid-selection";

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
    pub media: Option<String>,
    pub action: Option<ActionRequest>,
}

impl SessionOutcome {
    fn media(value: impl Into<String>) -> Self {
        Self {
            media: Some(value.into()),
            action: None,
        }
    }

    fn action(request: ActionRequest) -> Self {
        Self {
            media: None,
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
    main_menu_media: String,
}

impl TelephonySession {
    pub fn new(
        caller: CallerIdentity,
        targets: &TelephonyTargets,
        main_menu_media: String,
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
                SessionOutcome::media(session.selected_target_media(only))
            }
            _ => {
                session.current_context = MenuContext::TargetSelection {
                    choices: target_choices(targets),
                    input: String::new(),
                };
                SessionOutcome::media(session.current_prompt())
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
                        SessionOutcome::media(format!(
                            "{NO_TERMINALS_PROMPT},{CONFIGURATION_PROMPT}"
                        ))
                    } else {
                        SessionOutcome::media(self.current_prompt())
                    }
                } else {
                    SessionOutcome::media(format!(
                        "{INVALID_SELECTION_PROMPT},{CONFIGURATION_PROMPT}"
                    ))
                }
            }
            MenuContext::TargetSelection { input, .. } => {
                if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() && digit != "0" {
                    input.push_str(digit);
                    SessionOutcome {
                        media: None,
                        action: None,
                    }
                } else {
                    SessionOutcome::media(format!(
                        "{INVALID_SELECTION_PROMPT},{}",
                        self.current_prompt()
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
        SessionOutcome::media(self.current_prompt())
    }

    fn confirm(&mut self, fresh_targets: &TelephonyTargets) -> SessionOutcome {
        let MenuContext::TargetSelection { choices, input } = &self.current_context else {
            return SessionOutcome::media(self.current_prompt());
        };
        let selected = choices
            .iter()
            .find(|choice| choice.code == *input)
            .map(|choice| choice.option.clone());
        let Some(selected) = selected else {
            return SessionOutcome::media(format!(
                "{INVALID_SELECTION_PROMPT},{}",
                self.current_prompt()
            ));
        };

        if !fresh_targets.contains(&selected.target) {
            self.current_context = MenuContext::TargetSelection {
                choices: target_choices(fresh_targets),
                input: String::new(),
            };
            return SessionOutcome::media(format!(
                "{TARGET_UNAVAILABLE_PROMPT},{}",
                self.current_prompt()
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
        SessionOutcome::media(self.selected_target_media(&selected))
    }

    fn invoke_action(
        &mut self,
        digit: &str,
        fresh_targets: &TelephonyTargets,
        actions: &HashMap<String, ActionId>,
    ) -> SessionOutcome {
        let Some(action) = actions.get(digit).cloned() else {
            return SessionOutcome::media(format!(
                "{INVALID_SELECTION_PROMPT},{}",
                self.current_prompt()
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

    fn selected_target_media(&self, option: &TelephonyTargetOption) -> String {
        format!(
            "{TARGET_SELECTED_PROMPT},characters:{},{}",
            spoken_name(&option.name),
            self.current_prompt()
        )
    }

    fn current_prompt(&self) -> String {
        match &self.current_context {
            MenuContext::NoTargets => NO_TERMINALS_PROMPT.to_owned(),
            MenuContext::MainMenu => self.main_menu_media.clone(),
            MenuContext::Addon { .. } => "sound:bts/returned-to-addon".to_owned(),
            MenuContext::Configuration => CONFIGURATION_PROMPT.to_owned(),
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

fn target_menu_media(choices: &[TargetChoice]) -> String {
    let mut media = vec![SELECT_TARGET_PROMPT.to_owned()];
    for choice in choices {
        media.push(format!("digits:{}", choice.code));
        media.push(format!("characters:{}", spoken_name(&choice.option.name)));
    }
    media.join(",")
}

fn spoken_name(name: &str) -> String {
    let spoken = name
        .chars()
        .filter_map(|character| {
            if character.is_ascii_alphanumeric() {
                Some(character.to_ascii_lowercase())
            } else if character.is_whitespace() || matches!(character, '-' | '_' | '.') {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>();
    if spoken.is_empty() {
        "terminal".to_owned()
    } else {
        spoken
    }
}

#[cfg(test)]
mod tests {
    use bts_protocol::{GroupId, TargetScope, TerminalId};

    use super::*;

    const MAIN: &str = "sound:bts/main";

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
        let (none, outcome) = TelephonySession::new(caller(), &targets(vec![]), MAIN.to_owned());
        assert_eq!(none.selected_target, None);
        assert_eq!(none.current_context, MenuContext::NoTargets);
        assert_eq!(outcome.media.as_deref(), Some(NO_TERMINALS_PROMPT));

        let one_target = terminal("bedroom", "Bedroom");
        let (one, outcome) = TelephonySession::new(
            caller(),
            &targets(vec![one_target.clone()]),
            MAIN.to_owned(),
        );
        assert_eq!(one.selected_target, Some(one_target.target));
        assert_eq!(one.current_context, MenuContext::MainMenu);
        assert!(outcome.media.unwrap().contains("characters:bedroom"));

        let (many, outcome) = TelephonySession::new(
            caller(),
            &targets(vec![terminal("alpha", "Alpha"), terminal("bravo", "Bravo")]),
            MAIN.to_owned(),
        );
        assert!(many.selected_target.is_none());
        assert!(matches!(
            many.current_context,
            MenuContext::TargetSelection { .. }
        ));
        assert!(outcome.media.unwrap().contains("digits:1"));
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
        let (mut session, _) = TelephonySession::new(caller(), &initial, MAIN.to_owned());
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
        let (mut session, _) = TelephonySession::new(caller(), &catalogue, MAIN.to_owned());
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
        let (mut session, _) = TelephonySession::new(caller(), &initial, MAIN.to_owned());
        session.handle_dtmf("2", &fresh, &actions());
        let outcome = session.handle_dtmf("#", &fresh, &actions());
        assert!(session.selected_target.is_none());
        assert!(
            outcome
                .media
                .unwrap()
                .starts_with(TARGET_UNAVAILABLE_PROMPT)
        );
    }

    #[test]
    fn unavailable_selected_terminal_never_redirects_an_action() {
        let initial = targets(vec![terminal("alpha", "Alpha")]);
        let (mut session, _) = TelephonySession::new(caller(), &initial, MAIN.to_owned());
        let outcome = session.handle_dtmf("2", &targets(vec![]), &actions());
        assert!(outcome.action.is_none());
        assert_eq!(
            session.selected_target,
            Some(initial.terminals[0].target.clone())
        );
        assert_eq!(outcome.media.as_deref(), Some(TARGET_UNAVAILABLE_PROMPT));
    }

    #[test]
    fn group_and_all_targets_flow_through_action_context() {
        let mut catalogue = targets(vec![terminal("alpha", "Alpha"), terminal("bravo", "Bravo")]);
        catalogue.groups.push(group("downstairs", "Downstairs"));
        let (mut session, _) = TelephonySession::new(caller(), &catalogue, MAIN.to_owned());

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
        let (mut session, _) = TelephonySession::new(caller(), &catalogue, MAIN.to_owned());
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
}
