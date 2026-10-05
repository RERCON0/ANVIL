//! Hotkey table: Helm-style chord strings ("Ctrl-Shift-T") bound to actions.
//! Chords name physical keys, so bindings work on any keyboard layout.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

/// A physical key. The variants cover every key a chord or the terminal
/// encoder needs; the host maps winit's `KeyCode` onto them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyName {
    /// 'A'..='Z'
    Letter(char),
    /// 0..=9
    Digit(u8),
    /// 1..=24
    F(u8),
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Backspace,
    Enter,
    Tab,
    Space,
    Escape,
    Equal,
    Minus,
    Comma,
    Period,
    BracketLeft,
    BracketRight,
    Slash,
    Backslash,
    Backquote,
    Semicolon,
    Quote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub mods: Mods,
    pub key: KeyName,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    NewTab,
    NewWindow,
    CloseTab,
    ReopenTab,
    RenameTab,
    NextTab,
    PreviousTab,
    MoveTabLeft,
    MoveTabRight,
    /// 1..=10
    Tab(u8),
    SplitRight,
    SplitBottom,
    PaneNavLeft,
    PaneNavRight,
    PaneNavUp,
    PaneNavDown,
    PaneNavPrevious,
    PaneNavNext,
    PaneMaximize,
    ClosePane,
    PaneCollapse,
    PaneRestore,
    /// New tab with the profile of this id.
    Profile(String),
    ProfileSelector,
    Settings,
    ToggleFullscreen,
    /// The frame-time overlay, so frame-path work can be judged in the app
    /// itself instead of only in an external harness.
    ToggleFrameStats,
    CtrlC,
    Copy,
    Paste,
    SelectAll,
    Clear,
    ZoomIn,
    ZoomOut,
    ResetZoom,
    PreviousWord,
    NextWord,
    DeletePreviousWord,
    DeleteNextWord,
    DeleteLine,
    Search,
    ToggleWorkspace,
    ScrollToTop,
    ScrollToBottom,
    ScrollPageUp,
    ScrollPageDown,
    ScrollUp,
    ScrollDown,
}

const SIMPLE_ACTIONS: &[(&str, Action)] = &[
    ("new-tab", Action::NewTab),
    ("new-window", Action::NewWindow),
    ("close-tab", Action::CloseTab),
    ("reopen-tab", Action::ReopenTab),
    ("rename-tab", Action::RenameTab),
    ("next-tab", Action::NextTab),
    ("previous-tab", Action::PreviousTab),
    ("move-tab-left", Action::MoveTabLeft),
    ("move-tab-right", Action::MoveTabRight),
    ("split-right", Action::SplitRight),
    ("split-bottom", Action::SplitBottom),
    ("pane-nav-left", Action::PaneNavLeft),
    ("pane-nav-right", Action::PaneNavRight),
    ("pane-nav-up", Action::PaneNavUp),
    ("pane-nav-down", Action::PaneNavDown),
    ("pane-nav-previous", Action::PaneNavPrevious),
    ("pane-nav-next", Action::PaneNavNext),
    ("pane-maximize", Action::PaneMaximize),
    ("close-pane", Action::ClosePane),
    ("pane-collapse", Action::PaneCollapse),
    ("pane-restore", Action::PaneRestore),
    ("profile-selector", Action::ProfileSelector),
    ("settings", Action::Settings),
    ("toggle-fullscreen", Action::ToggleFullscreen),
    ("toggle-frame-stats", Action::ToggleFrameStats),
    ("ctrl-c", Action::CtrlC),
    ("copy", Action::Copy),
    ("paste", Action::Paste),
    ("select-all", Action::SelectAll),
    ("clear", Action::Clear),
    ("zoom-in", Action::ZoomIn),
    ("zoom-out", Action::ZoomOut),
    ("reset-zoom", Action::ResetZoom),
    ("previous-word", Action::PreviousWord),
    ("next-word", Action::NextWord),
    ("delete-previous-word", Action::DeletePreviousWord),
    ("delete-next-word", Action::DeleteNextWord),
    ("delete-line", Action::DeleteLine),
    ("search", Action::Search),
    ("toggle-workspace", Action::ToggleWorkspace),
    ("scroll-to-top", Action::ScrollToTop),
    ("scroll-to-bottom", Action::ScrollToBottom),
    ("scroll-page-up", Action::ScrollPageUp),
    ("scroll-page-down", Action::ScrollPageDown),
    ("scroll-up", Action::ScrollUp),
    ("scroll-down", Action::ScrollDown),
];

