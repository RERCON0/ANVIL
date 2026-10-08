#![allow(dead_code)]
//! English UI catalog.
pub const APP_TITLE: &str = "ANVIL";
pub const COPIED: &str = "Copied";
pub const CONFIG_BROKEN: &str = "config.json is damaged — defaults loaded; the original file has been saved";
pub const CONFIG_UNREADABLE: &str = "Cannot read config.json — defaults loaded";
pub const UNKNOWN_HOTKEYS: &str = "config.json contains invalid or conflicting keyboard shortcuts";
pub const CONPTY_MISSING: &str = "conpty.dll not found — omp and opencode may have input artifacts";
pub const CLAUDE_SETTINGS_BROKEN: &str = "Claude Code settings.json is damaged — status integration was not installed";
pub const CLAUDE_SETTINGS_CHANGED: &str = "settings.json changed during setup — status integration was not installed";
pub const BELL: &str = "Bell";
pub const WINDOW_MINIMIZE: &str = "Minimize";
pub const WINDOW_MAXIMIZE: &str = "Maximize / restore";
pub const WINDOW_CLOSE: &str = "Close";
pub const TAB_RENAME: &str = "Rename";
pub const TAB_DUPLICATE: &str = "Duplicate";
pub const TAB_COLOR: &str = "Color";
pub const TAB_COLOR_NONE: &str = "No color";
pub const TAB_COLOR_BLUE: &str = "Blue";
pub const TAB_COLOR_GREEN: &str = "Green";
pub const TAB_COLOR_ORANGE: &str = "Orange";
pub const TAB_COLOR_PURPLE: &str = "Purple";
pub const TAB_COLOR_RED: &str = "Red";
pub const TAB_COLOR_YELLOW: &str = "Yellow";
pub const TAB_CLOSE: &str = "Close";
pub const TAB_CLOSE_OTHERS: &str = "Close other tabs";
pub const TAB_NEW: &str = "New tab";
pub const TAB_PROFILES: &str = "Profiles";
pub const TAB_SETTINGS: &str = "Settings";
pub const MENU_COPY: &str = "Copy";
pub const MENU_PASTE: &str = "Paste";
pub const MENU_SELECT_ALL: &str = "Select all";
pub const MENU_CLEAR: &str = "Clear";
pub const MENU_SPLIT_RIGHT: &str = "Split right";
pub const MENU_SPLIT_DOWN: &str = "Split down";
pub const MENU_CLOSE_PANE: &str = "Close pane";
pub const PANE_CLOSE: &str = "Close";
pub const PANE_REARRANGE_HINT: &str = "Hold Ctrl+Shift and drag the pane to an edge of another pane or tab";
pub const SPAWN_FAILED: &str = "Could not start the profile's program";
pub const SEARCH_PLACEHOLDER: &str = "Search";
pub const SEARCH_CASE: &str = "Match case";
pub const SEARCH_REGEX: &str = "Regular expression";
pub const SEARCH_PREV: &str = "Previous";
pub const SEARCH_NEXT: &str = "Next";
pub const SEARCH_CLOSE: &str = "Close";
pub const SETTINGS_APPEARANCE: &str = "Appearance";
pub const SETTINGS_TERMINAL: &str = "Terminal";
pub const SETTINGS_PROFILES: &str = "Profiles";
pub const SETTINGS_HOTKEYS: &str = "Keyboard shortcuts";
pub const SETTINGS_FONT: &str = "Font";
pub const SETTINGS_FONT_SIZE: &str = "Size";
pub const SETTINGS_FONT_REFRESH: &str = "Refresh font list";
pub const SETTINGS_SCHEME: &str = "Color scheme";
pub const SCHEME_DARK: &str = "Dark";
pub const SCHEME_LIGHT: &str = "Light";
pub const SETTINGS_SCROLLBACK: &str = "Scrollback (lines)";
pub const SETTINGS_CURSOR: &str = "Cursor";
pub const CURSOR_BLOCK: &str = "Block";
pub const CURSOR_BAR: &str = "Bar";
pub const CURSOR_UNDERLINE: &str = "Underline";
pub const SETTINGS_BLINK: &str = "Blink";
pub const SETTINGS_BELL: &str = "Bell";
pub const BELL_OFF: &str = "Off";
pub const BELL_VISUAL: &str = "Visual";
pub const SETTINGS_RIGHT_CLICK: &str = "Right click";
pub const RIGHT_CLICK_CLIPBOARD: &str = "Clipboard";
pub const RIGHT_CLICK_PASTE: &str = "Paste";
pub const RIGHT_CLICK_MENU: &str = "Menu";
pub const SETTINGS_MIDDLE_CLICK: &str = "Paste on middle click";
pub const SETTINGS_COPY_ON_SELECT: &str = "Copy on selection";
pub const SETTINGS_ALLOW_OSC52: &str = "Allow applications to change the clipboard (OSC 52)";
pub const SETTINGS_WORD_SEPARATORS: &str = "Word separators";
pub const SETTINGS_DEFAULT_PROFILE: &str = "Default profile";
pub const SETTINGS_ADD: &str = "Add";
pub const SETTINGS_EDIT: &str = "Edit";
pub const SETTINGS_DELETE: &str = "Delete";
pub const SETTINGS_SAVE: &str = "Save";
pub const SETTINGS_CANCEL: &str = "Cancel";
pub const SETTINGS_NAME: &str = "Name";
pub const SETTINGS_COMMAND: &str = "Command";
pub const SETTINGS_ARGS: &str = "Arguments";
pub const SETTINGS_CWD: &str = "Folder";
pub const SETTINGS_OPEN_CONFIG: &str = "Open config.json";
pub const SETTINGS_WORKSPACE: &str = "Git panel";
pub const SETTINGS_AI_COMMAND: &str = "AI commit command";
pub const SETTINGS_AI_HINT: &str = "claude / opencode / codex / gemini / aider; leave empty to detect the running CLI";
pub const SETTINGS_AI_MODEL: &str = "OpenCode commit model";
pub const SETTINGS_AI_MODEL_DEFAULT: &str = "OpenCode default";
pub const SETTINGS_AI_MODEL_SEARCH: &str = "Find a model, e.g. DeepSeek";
pub const SETTINGS_AI_MODEL_REFRESH: &str = "Refresh models";
pub const SETTINGS_AI_MODEL_LOADING: &str = "Loading models…";
pub const SETTINGS_AI_MODEL_HINT: &str =
    "Uses OpenCode and its configured providers. Set up credentials with opencode auth login.";
