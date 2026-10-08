//! English and Russian user-visible text. English is the application default.
//! Persisted scheme identifiers are not translated.
mod en;
mod ru;
use serde::{Deserialize, Serialize};
#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[serde(rename = "ru")]
    Russian,
    #[default]
    #[serde(rename = "en", other)]
    English,
}
#[cfg(not(test))]
static RUSSIAN: AtomicBool = AtomicBool::new(false);
// Each legacy fixture tests Russian presentation in isolation. Explicit locale
// tests cover English; parallel tests cannot change another fixture's language.
#[cfg(test)]
thread_local! { static TEST_LANGUAGE: std::cell::Cell<Language> = const { std::cell::Cell::new(Language::Russian) }; }

pub fn set_language(language: Language) {
    #[cfg(not(test))]
    RUSSIAN.store(language == Language::Russian, Ordering::Relaxed);
    #[cfg(test)]
    TEST_LANGUAGE.with(|value| value.set(language));
}
pub fn language() -> Language {
    #[cfg(not(test))]
    {
        if RUSSIAN.load(Ordering::Relaxed) {
            Language::Russian
        } else {
            Language::English
        }
    }
    #[cfg(test)]
    TEST_LANGUAGE.with(std::cell::Cell::get)
}
pub fn pick<'a>(english: &'a str, russian: &'a str) -> &'a str {
    match language() {
        Language::English => english,
        Language::Russian => russian,
    }
}
pub use ru::{APP_TITLE, SCHEME_DARK, SCHEME_LIGHT};
pub fn scheme_label(name: &str) -> &str {
    match name {
        SCHEME_DARK | "Dark" | "dark" => pick("Dark", SCHEME_DARK),
        SCHEME_LIGHT | "Light" | "light" => pick("Light", SCHEME_LIGHT),
        _ => name,
    }
}
macro_rules! texts {
    ($($name:ident),* $(,)?) => { $(
        #[allow(non_snake_case)]
        pub fn $name() -> &'static str { pick(en::$name, ru::$name) }
    )*
        /// Localize known labels saved in a quota snapshot by another window.
        /// Remote/provider content and user-created labels remain untouched.
        pub fn localize(label: &str) -> &str {
            for (english, russian) in [$( (en::$name, ru::$name), )*] {
                if label == english || label == russian { return pick(english, russian); }
            }
            label
        }
    };
}
texts! {
    COPIED,
    CONFIG_BROKEN,
    CONFIG_UNREADABLE,
    UNKNOWN_HOTKEYS,
    CONPTY_MISSING,
    CLAUDE_SETTINGS_BROKEN,
    CLAUDE_SETTINGS_CHANGED,
    BELL,
    WINDOW_MINIMIZE,
    WINDOW_MAXIMIZE,
    WINDOW_CLOSE,
    TAB_RENAME,
    TAB_DUPLICATE,
    TAB_COLOR,
    TAB_COLOR_NONE,
    TAB_COLOR_BLUE,
    TAB_COLOR_GREEN,
    TAB_COLOR_ORANGE,
    TAB_COLOR_PURPLE,
    TAB_COLOR_RED,
    TAB_COLOR_YELLOW,
    TAB_CLOSE,
    TAB_CLOSE_OTHERS,
    TAB_NEW,
    TAB_PROFILES,
    TAB_SETTINGS,
    MENU_COPY,
    MENU_PASTE,
    MENU_SELECT_ALL,
    MENU_CLEAR,
    MENU_SPLIT_RIGHT,
    MENU_SPLIT_DOWN,
    MENU_CLOSE_PANE,
    PANE_CLOSE,
    PANE_REARRANGE_HINT,
    SPAWN_FAILED,
    SEARCH_PLACEHOLDER,
    SEARCH_CASE,
    SEARCH_REGEX,
    SEARCH_PREV,
    SEARCH_NEXT,
    SEARCH_CLOSE,
    SETTINGS_APPEARANCE,
    SETTINGS_TERMINAL,
    SETTINGS_PROFILES,
    SETTINGS_HOTKEYS,
    SETTINGS_FONT,
    SETTINGS_FONT_SIZE,
    SETTINGS_FONT_REFRESH,
    SETTINGS_SCHEME,
    SETTINGS_SCROLLBACK,
    SETTINGS_CURSOR,
    CURSOR_BLOCK,
    CURSOR_BAR,
    CURSOR_UNDERLINE,
    SETTINGS_BLINK,
    SETTINGS_BELL,
    BELL_OFF,
    BELL_VISUAL,
    SETTINGS_RIGHT_CLICK,
    RIGHT_CLICK_CLIPBOARD,
    RIGHT_CLICK_PASTE,
    RIGHT_CLICK_MENU,
    SETTINGS_MIDDLE_CLICK,
    SETTINGS_COPY_ON_SELECT,
    SETTINGS_ALLOW_OSC52,
    SETTINGS_WORD_SEPARATORS,
    SETTINGS_DEFAULT_PROFILE,
    SETTINGS_ADD,
    SETTINGS_EDIT,
    SETTINGS_DELETE,
    SETTINGS_SAVE,
    SETTINGS_CANCEL,
    SETTINGS_NAME,
    SETTINGS_COMMAND,
    SETTINGS_ARGS,
    SETTINGS_CWD,
    SETTINGS_OPEN_CONFIG,
    SETTINGS_WORKSPACE,
    SETTINGS_AI_COMMAND,
    SETTINGS_AI_HINT,
    SETTINGS_AI_MODEL,
    SETTINGS_AI_MODEL_DEFAULT,
    SETTINGS_AI_MODEL_SEARCH,
    SETTINGS_AI_MODEL_REFRESH,
    SETTINGS_AI_MODEL_LOADING,
    SETTINGS_AI_MODEL_HINT,
    SETTINGS_AI_MODEL_NO_MATCH,
    SETTINGS_CLAUDE,
    SETTINGS_CLAUDE_ENABLED,
    SETTINGS_CLAUDE_ENABLED_HINT,
    SETTINGS_CLAUDE_NOW,
    SETTINGS_CLAUDE_LINE_MISSING,
    SETTINGS_CLAUDE_LINE_ANVIL,
    SETTINGS_CLAUDE_LINE_FOREIGN,
    SETTINGS_CLAUDE_LINE_BROKEN,
    SETTINGS_CLAUDE_BADGE,
    SETTINGS_CLAUDE_FIELDS,
    SETTINGS_CLAUDE_IN_CLAUDE,
    SETTINGS_CLAUDE_UNDER_TAB,
    SETTINGS_CLAUDE_FIELD_MODEL,
    SETTINGS_CLAUDE_FIELD_DIR,
    SETTINGS_CLAUDE_FIELD_BRANCH,
    SETTINGS_CLAUDE_FIELD_CONTEXT,
    SETTINGS_CLAUDE_FIELD_FIVE_HOUR,
    SETTINGS_CLAUDE_FIELD_SEVEN_DAY,
    SETTINGS_CLAUDE_FIELD_AGENT,
    SETTINGS_CLAUDE_FIELDS_NEED_ANVIL,
    SETTINGS_CLAUDE_DECLINED,
    SETTINGS_CLAUDE_PENDING,
    SETTINGS_CLAUDE_GLOBAL_HINT,
    SETTINGS_CLAUDE_INSTALL,
    SETTINGS_CLAUDE_RESTORE,
    SETTINGS_NO_PROFILES,
    SETTINGS_APPLY_HINT,
    SETTINGS_HOTKEYS_HINT,
    SETTINGS_QUOTA,
    SETTINGS_QUOTA_ENABLED,
    SETTINGS_QUOTA_INTERVAL,
    SETTINGS_QUOTA_REFRESH,
    SETTINGS_QUOTA_NO_LOGIN,
    SETTINGS_QUOTA_SET_KEY,
    SETTINGS_QUOTA_CHANGE_KEY,
    SETTINGS_QUOTA_DELETE_KEY,
    SETTINGS_QUOTA_KEY_FAILED,
    SETTINGS_QUOTA_KEY_DELETE_FAILED,
    QUOTA_REFRESH_DEFERRED,
    SETTINGS_QUOTA_KEY_INVALID,
    SETTINGS_QUOTA_WINDOWS_LATER,
    QUOTA_NO_DATA,
    QUOTA_REFRESH_FALLBACK,
    QUOTA_REFRESH_HINT,
    QUOTA_LOGIN,
    QUOTA_SOURCE_CLAUDE_CODE,
    QUOTA_SOURCE_CODEX,
    QUOTA_SOURCE_OMP,
    QUOTA_SOURCE_OPENCODE,
    QUOTA_SOURCE_ANVIL_KEY,
    QUOTA_SOURCE_ENV,
    QUOTA_SOURCE_CLAUDE_SETTINGS,
    QUOTA_UNIT_DAY,
    QUOTA_UNIT_HOUR,
    QUOTA_UNIT_MINUTE,
    QUOTA_MONTH,
    QUOTA_MCP,
    QUOTA_REVIEW,
    QUOTA_AUTH_EXPIRED,
    QUOTA_FORMAT_ERROR,
    QUOTA_STORE_UNREADABLE,
    QUOTA_RESET_IN,
    QUOTA_DATA_AT,
    QUOTA_CREDITS,
    QUOTA_CREDITS_SHORT,
    QUOTA_BALANCE,
    QUOTA_SPEND,
    QUOTA_SESSION,
    QUOTA_KILO_PASS,
    QUOTA_TOPPED_UP,
    QUOTA_GRANTED,
    QUOTA_CREDIT_LIMIT,
    QUOTA_NO_SUBSCRIPTION,
    QUOTA_ZEN_OTHER_SERVER,
    QUOTA_PROVIDER_SAID,
    QUOTA_RATE_LIMITED,
    SETTINGS_QUOTA_OWN_KEY,
    SETTINGS_QUOTA_ZEN_LOGIN,
    CLAUDE_INSTALL_QUESTION,
    CLAUDE_INSTALL_GLOBAL,
    CLAUDE_CURRENT_COMMAND,
    CLAUDE_NEW_COMMAND,
    CLAUDE_INSTALL_WARNING,
    CLAUDE_INSTALL_ACCEPT,
    CLAUDE_HELPER_MISSING,
    CLAUDE_DIRECTORY_CHANGED,
    PASTE_WARNING,
    PASTE_PREVIEW_HINT,
    PASTE_ACCEPT,
    WORKSPACE_NO_REPO,
    WORKSPACE_NO_REPO_HINT,
    WORKSPACE_CLOSE,
    WORKSPACE_REFRESH,
    WORKSPACE_STAGE,
    WORKSPACE_UNSTAGE,
    WORKSPACE_STAGE_ALL,
    WORKSPACE_UNSTAGE_ALL,
    WORKSPACE_COMMIT_HINT,
    WORKSPACE_COMMIT,
    WORKSPACE_AI,
    WORKSPACE_AI_GENERATING,
    WORKSPACE_DIFF_STAGED,
    WORKSPACE_DIFF_LOADING,
    WORKSPACE_TOGGLE_HINT,
    WORKSPACE_TAB_CHANGES,
    WORKSPACE_TAB_FILES,
    WORKSPACE_COMMITS_TITLE,
    WORKSPACE_PUBLISH,
    WORKSPACE_TRUNCATED,
    WORKSPACE_NO_COMMITS,
    WORKSPACE_COUNT_LINES,
    ZOOM_TOAST_PREFIX,
    WORKSPACE_SECTION_OUTGOING,
    WORKSPACE_SECTION_INCOMING,
    WORKSPACE_SECTION_HISTORY,
    WORKSPACE_BACK,
    WORKSPACE_COPY_HASH,
    WORKSPACE_COPY_PATCH,
    WORKSPACE_NEW_FILE,
    WORKSPACE_NEW_FOLDER,
    WORKSPACE_RENAME,
    WORKSPACE_DELETE,
    WORKSPACE_DELETE_HINT,
    WORKSPACE_DELETE_FILE_HINT,
    WORKSPACE_OPEN_EXTERNAL,
    WORKSPACE_REVEAL,
    WORKSPACE_FILE_FILTER,
    WORKSPACE_FILE_TRUNCATED,
    WORKSPACE_PUSH_HINT,
    WORKSPACE_FETCH_HINT,
    WORKSPACE_BRANCH_HINT,
    WORKSPACE_BRANCH_FILTER,
    WORKSPACE_BRANCHES_LOADING,
    WORKSPACE_BRANCH_NEW,
    WORKSPACE_BRANCH_CREATE,
    WORKSPACE_BRANCH_INVALID,
    WORKSPACE_BRANCH_REMOTE_HINT,
    WORKSPACE_NO_BRANCH,
    WORKSPACE_NO_AI_COMMAND,
    WORKSPACE_AI_EMPTY,
    WORKSPACE_NO_CHANGES_FOR_FILE,
    WORKSPACE_PATH_INSIDE_REPO,
    WORKSPACE_FILE_EXISTS,
    WORKSPACE_NO_SUCH_FILE,
    WORKSPACE_WORKER_LOST,
    WORKSPACE_DIFF_STALE,
    WORKSPACE_REPO_CHANGED,
    WORKSPACE_TRUST_TITLE,
    WORKSPACE_TRUST_HINT,
    WORKSPACE_TRUST_HAZARDS,
    WORKSPACE_TRUST_APPROVE,
    WORKSPACE_TRUST_STALE,
    WORKSPACE_AI_NO_STAGE,
    PICKER_FILTER,
    PICKER_EMPTY,
    COLLAPSED_HINT,
    COLLAPSED_EMPTY,
    TAB_COLLAPSED,
    WORKSPACE_PREVIEW_CLOSE
}
pub fn process_exited(code: Option<i32>) -> String {
    match language() {
        Language::English => en::process_exited(code),
        Language::Russian => ru::process_exited(code),
    }
}
pub fn workspace_changes_tab(count: usize) -> String {
    match language() {
        Language::English => en::workspace_changes_tab(count),
        Language::Russian => ru::workspace_changes_tab(count),
    }
}
pub fn workspace_line_count(files: usize, lines: u64) -> String {
    match language() {
        Language::English => en::workspace_line_count(files, lines),
        Language::Russian => ru::workspace_line_count(files, lines),
    }
}
pub fn zoom_toast(size: f32) -> String {
    match language() {
        Language::English => en::zoom_toast(size),
        Language::Russian => ru::zoom_toast(size),
    }
}
pub fn relative_time(now_secs: i64, then_secs: i64) -> String {
    match language() {
        Language::English => en::relative_time(now_secs, then_secs),
        Language::Russian => ru::relative_time(now_secs, then_secs),
    }
}
pub fn workspace_pushed(branch: &str) -> String {
    match language() {
        Language::English => en::workspace_pushed(branch),
        Language::Russian => ru::workspace_pushed(branch),
    }
}
pub fn workspace_switched(branch: &str) -> String {
    match language() {
        Language::English => en::workspace_switched(branch),
        Language::Russian => ru::workspace_switched(branch),
    }
}
pub fn workspace_fetch_done(what: &str) -> String {
    match language() {
        Language::English => en::workspace_fetch_done(what),
        Language::Russian => ru::workspace_fetch_done(what),
    }
}
pub fn workspace_staged(staged: usize, unstaged: usize) -> String {
    match language() {
        Language::English => en::workspace_staged(staged, unstaged),
        Language::Russian => ru::workspace_staged(staged, unstaged),
    }
}
pub fn workspace_committed(hash: &str) -> String {
    match language() {
        Language::English => en::workspace_committed(hash),
        Language::Russian => ru::workspace_committed(hash),
    }
}
pub fn workspace_ai(command: &str) -> String {
    match language() {
        Language::English => en::workspace_ai(command),
        Language::Russian => ru::workspace_ai(command),
    }
}
pub fn hotkey_label(action: &crate::hotkeys::Action) -> String {
    match language() {
        Language::English => en::hotkey_label(action),
        Language::Russian => ru::hotkey_label(action),
    }
}