impl Action {
    pub fn from_id(id: &str) -> Option<Action> {
        if let Some(n) = id.strip_prefix("tab-") {
            return match n.parse::<u8>() {
                Ok(n @ 1..=10) => Some(Action::Tab(n)),
                _ => None,
            };
        }
        if let Some(profile) = id.strip_prefix("profile:") {
            return (!profile.is_empty()).then(|| Action::Profile(profile.to_owned()));
        }
        SIMPLE_ACTIONS.iter().find(|(name, _)| *name == id).map(|(_, a)| a.clone())
    }

    pub fn id(&self) -> String {
        // Exhaustive: adding an action must also name it, at compile time.
        let name = match self {
            Action::Tab(n) => return format!("tab-{n}"),
            Action::Profile(p) => return format!("profile:{p}"),
            Action::NewTab => "new-tab",
            Action::NewWindow => "new-window",
            Action::CloseTab => "close-tab",
            Action::ReopenTab => "reopen-tab",
            Action::RenameTab => "rename-tab",
            Action::NextTab => "next-tab",
            Action::PreviousTab => "previous-tab",
            Action::MoveTabLeft => "move-tab-left",
            Action::MoveTabRight => "move-tab-right",
            Action::SplitRight => "split-right",
            Action::SplitBottom => "split-bottom",
            Action::PaneNavLeft => "pane-nav-left",
            Action::PaneNavRight => "pane-nav-right",
            Action::PaneNavUp => "pane-nav-up",
            Action::PaneNavDown => "pane-nav-down",
            Action::PaneNavPrevious => "pane-nav-previous",
            Action::PaneNavNext => "pane-nav-next",
            Action::PaneMaximize => "pane-maximize",
            Action::ClosePane => "close-pane",
            Action::PaneCollapse => "pane-collapse",
            Action::PaneRestore => "pane-restore",
            Action::ProfileSelector => "profile-selector",
            Action::Settings => "settings",
            Action::ToggleFullscreen => "toggle-fullscreen",
            Action::ToggleFrameStats => "toggle-frame-stats",
            Action::CtrlC => "ctrl-c",
            Action::Copy => "copy",
            Action::Paste => "paste",
            Action::SelectAll => "select-all",
            Action::Clear => "clear",
            Action::ZoomIn => "zoom-in",
            Action::ZoomOut => "zoom-out",
            Action::ResetZoom => "reset-zoom",
            Action::PreviousWord => "previous-word",
            Action::NextWord => "next-word",
            Action::DeletePreviousWord => "delete-previous-word",
            Action::DeleteNextWord => "delete-next-word",
            Action::DeleteLine => "delete-line",
            Action::Search => "search",
            Action::ToggleWorkspace => "toggle-workspace",
            Action::ScrollToTop => "scroll-to-top",
            Action::ScrollToBottom => "scroll-to-bottom",
            Action::ScrollPageUp => "scroll-page-up",
            Action::ScrollPageDown => "scroll-page-down",
            Action::ScrollUp => "scroll-up",
            Action::ScrollDown => "scroll-down",
        };
        name.to_owned()
    }

    /// Actions that act on the focused terminal pane. They only fire while the
    /// terminal owns the keyboard; everything else fires from anywhere.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Action::CtrlC
                | Action::Copy
                | Action::Paste
                | Action::SelectAll
                | Action::Clear
                | Action::ZoomIn
                | Action::ZoomOut
                | Action::ResetZoom
                | Action::PreviousWord
                | Action::NextWord
                | Action::DeletePreviousWord
                | Action::DeleteNextWord
                | Action::DeleteLine
                | Action::Search
                | Action::ToggleWorkspace
                | Action::ScrollToTop
                | Action::ScrollToBottom
                | Action::ScrollPageUp
                | Action::ScrollPageDown
                | Action::ScrollUp
                | Action::ScrollDown
        )
    }

    /// Whether holding the key should keep running the action. Continuous
    /// operations the user holds (scrolling, zooming, moving and deleting by
    /// word, an interrupting Ctrl+C) repeat; anything that opens, closes or
    /// pastes must happen once per press, or a held key repeats it.
    pub fn repeats_on_hold(&self) -> bool {
        matches!(
            self,
            Action::ScrollUp
                | Action::ScrollDown
                | Action::ScrollPageUp
                | Action::ScrollPageDown
                | Action::ZoomIn
                | Action::ZoomOut
                | Action::PreviousWord
                | Action::NextWord
                | Action::DeletePreviousWord
                | Action::DeleteNextWord
                | Action::DeleteLine
                | Action::CtrlC
        )
    }
}

