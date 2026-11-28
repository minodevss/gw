use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::action::Command;
use super::view::View;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Context {
    Accounts,
    Activity,
    Models,
    Keys,
    Filter,
    Menu,
    Confirm,
    Help,
    Choice,
    Input,
    Done,
    Code,
    Waiting,
    Failed,
    OneTime,
    Busy,
    List,
    Find,
    Global,
    Detail,
}

impl Context {
    fn chain(self) -> &'static [Context] {
        match self {
            Self::Accounts => &[Self::List, Self::Accounts, Self::Find, Self::Global],
            Self::Activity => &[Self::List, Self::Activity, Self::Find, Self::Global],
            Self::Models => &[Self::List, Self::Models, Self::Find, Self::Global],
            Self::Keys => &[Self::List, Self::Keys, Self::Global],
            Self::Filter => &[Self::Filter],
            Self::Menu => &[Self::Menu],
            Self::Confirm => &[Self::Confirm],
            Self::Help => &[Self::Help],
            Self::Choice => &[Self::Choice],
            Self::Input => &[Self::Input],
            Self::Done => &[Self::Done],
            Self::Code => &[Self::Code],
            Self::Waiting => &[Self::Waiting],
            Self::Failed => &[Self::Failed],
            Self::OneTime => &[Self::OneTime],
            Self::Busy => &[],
            Self::List => &[Self::List],
            Self::Find => &[Self::Find],
            Self::Global => &[Self::Global],
            Self::Detail => &[Self::Detail, Self::Global],
        }
    }

    pub(crate) fn of(view: View) -> Self {
        match view {
            View::Accounts => Self::Accounts,
            View::Activity => Self::Activity,
            View::Models => Self::Models,
            View::Keys => Self::Keys,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    code: KeyCode,
    mods: KeyModifiers,
}

const fn ch(c: char) -> Key {
    Key {
        code: KeyCode::Char(c),
        mods: KeyModifiers::NONE,
    }
}

const fn code(code: KeyCode) -> Key {
    Key {
        code,
        mods: KeyModifiers::NONE,
    }
}

const fn ctrl(c: char) -> Key {
    Key {
        code: KeyCode::Char(c),
        mods: KeyModifiers::CONTROL,
    }
}

impl Key {
    fn matches(self, event: &KeyEvent) -> bool {
        let mods = if matches!(event.code, KeyCode::Char(_) | KeyCode::BackTab) {
            event.modifiers.difference(KeyModifiers::SHIFT)
        } else {
            event.modifiers
        };
        event.code == self.code && mods == self.mods
    }

    fn name(self) -> String {
        let base = match self.code {
            KeyCode::Char(' ') => "space".to_owned(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "enter".to_owned(),
            KeyCode::Esc => "esc".to_owned(),
            KeyCode::Tab => "tab".to_owned(),
            KeyCode::BackTab => "shift-tab".to_owned(),
            KeyCode::Up => "↑".to_owned(),
            KeyCode::Down => "↓".to_owned(),
            KeyCode::Left => "←".to_owned(),
            KeyCode::Right => "→".to_owned(),
            KeyCode::PageUp => "pgup".to_owned(),
            KeyCode::PageDown => "pgdn".to_owned(),
            KeyCode::Home => "home".to_owned(),
            KeyCode::End => "end".to_owned(),
            KeyCode::Backspace => "backspace".to_owned(),
            KeyCode::Delete => "delete".to_owned(),
            KeyCode::Insert => "insert".to_owned(),
            KeyCode::F(number) => format!("f{number}"),
            KeyCode::Null
            | KeyCode::CapsLock
            | KeyCode::ScrollLock
            | KeyCode::NumLock
            | KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::Menu
            | KeyCode::KeypadBegin
            | KeyCode::Media(_)
            | KeyCode::Modifier(_) => String::new(),
        };
        if self.mods.contains(KeyModifiers::CONTROL) {
            format!("ctrl-{base}")
        } else {
            base
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Binding {
    context: Context,
    keys: &'static [Key],
    command: Command,
    label: &'static str,
    footer: Option<(&'static str, &'static str)>,
}

const fn bind(
    context: Context,
    keys: &'static [Key],
    command: Command,
    label: &'static str,
    footer: Option<(&'static str, &'static str)>,
) -> Binding {
    Binding {
        context,
        keys,
        command,
        label,
        footer,
    }
}

const TABLE: &[Binding] = &[
    bind(
        Context::Detail,
        &[code(KeyCode::Left)],
        Command::Left,
        "back to the list",
        Some(("←", "back")),
    ),
    bind(
        Context::Detail,
        &[code(KeyCode::Enter)],
        Command::Select,
        "actions for this item",
        Some(("enter", "actions")),
    ),
    bind(
        Context::List,
        &[code(KeyCode::Up), ch('k')],
        Command::Up,
        "move up",
        None,
    ),
    bind(
        Context::List,
        &[code(KeyCode::Down), ch('j')],
        Command::Down,
        "move down",
        Some(("↑↓", "select")),
    ),
    bind(
        Context::List,
        &[code(KeyCode::PageUp)],
        Command::PageUp,
        "page up",
        None,
    ),
    bind(
        Context::List,
        &[code(KeyCode::PageDown)],
        Command::PageDown,
        "page down",
        None,
    ),
    bind(
        Context::List,
        &[code(KeyCode::Home), ch('g')],
        Command::Top,
        "first",
        None,
    ),
    bind(
        Context::List,
        &[code(KeyCode::End), ch('G')],
        Command::Bottom,
        "last",
        None,
    ),
    bind(
        Context::List,
        &[code(KeyCode::Enter)],
        Command::Select,
        "actions for the selection",
        Some(("enter", "actions")),
    ),
    bind(
        Context::List,
        &[code(KeyCode::Right)],
        Command::Right,
        "open details",
        None,
    ),
    bind(
        Context::List,
        &[code(KeyCode::Left)],
        Command::Left,
        "close details",
        None,
    ),
    bind(
        Context::List,
        &[code(KeyCode::Esc)],
        Command::Back,
        "clear the filter, close details",
        None,
    ),
    bind(
        Context::Find,
        &[ch('/')],
        Command::Filter,
        "filter",
        Some(("/", "filter")),
    ),
    bind(Context::Keys, &[ch('/')], Command::Filter, "filter", None),
    bind(
        Context::Accounts,
        &[ch('a')],
        Command::Add,
        "add an account",
        Some(("a", "add")),
    ),
    bind(
        Context::Accounts,
        &[ch('r')],
        Command::RefreshUsage,
        "refresh usage (asks the provider)",
        None,
    ),
    bind(
        Context::Accounts,
        &[ch('l')],
        Command::Relogin,
        "sign in again",
        None,
    ),
    bind(
        Context::Accounts,
        &[ch('e')],
        Command::Rename,
        "rename",
        None,
    ),
    bind(
        Context::Accounts,
        &[ch('d')],
        Command::Remove,
        "remove",
        None,
    ),
    bind(
        Context::Activity,
        &[code(KeyCode::End), ch('G')],
        Command::Bottom,
        "jump to the newest and follow",
        Some(("end", "follow")),
    ),
    bind(
        Context::Activity,
        &[ch('f')],
        Command::FilterAccount,
        "show only this account",
        None,
    ),
    bind(
        Context::Activity,
        &[ch('s')],
        Command::FilterSession,
        "show only this session",
        None,
    ),
    bind(
        Context::Activity,
        &[ch('c')],
        Command::Copy,
        "copy the details",
        None,
    ),
    bind(
        Context::Models,
        &[ch('r')],
        Command::RefreshModels,
        "refresh model lists (asks every provider)",
        Some(("r", "refresh")),
    ),
    bind(
        Context::Models,
        &[ch('c')],
        Command::Copy,
        "copy the model name",
        None,
    ),
    bind(
        Context::Keys,
        &[ch('a')],
        Command::Add,
        "create a key",
        Some(("a", "new key")),
    ),
    bind(
        Context::Keys,
        &[ch('d')],
        Command::Remove,
        "revoke the key",
        None,
    ),
    bind(
        Context::Keys,
        &[ch('c')],
        Command::Copy,
        "copy the client settings",
        Some(("c", "copy")),
    ),
    bind(
        Context::Global,
        &[code(KeyCode::Tab)],
        Command::NextView,
        "next screen",
        None,
    ),
    bind(
        Context::Global,
        &[code(KeyCode::BackTab)],
        Command::PrevView,
        "previous screen",
        None,
    ),
    bind(
        Context::Global,
        &[ch('1')],
        Command::Goto(View::Accounts),
        "Accounts",
        None,
    ),
    bind(
        Context::Global,
        &[ch('2')],
        Command::Goto(View::Activity),
        "Activity",
        None,
    ),
    bind(
        Context::Global,
        &[ch('3')],
        Command::Goto(View::Models),
        "Models",
        None,
    ),
    bind(
        Context::Global,
        &[ch('4')],
        Command::Goto(View::Keys),
        "Keys",
        None,
    ),
    bind(
        Context::Global,
        &[ch('?')],
        Command::Help,
        "this help",
        Some(("?", "help")),
    ),
    bind(
        Context::Global,
        &[ch('q'), ctrl('c')],
        Command::Quit,
        "quit",
        None,
    ),
    bind(
        Context::Filter,
        &[code(KeyCode::Enter)],
        Command::Select,
        "keep the filter",
        Some(("enter", "apply")),
    ),
    bind(
        Context::Filter,
        &[code(KeyCode::Esc)],
        Command::Back,
        "clear the filter",
        Some(("esc", "clear")),
    ),
    bind(
        Context::Menu,
        &[code(KeyCode::Up), ch('k')],
        Command::Up,
        "move up",
        None,
    ),
    bind(
        Context::Menu,
        &[code(KeyCode::Down), ch('j')],
        Command::Down,
        "move down",
        Some(("↑↓", "select")),
    ),
    bind(
        Context::Menu,
        &[code(KeyCode::Enter)],
        Command::Select,
        "run",
        Some(("enter", "run")),
    ),
    bind(
        Context::Menu,
        &[code(KeyCode::Esc)],
        Command::Back,
        "close",
        Some(("esc", "close")),
    ),
    bind(
        Context::Confirm,
        &[code(KeyCode::Left), ch('h')],
        Command::Left,
        "choose Cancel",
        Some(("←→", "choose")),
    ),
    bind(
        Context::Confirm,
        &[code(KeyCode::Right), ch('l')],
        Command::Right,
        "choose the action",
        None,
    ),
    bind(
        Context::Confirm,
        &[code(KeyCode::Enter)],
        Command::Select,
        "confirm",
        Some(("enter", "confirm")),
    ),
    bind(
        Context::Confirm,
        &[code(KeyCode::Esc)],
        Command::Back,
        "cancel",
        Some(("esc", "cancel")),
    ),
    bind(
        Context::Help,
        &[code(KeyCode::Esc), code(KeyCode::Enter), ch('?'), ch('q')],
        Command::Back,
        "close",
        Some(("esc", "close")),
    ),
    bind(
        Context::Choice,
        &[code(KeyCode::Up), ch('k')],
        Command::Up,
        "move up",
        None,
    ),
    bind(
        Context::Choice,
        &[code(KeyCode::Down), ch('j')],
        Command::Down,
        "move down",
        None,
    ),
    bind(
        Context::Choice,
        &[code(KeyCode::Enter)],
        Command::Select,
        "next",
        Some(("enter", "next")),
    ),
    bind(
        Context::Choice,
        &[code(KeyCode::Esc)],
        Command::Back,
        "back",
        Some(("esc", "back")),
    ),
    bind(
        Context::Input,
        &[code(KeyCode::Enter)],
        Command::Select,
        "next",
        Some(("enter", "next")),
    ),
    bind(
        Context::Input,
        &[code(KeyCode::Esc)],
        Command::Back,
        "back",
        Some(("esc", "back")),
    ),
    bind(
        Context::Done,
        &[code(KeyCode::Enter)],
        Command::Select,
        "done",
        Some(("enter", "done")),
    ),
    bind(
        Context::Done,
        &[code(KeyCode::Esc)],
        Command::Back,
        "cancel",
        Some(("esc", "cancel")),
    ),
    bind(
        Context::Code,
        &[code(KeyCode::Enter)],
        Command::Select,
        "continue",
        Some(("enter", "continue")),
    ),
    bind(
        Context::Code,
        &[ctrl('y')],
        Command::Copy,
        "copy the link",
        Some(("ctrl-y", "copy link")),
    ),
    bind(
        Context::Code,
        &[code(KeyCode::Esc)],
        Command::Back,
        "back",
        Some(("esc", "back")),
    ),
    bind(
        Context::Waiting,
        &[ch('c')],
        Command::Copy,
        "copy",
        Some(("c", "copy")),
    ),
    bind(
        Context::Failed,
        &[code(KeyCode::Enter)],
        Command::Select,
        "try again",
        Some(("enter", "try again")),
    ),
    bind(
        Context::Failed,
        &[code(KeyCode::Esc)],
        Command::Back,
        "back",
        Some(("esc", "back")),
    ),
    bind(
        Context::Waiting,
        &[code(KeyCode::Esc)],
        Command::Back,
        "back",
        Some(("esc", "back")),
    ),
    bind(
        Context::OneTime,
        &[ch('c')],
        Command::Copy,
        "copy the key",
        Some(("c", "copy key")),
    ),
    bind(
        Context::OneTime,
        &[ch('s')],
        Command::CopySettings,
        "copy the client settings",
        Some(("s", "copy settings")),
    ),
    bind(
        Context::OneTime,
        &[code(KeyCode::Left)],
        Command::Left,
        "previous client",
        None,
    ),
    bind(
        Context::OneTime,
        &[code(KeyCode::Right)],
        Command::Right,
        "next client",
        Some(("←→", "client")),
    ),
    bind(
        Context::OneTime,
        &[code(KeyCode::Enter)],
        Command::Select,
        "done",
        Some(("enter", "done")),
    ),
];

fn bindings(context: Context) -> impl Iterator<Item = &'static Binding> {
    context
        .chain()
        .iter()
        .flat_map(|link| TABLE.iter().filter(move |binding| binding.context == *link))
}

pub(crate) fn lookup(context: Context, event: &KeyEvent) -> Option<Command> {
    let mut ordered: Vec<&Binding> = bindings(context).collect();
    ordered.sort_by_key(|binding| specificity(context, binding.context));
    ordered
        .into_iter()
        .find(|binding| binding.keys.iter().any(|key| key.matches(event)))
        .map(|binding| binding.command)
}

fn specificity(context: Context, link: Context) -> usize {
    context
        .chain()
        .iter()
        .position(|candidate| *candidate == link)
        .map_or(usize::MAX, |position| match link {
            Context::List | Context::Find | Context::Global => position + 10,
            Context::Accounts
            | Context::Activity
            | Context::Models
            | Context::Keys
            | Context::Filter
            | Context::Menu
            | Context::Confirm
            | Context::Help
            | Context::Choice
            | Context::Input
            | Context::Done
            | Context::Code
            | Context::Waiting
            | Context::Failed
            | Context::OneTime
            | Context::Busy
            | Context::Detail => position,
        })
}

pub(crate) fn shortcut(context: Context, command: Command) -> Option<char> {
    bindings(context)
        .filter(|binding| binding.command == command)
        .flat_map(|binding| binding.keys.iter())
        .find_map(|key| match (key.code, key.mods) {
            (KeyCode::Char(c), KeyModifiers::NONE) => Some(c),
            _ => None,
        })
}

pub(crate) fn footer(context: Context) -> Vec<(&'static str, &'static str)> {
    let mut hints: Vec<(&'static str, &'static str)> = Vec::new();
    for binding in bindings(context) {
        if let Some(hint) = binding.footer
            && lookup_first_key(context, binding) == Some(binding.command)
            && !hints.contains(&hint)
        {
            hints.push(hint);
        }
    }
    hints
}

fn lookup_first_key(context: Context, binding: &Binding) -> Option<Command> {
    let key = binding.keys.first()?;
    let event = KeyEvent::new(key.code, key.mods);
    lookup(context, &event)
}

pub(crate) fn help(context: Context) -> Vec<(String, &'static str)> {
    let mut rows: Vec<(String, &'static str)> = Vec::new();
    let mut seen: Vec<Command> = Vec::new();
    let mut ordered: Vec<&Binding> = bindings(context).collect();
    ordered.sort_by_key(|binding| specificity(context, binding.context));
    for binding in ordered {
        if binding.label.is_empty() || seen.contains(&binding.command) {
            continue;
        }
        if matches!(binding.command, Command::Goto(_)) {
            continue;
        }
        seen.push(binding.command);
        let keys: Vec<String> = binding.keys.iter().map(|key| key.name()).collect();
        rows.push((keys.join(" "), binding.label));
    }
    if context.chain().contains(&Context::Global) {
        rows.push(("1 2 3 4".to_owned(), "go to a screen"));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Context; 16] = [
        Context::Accounts,
        Context::Activity,
        Context::Models,
        Context::Keys,
        Context::Filter,
        Context::Menu,
        Context::Confirm,
        Context::Help,
        Context::Choice,
        Context::Input,
        Context::Done,
        Context::Code,
        Context::Waiting,
        Context::Failed,
        Context::OneTime,
        Context::Busy,
    ];

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn footers_have_at_most_five_hints() {
        for context in ALL {
            assert!(footer(context).len() <= 5, "{context:?}");
        }
    }

    #[test]
    fn every_footer_hint_is_in_the_help() {
        for context in ALL {
            let labels: Vec<&str> = help(context).iter().map(|(_, label)| *label).collect();
            for binding in bindings(context).filter(|b| b.footer.is_some()) {
                assert!(
                    labels.contains(&binding.label),
                    "{context:?} {}",
                    binding.label
                );
            }
        }
    }

    #[test]
    fn footer_keys_run_the_command_they_advertise() {
        for context in ALL {
            for binding in bindings(context).filter(|b| b.footer.is_some()) {
                let key = binding.keys.first().unwrap();
                let event = KeyEvent::new(key.code, key.mods);
                let found = lookup(context, &event);
                assert_eq!(
                    found,
                    Some(binding.command),
                    "{context:?} {}",
                    binding.label
                );
            }
        }
    }

    #[test]
    fn view_footers_show_the_documented_hints() {
        let hints = |context| -> Vec<&'static str> {
            footer(context).iter().map(|(key, _)| *key).collect()
        };
        assert_eq!(hints(Context::Accounts), ["↑↓", "enter", "a", "/", "?"]);
        assert_eq!(hints(Context::Activity), ["↑↓", "enter", "end", "/", "?"]);
        assert_eq!(hints(Context::Keys), ["↑↓", "enter", "a", "c", "?"]);
    }

    #[test]
    fn no_key_is_bound_twice_inside_one_context() {
        for link in ALL
            .iter()
            .chain(&[Context::List, Context::Find, Context::Global])
        {
            let mut seen: Vec<Key> = Vec::new();
            for binding in TABLE.iter().filter(|b| b.context == *link) {
                for key in binding.keys {
                    assert!(!seen.contains(key), "{link:?} {key:?}");
                    seen.push(*key);
                }
            }
        }
    }

    #[test]
    fn view_bindings_shadow_the_shared_list_bindings() {
        assert_eq!(
            lookup(Context::Activity, &press(KeyCode::End)),
            Some(Command::Bottom)
        );
        assert_eq!(
            lookup(Context::Accounts, &press(KeyCode::Char('a'))),
            Some(Command::Add)
        );
        assert_eq!(
            lookup(Context::Accounts, &press(KeyCode::Char('j'))),
            Some(Command::Down)
        );
        assert_eq!(
            lookup(Context::Accounts, &press(KeyCode::Char('q'))),
            Some(Command::Quit)
        );
    }

    #[test]
    fn modal_contexts_do_not_inherit_global_keys() {
        assert_eq!(lookup(Context::Menu, &press(KeyCode::Char('q'))), None);
        assert_eq!(lookup(Context::Input, &press(KeyCode::Tab)), None);
        assert_eq!(lookup(Context::Busy, &press(KeyCode::Esc)), None);
    }

    #[test]
    fn shift_is_ignored_for_letters_and_back_tab() {
        let upper = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(lookup(Context::Accounts, &upper), Some(Command::Bottom));
        let back_tab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(
            lookup(Context::Accounts, &back_tab),
            Some(Command::PrevView)
        );
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(lookup(Context::Accounts, &ctrl_c), Some(Command::Quit));
    }

    #[test]
    fn menu_shortcuts_come_from_the_table_and_do_not_collide() {
        let commands = [
            Command::Relogin,
            Command::RefreshUsage,
            Command::Rename,
            Command::Remove,
        ];
        let letters: Vec<char> = commands
            .iter()
            .map(|command| shortcut(Context::Accounts, *command).unwrap())
            .collect();
        assert_eq!(letters, ['l', 'r', 'e', 'd']);
    }

    #[test]
    fn help_lists_view_and_global_keys_once() {
        let rows = help(Context::Accounts);
        assert!(
            rows.iter()
                .any(|(keys, label)| keys == "a" && *label == "add an account")
        );
        assert!(rows.iter().any(|(keys, _)| keys.contains("tab")));
        let quits = rows.iter().filter(|(_, label)| *label == "quit").count();
        assert_eq!(quits, 1);
    }
}
