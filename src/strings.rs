//! Every user-visible string. Russian only for now; keep them here so the UI
//! can be translated later without hunting through the code.

pub const APP_TITLE: &str = "ANVIL";
pub const COPIED: &str = "Скопировано";
pub const CONFIG_BROKEN: &str = "config.json повреждён — загружены настройки по умолчанию, старый файл сохранён";
pub const CONFIG_UNREADABLE: &str = "Не удалось прочитать config.json — загружены настройки по умолчанию";
pub const UNKNOWN_HOTKEYS: &str = "В config.json есть неизвестные горячие клавиши";
pub const CONPTY_MISSING: &str = "Не найден conpty.dll — возможны артефакты ввода в omp и opencode";
pub const CLAUDE_SETTINGS_BROKEN: &str = "settings.json Claude Code повреждён — статус не подключён";
pub const BELL: &str = "Сигнал";

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
pub const SETTINGS_CLAUDE: &str = "Claude Code";
pub const SETTINGS_CLAUDE_ENABLED: &str = "Статус Claude Code";
pub const SETTINGS_CLAUDE_CONNECTED: &str = "Подключён";
pub const SETTINGS_CLAUDE_NOT_CONNECTED: &str = "Не подключён";
pub const SETTINGS_CLAUDE_DECLINED: &str = "Строка статуса не подключена: вы выбрали оставить свою команду";
pub const SETTINGS_CUSTOM_PROFILE: &str = "Пользовательский профиль";
pub const SETTINGS_APPLY_HINT: &str = "Изменения применяются сразу и сохраняются в config.json";
pub const SETTINGS_HOTKEYS_HINT: &str = "Редактирование клавиш — только в config.json";

// Claude Code status line setup dialog.
pub fn claude_replace_question(command: &str) -> String {
    format!("Строка статуса Claude Code сейчас: `{command}`. Заменить на встроенную в ANVIL? Вывод тот же, Node не нужен.")
}
pub const CLAUDE_REPLACE: &str = "Заменить";
pub const CLAUDE_KEEP: &str = "Оставить";

// Profile picker.
pub const PICKER_FILTER: &str = "Фильтр";
pub const PICKER_EMPTY: &str = "Ничего не найдено";
