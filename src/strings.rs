//! Every user-visible string. Russian only for now; keep them here so the UI
//! can be translated later without hunting through the code.

pub const APP_TITLE: &str = "ANVIL";
pub const COPIED: &str = "Скопировано";
pub const CONFIG_BROKEN: &str = "config.json повреждён — загружены настройки по умолчанию, старый файл сохранён";
pub const CONFIG_UNREADABLE: &str = "Не удалось прочитать config.json — загружены настройки по умолчанию";
pub const UNKNOWN_HOTKEYS: &str = "В config.json есть неизвестные горячие клавиши";
pub const CONPTY_MISSING: &str = "Не найден conpty.dll — возможны артефакты ввода в omp и opencode";
pub const CLAUDE_SETTINGS_BROKEN: &str = "settings.json Claude Code повреждён — статус не подключён";
pub const CLAUDE_SETTINGS_CHANGED: &str = "settings.json изменился во время настройки — статус не подключён";
pub const BELL: &str = "Сигнал";
pub const WINDOW_MINIMIZE: &str = "Свернуть";
pub const WINDOW_MAXIMIZE: &str = "Развернуть / восстановить";
pub const WINDOW_CLOSE: &str = "Закрыть";

// Tabs.
pub const TAB_RENAME: &str = "Переименовать";
pub const TAB_DUPLICATE: &str = "Дублировать";
pub const TAB_CLOSE: &str = "Закрыть";
pub const TAB_CLOSE_OTHERS: &str = "Закрыть остальные";
pub const TAB_NEW: &str = "Новая вкладка";
pub const TAB_PROFILES: &str = "Профили";
pub const TAB_SETTINGS: &str = "Настройки";

// Pane context menu.
pub const MENU_COPY: &str = "Копировать";
pub const MENU_PASTE: &str = "Вставить";
pub const MENU_SELECT_ALL: &str = "Выделить всё";
pub const MENU_CLEAR: &str = "Очистить";
pub const MENU_SPLIT_RIGHT: &str = "Сплит вправо";
pub const MENU_SPLIT_DOWN: &str = "Сплит вниз";
pub const MENU_CLOSE_PANE: &str = "Закрыть панель";
pub const PANE_CLOSE: &str = "Закрыть";
pub const SPAWN_FAILED: &str = "Не удалось запустить программу профиля";

/// Message shown in a pane whose process exited with a non-zero code.
pub fn process_exited(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("[процесс завершён с кодом {code} — нажмите любую клавишу]"),
        None => "[процесс завершён — нажмите любую клавишу]".to_owned(),
    }
}

// Search bar.
pub const SEARCH_PLACEHOLDER: &str = "Поиск";
pub const SEARCH_CASE: &str = "Учитывать регистр";
pub const SEARCH_REGEX: &str = "Регулярное выражение";
pub const SEARCH_PREV: &str = "Предыдущее";
pub const SEARCH_NEXT: &str = "Следующее";
pub const SEARCH_CLOSE: &str = "Закрыть";

