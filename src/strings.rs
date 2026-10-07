//! Every user-visible string. Russian only for now; keep them here so the UI
//! can be translated later without hunting through the code.

pub const APP_TITLE: &str = "ANVIL";
pub const COPIED: &str = "Скопировано";
pub const CONFIG_BROKEN: &str = "config.json повреждён — загружены настройки по умолчанию, старый файл сохранён";
pub const CONFIG_UNREADABLE: &str = "Не удалось прочитать config.json — загружены настройки по умолчанию";
pub const UNKNOWN_HOTKEYS: &str = "В config.json есть ошибки или конфликты горячих клавиш";
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
pub const TAB_COLOR: &str = "Цвет";
pub const TAB_COLOR_NONE: &str = "Без цвета";
pub const TAB_COLOR_BLUE: &str = "Синий";
pub const TAB_COLOR_GREEN: &str = "Зелёный";
pub const TAB_COLOR_ORANGE: &str = "Оранжевый";
pub const TAB_COLOR_PURPLE: &str = "Фиолетовый";
pub const TAB_COLOR_RED: &str = "Красный";
pub const TAB_COLOR_YELLOW: &str = "Жёлтый";
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
pub const PANE_REARRANGE_HINT: &str = "Удерживайте Ctrl+Shift и перетащите панель к краю другой панели или вкладки";
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
pub const SETTINGS_FONT_REFRESH: &str = "Обновить список шрифтов";
pub const SETTINGS_SCHEME: &str = "Цветовая схема";
/// The two built-in colour schemes, by how the window reads: the owner's dark
/// design and the light one.
pub const SCHEME_DARK: &str = "Тёмная";
pub const SCHEME_LIGHT: &str = "Светлая";
pub const SETTINGS_SCROLLBACK: &str = "История (строк)";
pub const SETTINGS_CURSOR: &str = "Курсор";
pub const CURSOR_BLOCK: &str = "Блок";
pub const CURSOR_BAR: &str = "Черта";
pub const CURSOR_UNDERLINE: &str = "Подчёркивание";
pub const SETTINGS_BLINK: &str = "Мигание";
pub const SETTINGS_BELL: &str = "Сигнал";
pub const BELL_OFF: &str = "Выкл";
pub const BELL_VISUAL: &str = "Визуальный";
pub const SETTINGS_RIGHT_CLICK: &str = "Правая кнопка";
pub const RIGHT_CLICK_CLIPBOARD: &str = "Буфер обмена";
pub const RIGHT_CLICK_PASTE: &str = "Вставка";
pub const RIGHT_CLICK_MENU: &str = "Меню";
pub const SETTINGS_MIDDLE_CLICK: &str = "Вставка средней кнопкой";
pub const SETTINGS_COPY_ON_SELECT: &str = "Копировать при выделении";
pub const SETTINGS_ALLOW_OSC52: &str = "Разрешить приложениям менять буфер обмена (OSC 52)";
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
pub const SETTINGS_AI_MODEL: &str = "Модель OpenCode для коммитов";
pub const SETTINGS_AI_MODEL_DEFAULT: &str = "По умолчанию в OpenCode";
pub const SETTINGS_AI_MODEL_SEARCH: &str = "Поиск модели, например DeepSeek";
pub const SETTINGS_AI_MODEL_REFRESH: &str = "Обновить модели";
pub const SETTINGS_AI_MODEL_LOADING: &str = "Загрузка моделей…";
pub const SETTINGS_AI_MODEL_HINT: &str =
    "Выбор модели использует OpenCode и его подключённые провайдеры. Ключи — через opencode auth login.";
pub const SETTINGS_AI_MODEL_NO_MATCH: &str = "Модели не найдены по этому запросу.";
pub const SETTINGS_CLAUDE: &str = "Claude Code";
pub const SETTINGS_CLAUDE_ENABLED: &str = "Строка статуса от ANVIL";
pub const SETTINGS_CLAUDE_ENABLED_HINT: &str =
    "Та же строка, что у вашей команды, плюс данные для вкладок ANVIL. При выключении вернётся прежняя команда.";