pub const SETTINGS_AI_MODEL_NO_MATCH: &str = "No models match this search.";
pub const SETTINGS_CLAUDE: &str = "Claude Code";
pub const SETTINGS_CLAUDE_ENABLED: &str = "ANVIL status line";
pub const SETTINGS_CLAUDE_ENABLED_HINT: &str =
    "Keeps your status command and adds data for ANVIL tabs. Disabling restores the previous command.";
pub const SETTINGS_CLAUDE_NOW: &str = "Current Claude Code status:";
pub const SETTINGS_CLAUDE_LINE_MISSING: &str = "no status line configured";
pub const SETTINGS_CLAUDE_LINE_ANVIL: &str = "ANVIL status line";
pub const SETTINGS_CLAUDE_LINE_FOREIGN: &str = "custom command —";
pub const SETTINGS_CLAUDE_LINE_BROKEN: &str = "settings.json is damaged:";
pub const SETTINGS_CLAUDE_BADGE: &str = "Show status under the tab";
pub const SETTINGS_CLAUDE_FIELDS: &str = "Fields";
pub const SETTINGS_CLAUDE_IN_CLAUDE: &str = "in Claude Code";
pub const SETTINGS_CLAUDE_UNDER_TAB: &str = "under the tab";
pub const SETTINGS_CLAUDE_FIELD_MODEL: &str = "Model";
pub const SETTINGS_CLAUDE_FIELD_DIR: &str = "Folder";
pub const SETTINGS_CLAUDE_FIELD_BRANCH: &str = "Branch";
pub const SETTINGS_CLAUDE_FIELD_CONTEXT: &str = "Context";
pub const SETTINGS_CLAUDE_FIELD_FIVE_HOUR: &str = "5h";
pub const SETTINGS_CLAUDE_FIELD_SEVEN_DAY: &str = "7d";
pub const SETTINGS_CLAUDE_FIELD_AGENT: &str = "Agent";
pub const SETTINGS_CLAUDE_FIELDS_NEED_ANVIL: &str = "Requires the ANVIL status line";
pub const SETTINGS_CLAUDE_DECLINED: &str = "Status integration was not installed: you chose to keep your command";
pub const SETTINGS_CLAUDE_PENDING: &str = "Integration enabled; installation requires confirmation";
pub const SETTINGS_CLAUDE_GLOBAL_HINT: &str =
    "Changes the global Claude Code status line. Disable before removing ANVIL; confirm the new path after moving it.";