/// Default bindings: the owner's Helm configuration.
pub const DEFAULT_BINDINGS: &[(&str, &[&str])] = &[
    ("new-tab", &["Ctrl-Shift-T"]),
    ("new-window", &["Ctrl-Shift-N"]),
    ("close-tab", &["Ctrl-Shift-W"]),
    ("reopen-tab", &["Ctrl-Shift-Z"]),
    ("rename-tab", &["Ctrl-Shift-R"]),
    ("next-tab", &["Ctrl-Shift-Right", "Ctrl-Tab"]),
    ("previous-tab", &["Ctrl-Shift-Left", "Ctrl-Shift-Tab"]),
    ("move-tab-left", &["Ctrl-Shift-PageUp"]),
    ("move-tab-right", &["Ctrl-Shift-PageDown"]),
    ("tab-1", &["Alt-1"]),
    ("tab-2", &["Alt-2"]),
    ("tab-3", &["Alt-3"]),
    ("tab-4", &["Alt-4"]),
    ("tab-5", &["Alt-5"]),
    ("tab-6", &["Alt-6"]),
    ("tab-7", &["Alt-7"]),
    ("tab-8", &["Alt-8"]),
    ("tab-9", &["Alt-9"]),
    ("tab-10", &["Alt-0"]),
    ("split-right", &["Ctrl-Shift-S"]),
    ("split-bottom", &["Ctrl-Shift-D"]),
    ("pane-nav-left", &["Ctrl-Alt-Left"]),
    ("pane-nav-right", &["Ctrl-Alt-Right"]),
    ("pane-nav-up", &["Ctrl-Alt-Up"]),
    ("pane-nav-down", &["Ctrl-Alt-Down"]),
    ("pane-nav-previous", &["Ctrl-Alt-["]),
    ("pane-nav-next", &["Ctrl-Alt-]"]),
    ("pane-maximize", &["Ctrl-Alt-Enter"]),
    ("close-pane", &["Ctrl-Shift-L"]),
    ("pane-collapse", &["Ctrl-Alt-C"]),
    ("pane-restore", &["Ctrl-Alt-R"]),
    ("profile:powershell", &["Ctrl-Alt-P"]),
    ("profile-selector", &["Ctrl-Shift-E"]),
    ("settings", &["Ctrl-,"]),
    ("toggle-fullscreen", &["F11", "Alt-Enter"]),
    ("toggle-frame-stats", &["Ctrl-Shift-F12"]),
    ("ctrl-c", &["Ctrl-C"]),
    ("copy", &["Ctrl-Shift-C"]),
    ("paste", &["Ctrl-Shift-V", "Shift-Insert"]),
    ("select-all", &["Ctrl-Shift-A"]),
    ("clear", &["Ctrl-K"]),
    ("zoom-in", &["Ctrl-=", "Ctrl-Shift-="]),
    ("zoom-out", &["Ctrl--", "Ctrl-Shift--"]),
    ("reset-zoom", &["Ctrl-0"]),
    ("previous-word", &["Ctrl-Left"]),
    ("next-word", &["Ctrl-Right"]),
    ("delete-previous-word", &["Ctrl-Backspace"]),
    ("delete-next-word", &["Ctrl-Delete"]),
    ("delete-line", &["Ctrl-Shift-Backspace"]),
    ("search", &["Ctrl-Shift-F"]),
    ("toggle-workspace", &["Ctrl-Shift-G"]),
    ("scroll-to-top", &["Ctrl-PageUp"]),
    ("scroll-to-bottom", &["Ctrl-PageDown"]),
    ("scroll-page-up", &["Alt-PageUp"]),
    ("scroll-page-down", &["Alt-PageDown"]),
    ("scroll-up", &["Ctrl-Shift-Up"]),
    ("scroll-down", &["Ctrl-Shift-Down"]),
];