pub const SETTINGS_CLAUDE_NOW: &str = "Сейчас в Claude Code:";
pub const SETTINGS_CLAUDE_LINE_MISSING: &str = "строка статуса не задана";
pub const SETTINGS_CLAUDE_LINE_ANVIL: &str = "строка статуса от ANVIL";
pub const SETTINGS_CLAUDE_LINE_FOREIGN: &str = "своя команда —";
pub const SETTINGS_CLAUDE_LINE_BROKEN: &str = "settings.json повреждён:";
pub const SETTINGS_CLAUDE_BADGE: &str = "Показывать строку под вкладкой";
pub const SETTINGS_CLAUDE_FIELDS: &str = "Данные";
pub const SETTINGS_CLAUDE_IN_CLAUDE: &str = "в Claude Code";
pub const SETTINGS_CLAUDE_UNDER_TAB: &str = "под вкладкой";
pub const SETTINGS_CLAUDE_FIELD_MODEL: &str = "Модель";
pub const SETTINGS_CLAUDE_FIELD_DIR: &str = "Папка";
pub const SETTINGS_CLAUDE_FIELD_BRANCH: &str = "Ветка";
pub const SETTINGS_CLAUDE_FIELD_CONTEXT: &str = "Контекст";
pub const SETTINGS_CLAUDE_FIELD_FIVE_HOUR: &str = "5ч";
pub const SETTINGS_CLAUDE_FIELD_SEVEN_DAY: &str = "7д";
pub const SETTINGS_CLAUDE_FIELD_AGENT: &str = "Агент";
pub const SETTINGS_CLAUDE_FIELDS_NEED_ANVIL: &str = "Работает, когда строка статуса от ANVIL";
pub const SETTINGS_CLAUDE_DECLINED: &str = "Строка статуса не подключена: вы выбрали оставить свою команду";
pub const SETTINGS_CLAUDE_PENDING: &str = "Интеграция включена; установка требует подтверждения";
pub const SETTINGS_CLAUDE_GLOBAL_HINT: &str = "Установка меняет глобальную строку статуса Claude Code. Отключите интеграцию перед удалением ANVIL; после перемещения подтвердите новый путь.";
pub const SETTINGS_CLAUDE_INSTALL: &str = "Установить / обновить интеграцию…";
pub const SETTINGS_CLAUDE_RESTORE: &str = "Повторить восстановление строки статуса";
pub const SETTINGS_NO_PROFILES: &str = "Пользовательских профилей нет";
pub const SETTINGS_APPLY_HINT: &str = "Изменения применяются сразу и сохраняются в config.json";
pub const SETTINGS_HOTKEYS_HINT: &str =
    "Редактирование клавиш — только в config.json: имена и аккорды там пишутся по-английски, через дефис";

// Quotas.
pub const SETTINGS_QUOTA: &str = "Квоты";
pub const SETTINGS_QUOTA_ENABLED: &str = "Показывать квоты";
pub const SETTINGS_QUOTA_INTERVAL: &str = "Фоновое обновление раз в 30 секунд";
pub const SETTINGS_QUOTA_REFRESH: &str = "Обновить сейчас";
pub const SETTINGS_QUOTA_NO_LOGIN: &str = "вход не найден";
pub const SETTINGS_QUOTA_SET_KEY: &str = "Задать ключ…";
pub const SETTINGS_QUOTA_CHANGE_KEY: &str = "Ключ…";
pub const SETTINGS_QUOTA_DELETE_KEY: &str = "Удалить ключ";
pub const SETTINGS_QUOTA_KEY_FAILED: &str = "Не удалось сохранить ключ в Диспетчере учётных данных Windows";
pub const SETTINGS_QUOTA_KEY_DELETE_FAILED: &str = "Не удалось удалить ключ из Диспетчера учётных данных Windows";
/// A refresh asked for inside the anti-hammer gap is served after a wait; the
/// click has to say so instead of looking like a button that did nothing.
pub const QUOTA_REFRESH_DEFERRED: &str = "Обновление уже запрошено — следующий цикл через {0} с";
pub const SETTINGS_QUOTA_KEY_INVALID: &str = "Ключ пуст, слишком длинный или содержит переводы строк";
pub const SETTINGS_QUOTA_WINDOWS_LATER: &str = "окна появятся после первого ответа";
pub const QUOTA_NO_DATA: &str = "квоты: данных пока нет";
pub const QUOTA_REFRESH_FALLBACK: &str = "обн";
pub const QUOTA_REFRESH_HINT: &str = "Обновить квоты";
pub const QUOTA_LOGIN: &str = "вход:";
pub const QUOTA_SOURCE_CLAUDE_CODE: &str = "Claude Code";
pub const QUOTA_SOURCE_CODEX: &str = "Codex CLI";
pub const QUOTA_SOURCE_OMP: &str = "OMP";
pub const QUOTA_SOURCE_OPENCODE: &str = "OpenCode";
pub const QUOTA_SOURCE_ANVIL_KEY: &str = "ключ ANVIL";
pub const QUOTA_SOURCE_ENV: &str = "переменная";
pub const QUOTA_SOURCE_CLAUDE_SETTINGS: &str = "настройки Claude Code";
pub const QUOTA_UNIT_DAY: &str = "д";
pub const QUOTA_UNIT_HOUR: &str = "ч";
pub const QUOTA_UNIT_MINUTE: &str = "мин";
pub const QUOTA_MONTH: &str = "мес";
pub const QUOTA_MCP: &str = "MCP";
pub const QUOTA_REVIEW: &str = "ревью";
pub const QUOTA_AUTH_EXPIRED: &str = "вход устарел";
pub const QUOTA_FORMAT_ERROR: &str = "ошибка формата";
pub const QUOTA_STORE_UNREADABLE: &str = "не удалось прочитать локальный вход, повтор через минуту";
pub const QUOTA_RESET_IN: &str = "сброс через";
pub const QUOTA_DATA_AT: &str = "данные от";