pub const SETTINGS_CLAUDE_INSTALL: &str = "Install / update integration…";
pub const SETTINGS_CLAUDE_RESTORE: &str = "Retry restoring the status line";
pub const SETTINGS_NO_PROFILES: &str = "No custom profiles";
pub const SETTINGS_APPLY_HINT: &str = "Changes apply immediately and are saved to config.json";
pub const SETTINGS_HOTKEYS_HINT: &str =
    "Edit shortcuts in config.json: use English action names and hyphen-separated chords";
pub const SETTINGS_QUOTA: &str = "Quotas";
pub const SETTINGS_QUOTA_ENABLED: &str = "Show quotas";
pub const SETTINGS_QUOTA_INTERVAL: &str = "Background refresh every 30 seconds";
pub const SETTINGS_QUOTA_REFRESH: &str = "Refresh now";
pub const SETTINGS_QUOTA_NO_LOGIN: &str = "no login found";
pub const SETTINGS_QUOTA_SET_KEY: &str = "Set key…";
pub const SETTINGS_QUOTA_CHANGE_KEY: &str = "Key…";
pub const SETTINGS_QUOTA_DELETE_KEY: &str = "Delete key";
pub const SETTINGS_QUOTA_KEY_FAILED: &str = "Could not save the key to Windows Credential Manager";
pub const SETTINGS_QUOTA_KEY_DELETE_FAILED: &str = "Could not delete the key from Windows Credential Manager";
pub const QUOTA_REFRESH_DEFERRED: &str = "Refresh already requested — next cycle in {0} s";
pub const SETTINGS_QUOTA_KEY_INVALID: &str = "The key is empty, too long or contains line breaks";
pub const SETTINGS_QUOTA_WINDOWS_LATER: &str = "Windows appear after the first response";
pub const QUOTA_NO_DATA: &str = "quotas: no data yet";
pub const QUOTA_REFRESH_FALLBACK: &str = "refresh";
pub const QUOTA_REFRESH_HINT: &str = "Refresh quotas";
pub const QUOTA_LOGIN: &str = "login:";
pub const QUOTA_SOURCE_CLAUDE_CODE: &str = "Claude Code";
pub const QUOTA_SOURCE_CODEX: &str = "Codex CLI";
pub const QUOTA_SOURCE_OMP: &str = "OMP";
pub const QUOTA_SOURCE_OPENCODE: &str = "OpenCode";
pub const QUOTA_SOURCE_ANVIL_KEY: &str = "ANVIL key";
pub const QUOTA_SOURCE_ENV: &str = "environment";
pub const QUOTA_SOURCE_CLAUDE_SETTINGS: &str = "Claude Code settings";
pub const QUOTA_UNIT_DAY: &str = "d";
pub const QUOTA_UNIT_HOUR: &str = "h";
pub const QUOTA_UNIT_MINUTE: &str = "min";
pub const QUOTA_MONTH: &str = "mo";
pub const QUOTA_MCP: &str = "MCP";
pub const QUOTA_REVIEW: &str = "review";
pub const QUOTA_AUTH_EXPIRED: &str = "login expired";
pub const QUOTA_FORMAT_ERROR: &str = "invalid response format";
pub const QUOTA_STORE_UNREADABLE: &str = "cannot read local login; retrying in a minute";
pub const QUOTA_RESET_IN: &str = "resets in";
pub const QUOTA_DATA_AT: &str = "data from";
pub const QUOTA_CREDITS: &str = "credits";
pub const QUOTA_CREDITS_SHORT: &str = "cr.";
pub const QUOTA_BALANCE: &str = "balance";
pub const QUOTA_SPEND: &str = "spend";
pub const QUOTA_SESSION: &str = "session";
pub const QUOTA_KILO_PASS: &str = "Kilo Pass";
pub const QUOTA_TOPPED_UP: &str = "topped up";
pub const QUOTA_GRANTED: &str = "granted";
pub const QUOTA_CREDIT_LIMIT: &str = "credit limit";
pub const QUOTA_NO_SUBSCRIPTION: &str = "no subscription";
pub const QUOTA_ZEN_OTHER_SERVER: &str = "OpenCode login belongs to another console server";
pub const QUOTA_PROVIDER_SAID: &str = "provider response:";
pub const QUOTA_RATE_LIMITED: &str = "rate limited; retry at";
pub const SETTINGS_QUOTA_OWN_KEY: &str = "Custom key…";
pub const SETTINGS_QUOTA_ZEN_LOGIN: &str = "Log in to OpenCode: opencode auth login";
pub const CLAUDE_INSTALL_QUESTION: &str = "Install or update the Claude Code status line?";
pub const CLAUDE_INSTALL_GLOBAL: &str = "Changes global Claude Code settings for every terminal:";
pub const CLAUDE_CURRENT_COMMAND: &str = "Current command:";
pub const CLAUDE_NEW_COMMAND: &str = "New command:";
pub const CLAUDE_INSTALL_WARNING: &str = "Disabling ANVIL restores the previous line if it has not been changed. After moving or removing ANVIL, the command will stop working: disable integration before removing it or confirm the new path.";
pub const CLAUDE_INSTALL_ACCEPT: &str = "Install / update";
pub const CLAUDE_HELPER_MISSING: &str = "anvil-claude-status.exe not found; integration was not installed.";
pub const CLAUDE_DIRECTORY_CHANGED: &str =
    "Disable integration and restore the previous Claude Code settings first, then install it from the new folder.";