// Settings page.
pub const SETTINGS_APPEARANCE: &str = "Внешний вид";
pub const SETTINGS_TERMINAL: &str = "Терминал";
pub const SETTINGS_PROFILES: &str = "Профили";
pub const SETTINGS_HOTKEYS: &str = "Горячие клавиши";
pub const SETTINGS_FONT: &str = "Шрифт";
pub const SETTINGS_FONT_SIZE: &str = "Размер";
pub const SETTINGS_SCHEME: &str = "Цветовая схема";
pub const SETTINGS_SCROLLBACK: &str = "История (строк)";
pub const SETTINGS_CURSOR: &str = "Курсор";
pub const CURSOR_BLOCK: &str = "Блок";
pub const CURSOR_BAR: &str = "Черта";
pub const CURSOR_UNDERLINE: &str = "Подчёркивание";
pub const SETTINGS_BLINK: &str = "Мигание";
pub const SETTINGS_RIGHT_CLICK: &str = "Правая кнопка";
pub const RIGHT_CLICK_CLIPBOARD: &str = "Буфер обмена";
pub const RIGHT_CLICK_PASTE: &str = "Вставка";
pub const RIGHT_CLICK_MENU: &str = "Меню";
pub const SETTINGS_MIDDLE_CLICK: &str = "Вставка средней кнопкой";
pub const SETTINGS_COPY_ON_SELECT: &str = "Копировать при выделении";
pub const SETTINGS_WORD_SEPARATORS: &str = "Разделители слов";
pub const SETTINGS_DEFAULT_PROFILE: &str = "Профиль по умолчанию";
pub const SETTINGS_ADD: &str = "Добавить";
pub const SETTINGS_EDIT: &str = "Изменить";
pub const SETTINGS_DELETE: &str = "Удалить";
pub const SETTINGS_SAVE: &str = "Сохранить";
pub const SETTINGS_CANCEL: &str = "Отмена";
pub const SETTINGS_NAME: &str = "Имя";
pub const SETTINGS_COMMAND: &str = "Команда";
pub const SETTINGS_ARGS: &str = "Аргументы";
pub const SETTINGS_CWD: &str = "Папка";
pub const SETTINGS_OPEN_CONFIG: &str = "Открыть config.json";
pub const SETTINGS_WORKSPACE: &str = "Панель git";
pub const SETTINGS_AI_COMMAND: &str = "AI-команда коммита";
pub const SETTINGS_AI_HINT: &str = "claude / opencode / codex / gemini / aider, пусто — определить из процесса";
pub const SETTINGS_CLAUDE: &str = "Claude Code";
pub const SETTINGS_CLAUDE_ENABLED: &str = "Статус Claude Code";
pub const SETTINGS_CLAUDE_CONNECTED: &str = "Подключён";
pub const SETTINGS_CLAUDE_NOT_CONNECTED: &str = "Не подключён";
pub const SETTINGS_CLAUDE_DECLINED: &str = "Строка статуса не подключена: вы выбрали оставить свою команду";
pub const SETTINGS_CUSTOM_PROFILE: &str = "Пользовательский профиль";
pub const SETTINGS_NO_PROFILES: &str = "Пользовательских профилей нет";
pub const SETTINGS_APPLY_HINT: &str = "Изменения применяются сразу и сохраняются в config.json";
pub const SETTINGS_HOTKEYS_HINT: &str = "Редактирование клавиш — только в config.json";

// Claude Code status line setup dialog.
pub fn claude_replace_question(command: &str) -> String {
    format!("Строка статуса Claude Code сейчас: `{command}`. Заменить на встроенную в ANVIL? Вывод тот же, Node не нужен.")
}
pub const CLAUDE_REPLACE: &str = "Заменить";
pub const CLAUDE_KEEP: &str = "Оставить";

// Workspace panel (git).
pub const WORKSPACE_NO_REPO: &str = "не репозиторий";
pub const WORKSPACE_NO_REPO_HINT: &str = "В этой папке нет git-репозитория.";
pub const WORKSPACE_CLOSE: &str = "Свернуть панель";
pub const WORKSPACE_REFRESH: &str = "Обновить";
pub const WORKSPACE_STAGE: &str = "Стейджить";
pub const WORKSPACE_UNSTAGE: &str = "Убрать";
pub const WORKSPACE_STAGE_ALL: &str = "Стейджить всё";
pub const WORKSPACE_UNSTAGE_ALL: &str = "Убрать всё";
pub const WORKSPACE_DIFF_SIDE: &str = "Индекс / рабочее дерево";
pub const WORKSPACE_COMMIT_HINT: &str = "Сообщение коммита (Ctrl+Enter)";
pub const WORKSPACE_COMMIT: &str = "Зафиксировать";
pub const WORKSPACE_AI: &str = "AI-сообщение";
pub const WORKSPACE_DIFF_STAGED: &str = "Индекс/дерево";
pub const WORKSPACE_TOGGLE_HINT: &str = "Панель git";
pub const WORKSPACE_TAB_CHANGES: &str = "Изменения";
pub const WORKSPACE_TAB_COMMITS: &str = "коммиты";
pub const WORKSPACE_TAB_FILES: &str = "Файлы";
pub const WORKSPACE_COMMITS_TITLE: &str = "КОММИТЫ";
pub const WORKSPACE_PUBLISH: &str = "Опубликовать";
pub const WORKSPACE_TRUNCATED: &str = "последние 80";
pub const WORKSPACE_NO_COMMITS: &str = "коммитов нет";