pub fn parse_chord(text: &str) -> Result<Chord, String> {
    let s = text.trim();
    let (prefix, key) = if s == "-" {
        ("", "-")
    } else if let Some(p) = s.strip_suffix("--") {
        (p, "-")
    } else {
        match s.rfind('-') {
            Some(i) => (&s[..i], &s[i + 1..]),
            None => ("", s),
        }
    };
    let mut mods = Mods::default();
    for part in prefix.split('-').filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" | "option" | "⌥" => mods.alt = true,
            "shift" => mods.shift = true,
            "meta" | "cmd" | "super" | "win" | "⌘" => mods.meta = true,
            _ => return Err(format!("unknown modifier `{part}` in `{text}`")),
        }
    }
    let key = parse_key(key).ok_or_else(|| format!("unknown key in `{text}`"))?;
    Ok(Chord { mods, key })
}

fn parse_key(name: &str) -> Option<KeyName> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.clone().next()) {
        return match c {
            'a'..='z' | 'A'..='Z' => Some(KeyName::Letter(c.to_ascii_uppercase())),
            '0'..='9' => Some(KeyName::Digit(c as u8 - b'0')),
            '=' => Some(KeyName::Equal),
            '-' => Some(KeyName::Minus),
            ',' => Some(KeyName::Comma),
            '.' => Some(KeyName::Period),
            '[' => Some(KeyName::BracketLeft),
            ']' => Some(KeyName::BracketRight),
            '/' => Some(KeyName::Slash),
            '\\' => Some(KeyName::Backslash),
            '`' => Some(KeyName::Backquote),
            ';' => Some(KeyName::Semicolon),
            '\'' => Some(KeyName::Quote),
            _ => None,
        };
    }
    let lower = name.to_ascii_lowercase();
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=24).contains(&n).then_some(KeyName::F(n));
    }
    Some(match lower.as_str() {
        "left" => KeyName::Left,
        "right" => KeyName::Right,
        "up" => KeyName::Up,
        "down" => KeyName::Down,
        "home" => KeyName::Home,
        "end" => KeyName::End,
        "pageup" => KeyName::PageUp,
        "pagedown" => KeyName::PageDown,
        "insert" => KeyName::Insert,
        "delete" => KeyName::Delete,
        "backspace" => KeyName::Backspace,
        "enter" | "return" => KeyName::Enter,
        "tab" => KeyName::Tab,
        "space" => KeyName::Space,
        "escape" | "esc" => KeyName::Escape,
        _ => return None,
    })
}

pub struct Keymap {
    bindings: Vec<(Chord, Action)>,
    rows: Vec<(String, Vec<String>)>,
}

impl Keymap {
    pub fn defaults() -> Keymap {
        Keymap::with_overrides(&BTreeMap::new()).0
    }

    /// Defaults with the user's per-action lists applied. An override replaces
    /// every default chord of that action; an empty list unbinds it. Returns the
    /// problems found (unknown actions, unparsable chords) for the log.
    pub fn with_overrides(overrides: &BTreeMap<String, Vec<String>>) -> (Keymap, Vec<String>) {
        let mut problems = Vec::new();
        let mut bindings = Vec::new();
        for (id, chords) in overrides {
            let Some(action) = Action::from_id(id) else {
                problems.push(format!("unknown hotkey action `{id}`"));
                continue;
            };
            for chord in chords {
                match parse_chord(chord) {
                    Ok(c) => bindings.push((c, action.clone())),
                    Err(e) => problems.push(e),
                }
            }
        }
        for (id, chords) in DEFAULT_BINDINGS {
            if overrides.contains_key(*id) {
                continue;
            }
            let action = Action::from_id(id).expect("default action ids are valid");
            for chord in *chords {
                bindings.push((parse_chord(chord).expect("default chords parse"), action.clone()));
            }
        }
        for (index, (chord, action)) in bindings.iter().enumerate() {
            if let Some((_, winner)) = bindings[..index].iter().find(|(other, _)| other == chord) {
                if winner != action {
                    problems.push(format!("hotkey {}: {} wins over {}", format_chord(chord), winner.id(), action.id()));
                }
            }
        }
        let mut keymap = Keymap { bindings, rows: Vec::new() };
        keymap.rows = keymap.describe_rows();
        (keymap, problems)
    }