pub const PASTE_WARNING: &str = "Multiline paste can execute shell commands.";
pub const PASTE_PREVIEW_HINT: &str = "Bracketed paste is off. Nothing has been sent. Review the text before pasting:";
pub const PASTE_ACCEPT: &str = "Paste and allow execution";
pub const WORKSPACE_NO_REPO: &str = "not a repository";
pub const WORKSPACE_NO_REPO_HINT: &str = "This folder is not a Git repository.";
pub const WORKSPACE_CLOSE: &str = "Collapse panel";
pub const WORKSPACE_REFRESH: &str = "Refresh";
pub const WORKSPACE_STAGE: &str = "Stage";
pub const WORKSPACE_UNSTAGE: &str = "Unstage";
pub const WORKSPACE_STAGE_ALL: &str = "Stage all";
pub const WORKSPACE_UNSTAGE_ALL: &str = "Unstage all";
pub const WORKSPACE_COMMIT_HINT: &str = "Commit message (Ctrl+Enter)";
pub const WORKSPACE_COMMIT: &str = "Commit";
pub const WORKSPACE_AI: &str = "AI message";
pub const WORKSPACE_AI_GENERATING: &str = "Generating message…";
pub const WORKSPACE_DIFF_STAGED: &str = "Index / worktree";
pub const WORKSPACE_DIFF_LOADING: &str = "Loading diff…";
pub const WORKSPACE_TOGGLE_HINT: &str = "Git panel";
pub const WORKSPACE_TAB_CHANGES: &str = "Changes";
pub const WORKSPACE_TAB_FILES: &str = "Files";
pub const WORKSPACE_COMMITS_TITLE: &str = "COMMITS";
pub const WORKSPACE_PUBLISH: &str = "Publish";
pub const WORKSPACE_TRUNCATED: &str = "latest 80";
pub const WORKSPACE_NO_COMMITS: &str = "no commits";
pub const WORKSPACE_COUNT_LINES: &str = "Count lines";
pub const ZOOM_TOAST_PREFIX: &str = "Text size";
pub const WORKSPACE_SECTION_OUTGOING: &str = "OUTGOING";
pub const WORKSPACE_SECTION_INCOMING: &str = "INCOMING";
pub const WORKSPACE_SECTION_HISTORY: &str = "HISTORY";
pub const WORKSPACE_BACK: &str = "‹ back";
pub const WORKSPACE_COPY_HASH: &str = "Copy hash";
pub const WORKSPACE_COPY_PATCH: &str = "Copy patch";
pub const WORKSPACE_NEW_FILE: &str = "New file";
pub const WORKSPACE_NEW_FOLDER: &str = "New folder";
pub const WORKSPACE_RENAME: &str = "Rename";
pub const WORKSPACE_DELETE: &str = "Move to Recycle Bin";
pub const WORKSPACE_DELETE_HINT: &str = "The entire folder, including hidden and ignored files, will move to the Recycle Bin. Cancellation or failure leaves it in place.";
pub const WORKSPACE_DELETE_FILE_HINT: &str =
    "The file will move to the Windows Recycle Bin. Cancellation or failure leaves it in place.";
