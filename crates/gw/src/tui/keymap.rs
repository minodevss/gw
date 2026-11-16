use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Accounts,
    Keys,
    Models,
    Log,
}

impl Screen {
    pub(crate) const ALL: [Self; 4] = [Self::Accounts, Self::Keys, Self::Models, Self::Log];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Accounts => "Accounts",
            Self::Keys => "Keys",
            Self::Models => "Models",
            Self::Log => "Log",
        }
    }

    pub(crate) fn number(self) -> u8 {
        match self {
            Self::Accounts => 1,
            Self::Keys => 2,
            Self::Models => 3,
            Self::Log => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    Goto(Screen),
    Down,
    Up,
    Top,
    Bottom,
    Enter,
    Back,
    Filter,
    New,
    Login,
    Remove,
    Copy,
    Refresh,
    Help,
    Quit,
}

type Chord = (KeyCode, KeyModifiers);

struct Binding {
    command: Command,
    chords: &'static [Chord],
    keys: &'static str,
    label: &'static str,
    screens: &'static [Screen],
    footer: bool,
}

const NONE: KeyModifiers = KeyModifiers::NONE;
const EVERYWHERE: &[Screen] = &[Screen::Accounts, Screen::Keys, Screen::Models, Screen::Log];
const ACCOUNTS: &[Screen] = &[Screen::Accounts];
const KEYS: &[Screen] = &[Screen::Keys];
const LOG: &[Screen] = &[Screen::Log];
const FILTERABLE: &[Screen] = &[Screen::Accounts, Screen::Models, Screen::Log];
const NOT_LOG: &[Screen] = &[Screen::Accounts, Screen::Keys, Screen::Models];
const DETAILED: &[Screen] = &[Screen::Accounts, Screen::Models, Screen::Log];
const REFRESHABLE: &[Screen] = &[Screen::Accounts, Screen::Models];

const fn binding(
    command: Command,
    chords: &'static [Chord],
    keys: &'static str,
    label: &'static str,
    screens: &'static [Screen],
    footer: bool,
) -> Binding {
    Binding {
        command,
        chords,
        keys,
        label,
        screens,
        footer,
    }
}

const BINDINGS: &[Binding] = &[
    binding(
        Command::Goto(Screen::Accounts),
        &[(KeyCode::Char('1'), NONE)],
        "1-4",
        "screen",
        EVERYWHERE,
        false,
    ),
    binding(
        Command::Goto(Screen::Keys),
        &[(KeyCode::Char('2'), NONE)],
        "",
        "",
        EVERYWHERE,
        false,
    ),
    binding(
        Command::Goto(Screen::Models),
        &[(KeyCode::Char('3'), NONE)],
        "",
        "",
        EVERYWHERE,
        false,
    ),
    binding(
        Command::Goto(Screen::Log),
        &[(KeyCode::Char('4'), NONE)],
        "",
        "",
        EVERYWHERE,
        false,
    ),
    binding(
        Command::Down,
        &[(KeyCode::Char('j'), NONE), (KeyCode::Down, NONE)],
        "j/k",
        "move",
        EVERYWHERE,
        false,
    ),
    binding(
        Command::Up,
        &[(KeyCode::Char('k'), NONE), (KeyCode::Up, NONE)],
        "",
        "",
        EVERYWHERE,
        false,
    ),
    binding(
        Command::Top,
        &[(KeyCode::Char('g'), NONE), (KeyCode::Home, NONE)],
        "g",
        "top",
        EVERYWHERE,
        false,
    ),
    binding(
        Command::Bottom,
        &[(KeyCode::Char('G'), NONE), (KeyCode::End, NONE)],
        "G",
        "follow",
        LOG,
        true,
    ),
    binding(
        Command::Bottom,
        &[(KeyCode::Char('G'), NONE), (KeyCode::End, NONE)],
        "G",
        "bottom",
        NOT_LOG,
        false,
    ),
    binding(
        Command::New,
        &[(KeyCode::Char('n'), NONE)],
        "n",
        "add",
        ACCOUNTS,
        true,
    ),
    binding(
        Command::New,
        &[(KeyCode::Char('n'), NONE)],
        "n",
        "create",
        KEYS,
        true,
    ),
    binding(
        Command::Login,
        &[(KeyCode::Char('l'), NONE)],
        "l",
        "login",
        ACCOUNTS,
        true,
    ),
    binding(
        Command::Remove,
        &[(KeyCode::Char('d'), NONE)],
        "d",
        "remove",
        ACCOUNTS,
        true,
    ),
    binding(
        Command::Remove,
        &[(KeyCode::Char('d'), NONE)],
        "d",
        "revoke",
        KEYS,
        true,
    ),
    binding(
        Command::Refresh,
        &[(KeyCode::Char('R'), NONE)],
        "R",
        "refresh",
        REFRESHABLE,
        true,
    ),
    binding(
        Command::Filter,
        &[(KeyCode::Char('/'), NONE)],
        "/",
        "filter",
        FILTERABLE,
        true,
    ),
    binding(
        Command::Enter,
        &[(KeyCode::Enter, NONE)],
        "enter",
        "detail",
        DETAILED,
        true,
    ),
    binding(
        Command::Enter,
        &[(KeyCode::Enter, NONE)],
        "enter",
        "setup",
        KEYS,
        true,
    ),
    binding(
        Command::Copy,
        &[(KeyCode::Char('y'), NONE)],
        "y",
        "copy selected name (OSC 52)",
        EVERYWHERE,
        false,
    ),
    binding(
        Command::Back,
        &[(KeyCode::Esc, NONE)],
        "esc",
        "back",
        EVERYWHERE,
        false,
    ),
    binding(
        Command::Help,
        &[(KeyCode::Char('?'), NONE)],
        "?",
        "help",
        EVERYWHERE,
        true,
    ),
    binding(
        Command::Quit,
        &[
            (KeyCode::Char('q'), NONE),
            (KeyCode::Char('c'), KeyModifiers::CONTROL),
        ],
        "q",
        "quit",
        EVERYWHERE,
        true,
    ),
];

fn matches(chords: &[Chord], key: &KeyEvent) -> bool {
    let modifiers = if matches!(key.code, KeyCode::Char(_)) {
        key.modifiers - KeyModifiers::SHIFT
    } else {
        key.modifiers
    };
    chords
        .iter()
        .any(|(code, wanted)| *code == key.code && *wanted == modifiers)
}

pub(crate) fn lookup(screen: Screen, key: &KeyEvent) -> Option<Command> {
    BINDINGS
        .iter()
        .find(|b| b.screens.contains(&screen) && matches(b.chords, key))
        .map(|b| b.command)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Nav {
    Esc,
    Enter,
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    Other,
}

pub(crate) fn nav(key: &KeyEvent) -> Nav {
    let code = key.code;
    if code == KeyCode::Esc {
        Nav::Esc
    } else if code == KeyCode::Enter {
        Nav::Enter
    } else if code == KeyCode::Tab {
        Nav::Tab
    } else if code == KeyCode::BackTab {
        Nav::BackTab
    } else if code == KeyCode::Up {
        Nav::Up
    } else if code == KeyCode::Down {
        Nav::Down
    } else if code == KeyCode::Left {
        Nav::Left
    } else if code == KeyCode::Right {
        Nav::Right
    } else {
        Nav::Other
    }
}

pub(crate) fn list_nav(key: &KeyEvent) -> Nav {
    if key.code == KeyCode::Char('j') {
        Nav::Down
    } else if key.code == KeyCode::Char('k') {
        Nav::Up
    } else if matches!(key.code, KeyCode::Char(_)) {
        Nav::Other
    } else {
        nav(key)
    }
}

pub(crate) fn is_interrupt(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

pub(crate) fn footer(screen: Screen) -> Vec<(&'static str, &'static str)> {
    BINDINGS
        .iter()
        .filter(|b| b.footer && b.screens.contains(&screen))
        .map(|b| (b.keys, b.label))
        .collect()
}

pub(crate) fn footer_text(screen: Screen) -> String {
    hints_text(&footer(screen))
}

pub(crate) fn hints_text(hints: &[(&str, &str)]) -> String {
    hints
        .iter()
        .map(|(keys, label)| format!("{keys} {label}"))
        .collect::<Vec<_>>()
        .join("  ")
}

pub(crate) fn help(screen: Screen) -> Vec<(String, String)> {
    BINDINGS
        .iter()
        .filter(|b| b.screens.contains(&screen) && !b.label.is_empty())
        .map(|b| {
            let keys = if b.command == Command::Quit {
                "q, ctrl-c"
            } else {
                b.keys
            };
            (keys.to_owned(), b.label.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn accounts_footer_lists_the_account_keys() {
        assert_eq!(
            footer_text(Screen::Accounts),
            "n add  l login  d remove  R refresh  / filter  enter detail  ? help  q quit"
        );
    }

    #[test]
    fn keys_footer_names_create_and_revoke() {
        assert_eq!(
            footer_text(Screen::Keys),
            "n create  d revoke  enter setup  ? help  q quit"
        );
    }

    #[test]
    fn log_footer_offers_follow() {
        assert!(footer_text(Screen::Log).contains("G follow"));
    }

    #[test]
    fn digits_switch_screens() {
        assert_eq!(
            lookup(Screen::Log, &press(KeyCode::Char('3'))),
            Some(Command::Goto(Screen::Models))
        );
    }

    #[test]
    fn login_key_only_exists_on_accounts() {
        assert_eq!(
            lookup(Screen::Accounts, &press(KeyCode::Char('l'))),
            Some(Command::Login)
        );
        assert_eq!(lookup(Screen::Keys, &press(KeyCode::Char('l'))), None);
    }

    #[test]
    fn shifted_letters_match_their_chord() {
        let key = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(lookup(Screen::Log, &key), Some(Command::Bottom));
    }

    #[test]
    fn ctrl_c_quits() {
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(lookup(Screen::Keys, &key), Some(Command::Quit));
        assert!(is_interrupt(&key));
    }

    #[test]
    fn help_lists_every_labelled_binding_for_the_screen() {
        let lines = help(Screen::Keys);
        assert!(
            lines
                .iter()
                .any(|(keys, label)| keys == "d" && label == "revoke")
        );
        assert!(lines.iter().all(|(_, label)| !label.is_empty()));
    }
}