// Quotas: balances and the bottom line.
pub const QUOTA_CREDITS: &str = "кредиты";
pub const QUOTA_CREDITS_SHORT: &str = "кр.";
pub const QUOTA_BALANCE: &str = "баланс";
pub const QUOTA_SPEND: &str = "траты";
pub const QUOTA_SESSION: &str = "сессия";
pub const QUOTA_KILO_PASS: &str = "Kilo Pass";
pub const QUOTA_TOPPED_UP: &str = "пополнено";
pub const QUOTA_GRANTED: &str = "подарено";
pub const QUOTA_CREDIT_LIMIT: &str = "кредитный лимит";
pub const QUOTA_NO_SUBSCRIPTION: &str = "нет подписки";
pub const QUOTA_ZEN_OTHER_SERVER: &str = "вход OpenCode относится к другому серверу консоли";
pub const QUOTA_PROVIDER_SAID: &str = "ответ провайдера:";
pub const QUOTA_RATE_LIMITED: &str = "лимит запросов, повтор в";
pub const SETTINGS_QUOTA_OWN_KEY: &str = "Свой ключ…";
pub const SETTINGS_QUOTA_ZEN_LOGIN: &str = "войдите в OpenCode: opencode auth login";

// Explicit consent for global Claude settings and executable paste.
pub const CLAUDE_INSTALL_QUESTION: &str = "Установить или обновить строку статуса Claude Code?";
pub const CLAUDE_INSTALL_GLOBAL: &str = "Это меняет глобальные настройки Claude Code для всех терминалов:";
pub const CLAUDE_CURRENT_COMMAND: &str = "Текущая команда:";
pub const CLAUDE_NEW_COMMAND: &str = "Новая команда:";
pub const CLAUDE_INSTALL_WARNING: &str = "При отключении ANVIL восстановит предыдущую строку, если её не изменили. После перемещения или удаления ANVIL команда перестанет работать: отключите интеграцию до удаления или подтвердите обновление пути.";
pub const CLAUDE_INSTALL_ACCEPT: &str = "Установить / обновить";
pub const CLAUDE_HELPER_MISSING: &str = "Не найден anvil-claude-status.exe; интеграция не установлена.";
pub const CLAUDE_DIRECTORY_CHANGED: &str =
    "Сначала отключите интеграцию и восстановите прежние настройки Claude Code, затем установите её в новом каталоге.";
pub const PASTE_WARNING: &str = "Многострочная вставка может выполнить команды в оболочке.";
pub const PASTE_PREVIEW_HINT: &str = "Bracketed paste отключён. Ничего не отправлено. Проверьте текст перед вставкой:";
pub const PASTE_ACCEPT: &str = "Вставить и разрешить выполнение";