pub const WORKSPACE_OPEN_EXTERNAL: &str = "Open in system";
pub const WORKSPACE_REVEAL: &str = "Show in Explorer";
pub const WORKSPACE_FILE_FILTER: &str = "Filter files";
pub const WORKSPACE_FILE_TRUNCATED: &str = "… file preview is truncated";
pub const WORKSPACE_PUSH_HINT: &str = "Push to the configured remote";
pub const WORKSPACE_FETCH_HINT: &str = "Fetch from the configured remote";
pub const WORKSPACE_BRANCH_HINT: &str = "Switch branch";
pub const WORKSPACE_BRANCH_FILTER: &str = "Filter branches";
pub const WORKSPACE_BRANCHES_LOADING: &str = "Loading branches…";
pub const WORKSPACE_BRANCH_NEW: &str = "New branch";
pub const WORKSPACE_BRANCH_CREATE: &str = "Create and switch";
pub const WORKSPACE_BRANCH_INVALID: &str = "invalid branch name";
pub const WORKSPACE_BRANCH_REMOTE_HINT: &str = "A local branch that tracks this remote one is created and checked out";
pub const WORKSPACE_NO_BRANCH: &str = "no current branch";
pub const WORKSPACE_NO_AI_COMMAND: &str = "no AI command selected";
pub const WORKSPACE_AI_EMPTY: &str = "CLI returned an empty message";
pub const WORKSPACE_NO_CHANGES_FOR_FILE: &str = "no changes for this file";
pub const WORKSPACE_PATH_INSIDE_REPO: &str = "path must be inside the repository";
pub const WORKSPACE_FILE_EXISTS: &str = "file already exists";
pub const WORKSPACE_NO_SUCH_FILE: &str = "file not found";
pub const WORKSPACE_WORKER_LOST: &str = "panel worker stopped — panel restarted";
pub const WORKSPACE_DIFF_STALE: &str = "file changed — refresh the panel";
pub const WORKSPACE_REPO_CHANGED: &str = "repository changed — action cancelled";
pub const WORKSPACE_TRUST_TITLE: &str = "Repository trust required";
pub const WORKSPACE_TRUST_HINT: &str = "This repository's configuration can run programs (filters, hooks, external diff drivers and signing programs). Ordinary settings such as branches, remotes and editors do not themselves require trust. Until you confirm, ANVIL does not read status, diffs or history, or perform Git actions. Permission lasts for this session, this repository and only the configuration shown; changes revoke it and keep your commit draft.";
pub const WORKSPACE_TRUST_HAZARDS: &str = "May run programs:";
pub const WORKSPACE_TRUST_APPROVE: &str = "Trust this configuration";
pub const WORKSPACE_TRUST_STALE: &str =
    "Repository or configuration changed — permission revoked. Check the folder and confirm again.";
pub const WORKSPACE_AI_NO_STAGE: &str = "Stage changes first: AI generates a message only from staged changes.";
pub const PICKER_FILTER: &str = "Filter";
pub const PICKER_EMPTY: &str = "Nothing found";
pub const COLLAPSED_HINT: &str = "Enter — restore pane · Esc — close";
pub const COLLAPSED_EMPTY: &str = "No collapsed panes. Ctrl+Alt+C hides a pane";
pub const TAB_COLLAPSED: &str = "Collapsed panes";
pub const WORKSPACE_PREVIEW_CLOSE: &str = "Close preview";
/// Message shown in a pane whose process exited with a non-zero code.
pub fn process_exited(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("[process exited with code {code} — press any key]"),
        None => "[process exited — press any key]".to_owned(),
    }
}