#[macro_export]
macro_rules! tr_format {
    ($en:literal, $ru:literal $(, $args:expr)* $(,)?) => {
        if $crate::strings::language() == $crate::strings::Language::English {
            format!($en $(, $args)*)
        } else {
            format!($ru $(, $args)*)
        }
    };
}

/// Timed-window labels can come from a cache written in either language.
pub fn quota_window_label(label: &str) -> String {
    for (suffixes, english, russian) in
        [(["d", "д", "days"], "d", "д"), (["h", "ч", "hours"], "h", "ч"), (["m", "мин", "min"], "min", "мин")]
    {
        for suffix in suffixes {
            if let Some(number) =
                label.strip_suffix(suffix).filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            {
                return format!("{number}{}", pick(english, russian));
            }
        }
    }
    localize(label).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locale_switches_without_changing_saved_identifiers() {
        assert_eq!(Language::default(), Language::English);
        set_language(Language::English);
        assert_eq!(WORKSPACE_TAB_FILES(), "Files");
        assert_eq!(workspace_line_count(3, 20), "20 lines · 3 files");
        assert_eq!(relative_time(120, 0), "2 min ago");
        assert_eq!(scheme_label(SCHEME_LIGHT), "Light");
        assert_eq!(localize(ru::QUOTA_SPEND), "spend");
        set_language(Language::Russian);
        assert_eq!(WORKSPACE_TAB_FILES(), ru::WORKSPACE_TAB_FILES);
        assert_eq!(scheme_label(SCHEME_LIGHT), SCHEME_LIGHT);
        assert_eq!(localize("spend"), ru::QUOTA_SPEND);
        assert_eq!(localize("My custom label"), "My custom label");
    }
    #[test]
    fn hostile_timestamps_cannot_overflow_relative_time() {
        set_language(Language::English);
        assert!(relative_time(i64::MAX, i64::MIN).ends_with(" y ago"));
        set_language(Language::Russian);
        assert!(!relative_time(i64::MAX, i64::MIN).is_empty());
    }
}