// Workspace panel (git).
pub const WORKSPACE_NO_REPO: &str = "не репозиторий";
pub const WORKSPACE_NO_REPO_HINT: &str = "В этой папке нет git-репозитория.";
pub const WORKSPACE_CLOSE: &str = "Свернуть панель";
pub const WORKSPACE_REFRESH: &str = "Обновить";
pub const WORKSPACE_STAGE: &str = "Стейджить";
pub const WORKSPACE_UNSTAGE: &str = "Убрать";
pub const WORKSPACE_STAGE_ALL: &str = "Стейджить всё";
pub const WORKSPACE_UNSTAGE_ALL: &str = "Убрать всё";
pub const WORKSPACE_COMMIT_HINT: &str = "Сообщение коммита (Ctrl+Enter)";
pub const WORKSPACE_COMMIT: &str = "Зафиксировать";
pub const WORKSPACE_AI: &str = "AI-сообщение";
pub const WORKSPACE_AI_GENERATING: &str = "Генерация сообщения…";
pub const WORKSPACE_DIFF_STAGED: &str = "Индекс/дерево";
pub const WORKSPACE_DIFF_LOADING: &str = "Загрузка diff…";
pub const WORKSPACE_TOGGLE_HINT: &str = "Панель git";
pub const WORKSPACE_TAB_CHANGES: &str = "Изменения";
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
pub const WORKSPACE_DELETE: &str = "Удалить в корзину";
pub const WORKSPACE_DELETE_HINT: &str = "Папка целиком, включая скрытые и игнорируемые файлы, будет перемещена в корзину. Отмена или ошибка оставит её на месте.";
pub const WORKSPACE_DELETE_FILE_HINT: &str =
    "Файл будет перемещён в корзину Windows. Отмена или ошибка оставят его на месте.";
pub const WORKSPACE_OPEN_EXTERNAL: &str = "Открыть в системе";
pub const WORKSPACE_REVEAL: &str = "Показать в проводнике";
pub const WORKSPACE_FILE_FILTER: &str = "Фильтр файлов";
pub const WORKSPACE_FILE_TRUNCATED: &str = "… файл показан не полностью";
pub const WORKSPACE_PUSH_HINT: &str = "Отправить в настроенный remote";
pub const WORKSPACE_FETCH_HINT: &str = "Забрать изменения из настроенного remote";

pub const WORKSPACE_NO_BRANCH: &str = "нет текущей ветки";
pub const WORKSPACE_NO_AI_COMMAND: &str = "не выбрана AI-команда";
pub const WORKSPACE_AI_EMPTY: &str = "CLI вернул пустое сообщение";
pub const WORKSPACE_NO_CHANGES_FOR_FILE: &str = "нет изменений для этого файла";
pub const WORKSPACE_PATH_INSIDE_REPO: &str = "нужен путь внутри репозитория";
pub const WORKSPACE_FILE_EXISTS: &str = "файл уже существует";
pub const WORKSPACE_NO_SUCH_FILE: &str = "нет такого файла";
pub const WORKSPACE_WORKER_LOST: &str = "поток панели остановился — панель перезапущена";
pub const WORKSPACE_DIFF_STALE: &str = "файл изменился — обновите панель";
pub const WORKSPACE_REPO_CHANGED: &str = "репозиторий сменился — действие отменено";
pub const WORKSPACE_TRUST_TITLE: &str = "Требуется доверие к репозиторию";
pub const WORKSPACE_TRUST_HINT: &str = "Конфигурация этого репозитория может запускать программы (фильтры, хуки, внешние diff-драйверы, программы подписи). Обычные настройки — ветки, remote, редактор — доверия не требуют и запрос не вызывают. До подтверждения ANVIL не читает статус, diff и историю и не выполняет Git-действия. Разрешение действует в этой сессии для этого репозитория и только для показанной конфигурации; её изменение сбрасывает разрешение, а ваш черновик коммита сохраняется.";
pub const WORKSPACE_TRUST_HAZARDS: &str = "Могут запускать программы:";
pub const WORKSPACE_TRUST_APPROVE: &str = "Доверять этой конфигурации";
pub const WORKSPACE_TRUST_STALE: &str =
    "Репозиторий или конфигурация изменились — разрешение отменено. Проверьте папку и подтвердите заново.";