    pub fn lookup(&self, chord: &Chord) -> Option<&Action> {
        self.bindings.iter().find(|(c, _)| c == chord).map(|(_, a)| a)
    }

    /// All (action id, chord strings) pairs for the settings page, in table order.
    /// An action the user unbound appears with an empty list rather than
    /// vanishing: a row that disappears looks like a feature that was removed.
    pub fn describe(&self) -> &[(String, Vec<String>)] {
        &self.rows
    }

    fn describe_rows(&self) -> Vec<(String, Vec<String>)> {
        let mut out: Vec<(String, Vec<String>)> =
            SIMPLE_ACTIONS.iter().map(|(_, action)| (action.id(), Vec::new())).collect();
        for (chord, action) in &self.bindings {
            let id = action.id();
            let text = format_chord(chord);
            match out.iter_mut().find(|(a, _)| *a == id) {
                Some((_, list)) => list.push(text),
                None => out.push((id, vec![text])),
            }
        }
        out
    }
}

pub fn format_chord(chord: &Chord) -> String {
    let mut parts: Vec<String> = Vec::new();
    if chord.mods.ctrl {
        parts.push("Ctrl".into());
    }
    if chord.mods.alt {
        parts.push("Alt".into());
    }
    if chord.mods.shift {
        parts.push("Shift".into());
    }
    if chord.mods.meta {
        parts.push("Meta".into());
    }
    let key = match chord.key {
        KeyName::Letter(c) => c.to_string(),
        KeyName::Digit(d) => d.to_string(),
        KeyName::F(n) => format!("F{n}"),
        KeyName::Equal => "=".into(),
        KeyName::Minus => "-".into(),
        KeyName::Comma => ",".into(),
        KeyName::Period => ".".into(),
        KeyName::BracketLeft => "[".into(),
        KeyName::BracketRight => "]".into(),
        KeyName::Slash => "/".into(),
        KeyName::Backslash => "\\".into(),
        KeyName::Backquote => "`".into(),
        KeyName::Semicolon => ";".into(),
        KeyName::Quote => "'".into(),
        other => format!("{other:?}"),
    };
    parts.push(key);
    parts.join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(mods: Mods, key: KeyName) -> Chord {
        Chord { mods, key }
    }

    const CTRL: Mods = Mods { ctrl: true, alt: false, shift: false, meta: false };
    const CTRL_SHIFT: Mods = Mods { ctrl: true, alt: false, shift: true, meta: false };
    const CTRL_ALT: Mods = Mods { ctrl: true, alt: true, shift: false, meta: false };
    const ALT: Mods = Mods { ctrl: false, alt: true, shift: false, meta: false };

    #[test]
    fn conflicts_are_reported_without_changing_override_priority() {
        let overrides =
            BTreeMap::from([("new-tab".into(), vec!["Ctrl-X".into()]), ("close-tab".into(), vec!["Ctrl-X".into()])]);
        let (keymap, problems) = Keymap::with_overrides(&overrides);
        assert_eq!(keymap.lookup(&parse_chord("Ctrl-X").unwrap()), Some(&Action::CloseTab));
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("close-tab wins over new-tab"));
        assert_eq!(keymap.describe().as_ptr(), keymap.describe().as_ptr(), "descriptions are cached");
    }

    #[test]
    fn parses_every_default_chord() {
        for (id, chords) in DEFAULT_BINDINGS {
            assert!(Action::from_id(id).is_some(), "{id}");
            for c in *chords {
                parse_chord(c).unwrap_or_else(|e| panic!("{e}"));
            }
        }
    }

    #[test]
    fn parses_minus_and_punctuation_keys() {
        assert_eq!(parse_chord("Ctrl--").unwrap(), chord(CTRL, KeyName::Minus));
        assert_eq!(parse_chord("Ctrl-Shift--").unwrap(), chord(CTRL_SHIFT, KeyName::Minus));
        assert_eq!(parse_chord("Ctrl-=").unwrap(), chord(CTRL, KeyName::Equal));
        assert_eq!(parse_chord("Ctrl-,").unwrap(), chord(CTRL, KeyName::Comma));
        assert_eq!(parse_chord("Ctrl-Alt-[").unwrap(), chord(CTRL_ALT, KeyName::BracketLeft));
        assert_eq!(parse_chord("Alt-0").unwrap(), chord(ALT, KeyName::Digit(0)));
        assert_eq!(parse_chord("F11").unwrap(), chord(Mods::default(), KeyName::F(11)));
        assert_eq!(parse_chord("ctrl-shift-t").unwrap(), chord(CTRL_SHIFT, KeyName::Letter('T')));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_chord("Ctrl-Hyper-T").is_err());
        assert!(parse_chord("Ctrl-Shift-Nope").is_err());
        assert!(parse_chord("F25").is_err());
    }

    #[test]
    fn action_ids_round_trip() {
        for (id, _) in DEFAULT_BINDINGS {
            assert_eq!(Action::from_id(id).unwrap().id(), *id);
        }
        assert_eq!(Action::from_id("tab-11"), None);
        assert_eq!(Action::from_id("profile:"), None);
        assert_eq!(Action::from_id("nope"), None);
    }

    #[test]
    fn default_lookup() {
        let km = Keymap::defaults();
        assert_eq!(km.lookup(&chord(CTRL_SHIFT, KeyName::Letter('T'))), Some(&Action::NewTab));
        assert_eq!(km.lookup(&chord(CTRL, KeyName::Tab)), Some(&Action::NextTab));
        assert_eq!(km.lookup(&chord(ALT, KeyName::Digit(0))), Some(&Action::Tab(10)));
        assert_eq!(
            km.lookup(&chord(CTRL_ALT, KeyName::Letter('P'))),
            Some(&Action::Profile("powershell".into()))
        );
        assert_eq!(km.lookup(&chord(CTRL, KeyName::Letter('C'))), Some(&Action::CtrlC));
        assert_eq!(km.lookup(&chord(CTRL, KeyName::Letter('V'))), None, "Ctrl+V goes to the app as ^V");
        assert_eq!(km.lookup(&chord(CTRL, KeyName::Space)), None, "Ctrl+Space goes to the app as NUL");
    }

    #[test]
    fn overrides_replace_and_unbind() {
        let mut o = BTreeMap::new();
        o.insert("split-right".to_owned(), vec!["Ctrl-Alt-S".to_owned()]);
        o.insert("close-pane".to_owned(), vec![]);
        o.insert("bogus".to_owned(), vec!["Ctrl-B".to_owned()]);
        o.insert("copy".to_owned(), vec!["Ctrl-Hyper-C".to_owned()]);
        let (km, problems) = Keymap::with_overrides(&o);
        assert_eq!(km.lookup(&chord(CTRL_ALT, KeyName::Letter('S'))), Some(&Action::SplitRight));
        assert_eq!(km.lookup(&chord(CTRL_SHIFT, KeyName::Letter('S'))), None);
        assert_eq!(km.lookup(&chord(CTRL_SHIFT, KeyName::Letter('L'))), None);
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn terminal_actions_are_flagged() {
        assert!(Action::Copy.is_terminal());
        assert!(Action::ZoomIn.is_terminal());
        assert!(!Action::NewTab.is_terminal());
        assert!(!Action::ToggleFullscreen.is_terminal());
    }

    #[test]
    fn describe_groups_chords_by_action() {
        let km = Keymap::defaults();
        let d = km.describe();
        let paste = d.iter().find(|(id, _)| id == "paste").unwrap();
        assert_eq!(paste.1, vec!["Ctrl-Shift-V".to_owned(), "Shift-Insert".to_owned()]);
        let zoom = d.iter().find(|(id, _)| id == "zoom-out").unwrap();
        assert_eq!(zoom.1, vec!["Ctrl--".to_owned(), "Ctrl-Shift--".to_owned()]);
    }
}