pub fn workspace_changes_tab(count: usize) -> String {
    if count == 0 {
        WORKSPACE_TAB_CHANGES.to_owned()
    } else {
        format!("{WORKSPACE_TAB_CHANGES} {count}")
    }
}
pub const WORKSPACE_NO_UPSTREAM: &str = "нет upstream";
/// "Посчитать строки" toolbar button.
pub const WORKSPACE_COUNT_LINES: &str = "Посчитать строки";
/// Result of the line count: files and total lines.
pub fn workspace_line_count(files: usize, lines: u64) -> String {
    format!("{lines} строк · {files} файлов")
}
/// Toast after Ctrl+= / Ctrl+- / Ctrl+0; steps of one zoom collapse into one.
pub const ZOOM_TOAST_PREFIX: &str = "Размер текста";
pub fn zoom_toast(size: f32) -> String {
    format!("{ZOOM_TOAST_PREFIX}: {size:.0}")
}
pub const WORKSPACE_SECTION_OUTGOING: &str = "ИСХОДЯЩИЕ";
pub const WORKSPACE_SECTION_INCOMING: &str = "ВХОДЯЩИЕ";
pub const WORKSPACE_SECTION_HISTORY: &str = "ИСТОРИЯ";
pub const WORKSPACE_BACK: &str = "‹ назад";
pub const WORKSPACE_COPY_HASH: &str = "Копировать хеш";
pub const WORKSPACE_COPY_PATCH: &str = "Копировать патч";
pub const WORKSPACE_NEW_FILE: &str = "Новый файл";
pub const WORKSPACE_NEW_FOLDER: &str = "Новая папка";
pub const WORKSPACE_RENAME: &str = "Переименовать";
pub const WORKSPACE_DELETE: &str = "Удалить";
pub const WORKSPACE_OPEN_EXTERNAL: &str = "Открыть в системе";
pub const WORKSPACE_REVEAL: &str = "Показать в проводнике";
pub const WORKSPACE_FILE_FILTER: &str = "Фильтр файлов";
pub const WORKSPACE_FILE_TRUNCATED: &str = "… файл показан не полностью";
pub const WORKSPACE_PUSH_HINT: &str = "Отправить в origin";
pub const WORKSPACE_FETCH_HINT: &str = "Забрать изменения из origin";

pub const WORKSPACE_NO_BRANCH: &str = "нет текущей ветки";
pub const WORKSPACE_NO_AI_COMMAND: &str = "не выбрана AI-команда";
pub const WORKSPACE_AI_EMPTY: &str = "CLI вернул пустое сообщение";
pub const WORKSPACE_NO_CHANGES_FOR_FILE: &str = "нет изменений для этого файла";
pub const WORKSPACE_PATH_INSIDE_REPO: &str = "нужен путь внутри репозитория";
pub const WORKSPACE_FILE_EXISTS: &str = "файл уже существует";
pub const WORKSPACE_NO_SUCH_FILE: &str = "нет такого файла";
pub const WORKSPACE_DIFF_STALE: &str = "файл изменился — обновите панель";

/// Russian relative time of a unix timestamp, as the commit list shows it.
pub fn relative_time(now_secs: i64, then_secs: i64) -> String {
    let delta = (now_secs - then_secs).max(0);
    match delta {
        0..=59 => "только что".to_owned(),
        60..=3599 => format!("{} мин назад", delta / 60),
        3600..=86_399 => format!("{} ч назад", delta / 3600),
        86_400..=2_591_999 => format!("{} дн назад", delta / 86_400),
        2_592_000..=31_535_999 => format!("{} мес назад", delta / 2_592_000),
        _ => format!("{} г назад", delta / 31_536_000),
    }
}

pub fn workspace_pushed(branch: &str) -> String {
    format!("push {branch} — готово")
}

pub fn workspace_fetch_done(what: &str) -> String {
    format!("{what} — готово")
}

pub fn workspace_staged(staged: usize, unstaged: usize) -> String {
    format!("в индексе {staged} · изменено {unstaged}")
}

pub fn workspace_committed(hash: &str) -> String {
    format!("Коммит {hash}")
}

pub fn workspace_ai(command: &str) -> String {
    format!("AI: {command}")
}

// Profile picker.
pub const PICKER_FILTER: &str = "Фильтр";
pub const PICKER_EMPTY: &str = "Ничего не найдено";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_time_is_russian() {
        let now = 1_800_000_000;
        assert_eq!(relative_time(now, now), "только что");
        assert_eq!(relative_time(now, now - 120), "2 мин назад");
        assert_eq!(relative_time(now, now - 7200), "2 ч назад");
        assert_eq!(relative_time(now, now - 3 * 86_400), "3 дн назад");
        assert_eq!(relative_time(now, now - 60 * 86_400), "2 мес назад");
        assert_eq!(relative_time(now, now - 800 * 86_400), "2 г назад");
    }
}