pub fn workspace_changes_tab(count: usize) -> String {
    if count == 0 {
        WORKSPACE_TAB_CHANGES.to_owned()
    } else {
        format!("{WORKSPACE_TAB_CHANGES} {count}")
    }
}
pub fn workspace_line_count(files: usize, lines: u64) -> String {
    format!("{lines} lines · {files} files")
}
pub fn zoom_toast(size: f32) -> String {
    format!("{ZOOM_TOAST_PREFIX}: {size:.0}")
}
pub fn relative_time(now_secs: i64, then_secs: i64) -> String {
    let delta = now_secs.saturating_sub(then_secs).max(0);
    match delta {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{} min ago", delta / 60),
        3600..=86_399 => format!("{} h ago", delta / 3600),
        86_400..=2_591_999 => format!("{} d ago", delta / 86_400),
        2_592_000..=31_535_999 => format!("{} mo ago", delta / 2_592_000),
        _ => format!("{} y ago", delta / 31_536_000),
    }
}
pub fn workspace_pushed(branch: &str) -> String {
    format!("push {branch} — done")
}
pub fn workspace_switched(branch: &str) -> String {
    format!("switched to {branch}")
}
pub fn workspace_fetch_done(what: &str) -> String {
    format!("{what} — done")
}
pub fn workspace_staged(staged: usize, unstaged: usize) -> String {
    format!("staged {staged} · changed {unstaged}")
}
pub fn workspace_committed(hash: &str) -> String {
    format!("Commit {hash}")
}
pub fn workspace_ai(command: &str) -> String {
    format!("AI: {command}")
}
pub fn hotkey_label(action: &crate::hotkeys::Action) -> String {
    use crate::hotkeys::Action as A;
    match action {
        A::Tab(n) => return format!("Tab {n}"),
        A::Profile(id) => return format!("Profile: {id}"),
        A::NewTab => "New tab",
        A::NewWindow => "New window",
        A::CloseTab => "Close tab",
        A::ReopenTab => "Reopen tab",
        A::RenameTab => "Rename tab",
        A::NextTab => "Next tab",
        A::PreviousTab => "Previous tab",
        A::MoveTabLeft => "Move tab left",
        A::MoveTabRight => "Move tab right",
        A::SplitRight => "Split right",
        A::SplitBottom => "Split down",
        A::PaneNavLeft => "Pane: left",
        A::PaneNavRight => "Pane: right",
        A::PaneNavUp => "Pane: up",
        A::PaneNavDown => "Pane: down",
        A::PaneNavPrevious => "Pane: previous",
        A::PaneNavNext => "Pane: next",
        A::PaneMaximize => "Maximize pane",
        A::ClosePane => "Close pane",
        A::PaneCollapse => "Collapse pane",
        A::PaneRestore => "Restore pane",
        A::CollapsedList => "Collapsed panes",
        A::ProfileSelector => "Choose profile",
        A::Settings => "Settings",
        A::ToggleFullscreen => "Fullscreen",
        A::ToggleFrameStats => "Frame time (debug)",
        A::CtrlC => "Interrupt (Ctrl+C)",
        A::Copy => "Copy",
        A::Paste => "Paste",
        A::SelectAll => "Select all",
        A::Clear => "Clear",
        A::ZoomIn => "Zoom in",
        A::ZoomOut => "Zoom out",
        A::ResetZoom => "Reset zoom",
        A::PreviousWord => "Previous word",
        A::NextWord => "Next word",
        A::DeletePreviousWord => "Delete previous word",
        A::DeleteNextWord => "Delete next word",
        A::DeleteLine => "Delete line",
        A::Search => "Search",
        A::ToggleWorkspace => "Git panel",
        A::ScrollToTop => "Scroll to top",
        A::ScrollToBottom => "Scroll to bottom",
        A::ScrollPageUp => "Page up",
        A::ScrollPageDown => "Page down",
        A::ScrollUp => "Scroll up",
        A::ScrollDown => "Scroll down",
    }
    .to_owned()
}