pub const WORKSPACE_AI_NO_STAGE: &str =
    "Сначала добавьте изменения в индекс: AI составляет сообщение только по staged-изменениям.";

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

// Collapsed panes list.
pub const COLLAPSED_HINT: &str = "Enter — вернуть пейн · Esc — закрыть";
pub const COLLAPSED_EMPTY: &str = "Нет свёрнутых пейнов. Ctrl+Alt+C прячет пейн от глаз";
pub const TAB_COLLAPSED: &str = "Свёрнутые панели";

/// The Russian name of a hotkey action, for the settings list. The
/// action ids stay the config.json spelling; only the shown name is
/// translated, so the list doubles as a guide to editing config.json.
pub fn hotkey_label(action: &crate::hotkeys::Action) -> String {
    use crate::hotkeys::Action as A;
    match action {
        A::NewTab => "Новая вкладка".to_owned(),
        A::NewWindow => "Новое окно".to_owned(),
        A::CloseTab => "Закрыть вкладку".to_owned(),
        A::ReopenTab => "Вернуть вкладку".to_owned(),
        A::RenameTab => "Переименовать вкладку".to_owned(),
        A::NextTab => "Следующая вкладка".to_owned(),
        A::PreviousTab => "Предыдущая вкладка".to_owned(),
        A::MoveTabLeft => "Сдвинуть вкладку влево".to_owned(),
        A::MoveTabRight => "Сдвинуть вкладку вправо".to_owned(),
        A::Tab(n) => format!("Вкладка {n}"),
        A::SplitRight => "Сплит вправо".to_owned(),
        A::SplitBottom => "Сплит вниз".to_owned(),
        A::PaneNavLeft => "Пейн: влево".to_owned(),
        A::PaneNavRight => "Пейн: вправо".to_owned(),
        A::PaneNavUp => "Пейн: вверх".to_owned(),
        A::PaneNavDown => "Пейн: вниз".to_owned(),
        A::PaneNavPrevious => "Пейн: предыдущий".to_owned(),
        A::PaneNavNext => "Пейн: следующий".to_owned(),
        A::PaneMaximize => "Развернуть пейн".to_owned(),
        A::ClosePane => "Закрыть пейн".to_owned(),
        A::PaneCollapse => "Свернуть пейн".to_owned(),
        A::PaneRestore => "Вернуть свёрнутый пейн".to_owned(),
        A::CollapsedList => "Список свёрнутых пейнов".to_owned(),
        A::Profile(id) => format!("Профиль: {id}"),
        A::ProfileSelector => "Выбор профиля".to_owned(),
        A::Settings => "Настройки".to_owned(),
        A::ToggleFullscreen => "Полный экран".to_owned(),
        A::ToggleFrameStats => "Время кадра (отладка)".to_owned(),
        A::CtrlC => "Прерывание (Ctrl+C)".to_owned(),
        A::Copy => "Копировать".to_owned(),
        A::Paste => "Вставить".to_owned(),
        A::SelectAll => "Выделить всё".to_owned(),
        A::Clear => "Очистить".to_owned(),
        A::ZoomIn => "Масштаб: увеличить".to_owned(),
        A::ZoomOut => "Масштаб: уменьшить".to_owned(),
        A::ResetZoom => "Масштаб: сбросить".to_owned(),
        A::PreviousWord => "Предыдущее слово".to_owned(),
        A::NextWord => "Следующее слово".to_owned(),
        A::DeletePreviousWord => "Удалить предыдущее слово".to_owned(),
        A::DeleteNextWord => "Удалить следующее слово".to_owned(),
        A::DeleteLine => "Удалить строку".to_owned(),
        A::Search => "Поиск".to_owned(),
        A::ToggleWorkspace => "Панель git".to_owned(),
        A::ScrollToTop => "Прокрутка: в начало".to_owned(),
        A::ScrollToBottom => "Прокрутка: в конец".to_owned(),
        A::ScrollPageUp => "Прокрутка: страница вверх".to_owned(),
        A::ScrollPageDown => "Прокрутка: страница вниз".to_owned(),
        A::ScrollUp => "Прокрутка: вверх".to_owned(),
        A::ScrollDown => "Прокрутка: вниз".to_owned(),
    }
}

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
