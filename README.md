<p align="center">
  <img src="icons/anvil-256.png" width="112" alt="Логотип ANVIL">
</p>

<h1 align="center">ANVIL</h1>

<p align="center">
  <strong>Лёгкий терминал для Windows с богатым набором инструментов.<br>Вкладки, сплиты, Git и AI-сессии в одном рабочем пространстве.</strong>
</p>

<p align="center">
  <a href="https://github.com/RERCON0/ANVIL/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/RERCON0/ANVIL/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <a href="https://github.com/RERCON0/ANVIL/actions/workflows/security.yml"><img alt="Security" src="https://github.com/RERCON0/ANVIL/actions/workflows/security.yml/badge.svg?branch=main"></a>
  <a href="https://github.com/RERCON0/ANVIL/blob/main/Cargo.toml"><img alt="Windows x64" src="https://img.shields.io/badge/Windows-x64-0078D4?logo=windows&logoColor=white"></a>
  <a href="LICENSE"><img alt="GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-2f855a"></a>
  <a href="https://t.me/rercon"><img alt="Telegram" src="https://img.shields.io/badge/Telegram-@rercon-26A5E4?logo=telegram&logoColor=white"></a>
</p>

<p align="center">
  <a href="#возможности">Возможности</a> ·
  <a href="#запуск">Запуск</a> ·
  <a href="#сборка">Сборка</a> ·
  <a href="#горячие-клавиши">Горячие клавиши</a> ·
  <a href="#документация">Документация</a>
</p>

> [!NOTE]
> ANVIL находится в активной разработке. Пока основной способ запуска — сборка из исходников; CI проверяет текущую ветку и сохраняет тестовый пакет.

## Возможности

ANVIL написан на Rust: интерфейс рисуется через egui/OpenGL, а оболочки работают
через ConPTY из проекта Windows Terminal. Claude Code, Codex, OpenCode, OMP
и обычные shell-сессии живут в одном окне рядом с инструментами проекта.

### Рабочее пространство под несколько задач

- **Вертикальные вкладки:** названия, цвета, активность, перетаскивание и быстрый выбор профиля.
- **Сплиты вправо и вниз:** навигация с клавиатуры, максимизация и сворачивание пейнов.
- **Перестановка пейнов:** `Ctrl+Shift` и перетаскивание с подсветкой места вставки; запущенные процессы продолжают работу.
- **Восстановление раскладки:** вкладки, каталоги, сплиты и состояние панелей сохраняются между запусками.

> [!IMPORTANT]
> Восстанавливается рабочее пространство. Оболочки запускаются заново; живые процессы AI-CLI и скроллбек между запусками не сохраняются.

### Git рядом с терминалом

У каждого пейна своя панель проекта: статус файлов, стейджинг целиком или по
отдельным ханкам, diff, история коммитов, push и fetch. Рядом — дерево файлов,
поиск и предпросмотр текста с рендером Markdown.

Для сообщения коммита можно вызвать AI: ANVIL передаёт staged-diff, предлагает
текст и оставляет его на редактирование. Доступны Codex, Claude, OpenCode,
Gemini и Aider; для внешних бэкендов нужны установленные CLI.

> [!CAUTION]
> При генерации сообщения staged-diff отправляется AI-провайдеру. Проверьте индекс перед вызовом: ключи, токены и другие секреты не должны попасть в запрос.

### Контекст и лимиты AI-сессий

| Интеграция | Что показывает |
|---|---|
| Claude Code | Модель, заполнение контекста, лимиты и агент — в statusLine и бейдже вкладки |
| Квоты 19 провайдеров | Проценты по временным окнам, сброс лимитов, баланс или расходы |
| Несколько окон | Общий кэш квот; API опрашивает одно окно |

Claude statusLine и строка квот включаются отдельно в настройках. ANVIL
использует существующий вход поддерживаемых CLI; отдельные ключи квот
хранятся в Диспетчере учётных данных Windows.

### Терминал для ежедневной работы

Truecolor, поиск по буферу, OSC 8-ссылки, режимы мыши, bracketed paste,
масштабирование текста и системные шрифты с fallback-глифами. Блочная графика
и Braille рисуются по сетке, чтобы ASCII-арт и спиннеры сохраняли форму.

Многострочная вставка получает предпросмотр, когда приложение не включило
bracketed paste. Копирование при выделении настраивается, OSC 52 выключен
по умолчанию. Подробности поведения — в [справочнике](docs/REFERENCE.md).

## Запуск

Нужны **Windows 10/11 x64** и установленная оболочка. Git for Windows
используется панелью Git; AI-CLI устанавливаются отдельно и обнаруживаются
через профили. Для встроенного Codex используется вход Codex CLI или API-ключ.

1. Соберите приложение по инструкции ниже.
2. Запустите `target\release\anvil.exe`, оставив `conpty.dll` и `OpenConsole.exe` рядом.
3. Выберите профиль оболочки и откройте каталог проекта.
4. В настройках включите нужные AI-интеграции и квоты.

> [!TIP]
> Claude statusLine подключается через **Настройки → Claude Code → Строка статуса от ANVIL**. Диалог покажет изменение файла настроек. После переноса программы обновите путь к helper; перед удалением отключите интеграцию.

## Сборка

Для сборки нужны Git, rustup и **Visual Studio Build Tools** с инструментами
C++ и Windows SDK. Rust **1.92.0**, rustfmt и Clippy закреплены в
[rust-toolchain.toml](rust-toolchain.toml); rustup выбирает их автоматически.

Из корня репозитория в PowerShell:

```powershell
cargo build --locked --release --bin anvil --bin anvil-claude-status
.\target\release\anvil.exe
```

Портативный пакет с ConPTY, лицензиями и сведениями о сборке (нужен Python
в `PATH`: `package.ps1` собирает лицензии Rust-крейтов):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\package.ps1
# dist\anvil-<версия>-x64.zip и dist\SHA256SUMS.txt
```

| Файл | Назначение |
|---|---|
| `anvil.exe` | Терминал, GUI, Git, AI-коммиты и фоновые квоты |
| `anvil-claude-status.exe` | Консольный helper для statusLine Claude Code |
| `conpty.dll`, `OpenConsole.exe` | Runtime псевдоконсоли Windows |
| `LICENSE`, `LICENSES/` | Лицензии приложения, встроенных ресурсов и Rust-зависимостей (`LICENSES/THIRD-PARTY-RUST.txt`) |
| `SOURCE.txt`, `BUILD.json` | Версия, исходный коммит и хеши файлов |

> [!NOTE]
> Пакеты разработки и артефакты CI пока не подписаны. SHA-256 и `BUILD.json` проверяют целостность; они не подтверждают издателя. Публикация релизов выполняется отдельно.

### Проверки перед коммитом

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
python -B -m unittest discover -s tests -p 'test_*.py' -v
python -B -m unittest discover -s scripts -p 'test_*.py' -v
python -B scripts/check_conpty.py
python -B scripts/check_docs.py
```

Python 3.13 используется для тестов Aider-helper и проверок CI. Иконки
пересобираются через `cargo run --locked --example gen_icons`;
исходник и правила экспорта — в [icons/README.md](icons/README.md).

## Горячие клавиши

| Действие | Клавиши |
|---|---|
| Новая вкладка / новое окно | `Ctrl+Shift+T` / `Ctrl+Shift+N` |
| Закрыть / вернуть вкладку | `Ctrl+Shift+W` / `Ctrl+Shift+Z` |
| Следующая / предыдущая вкладка | `Ctrl+Tab` / `Ctrl+Shift+Tab` |
| Сплит вправо / вниз | `Ctrl+Shift+S` / `Ctrl+Shift+D` |
| Перейти в соседний пейн | `Ctrl+Alt+Стрелки` |
| Переставить пейн | `Ctrl+Shift` и перетаскивание ярлыка мышью |
| Максимизировать пейн | `Ctrl+Alt+Enter` |
| Свернуть / вернуть пейн | `Ctrl+Alt+C` / `Ctrl+Alt+R` |
| Список свёрнутых пейнов | `Ctrl+Alt+L` |
| Панель Git / поиск | `Ctrl+Shift+G` / `Ctrl+Shift+F` |
| Копировать / вставить | `Ctrl+Shift+C` / `Ctrl+Shift+V` |
| Настройки / полный экран | `Ctrl+,` / `F11` |

Полный список и переопределение — в настройках и
[справочнике](docs/REFERENCE.md#горячие-клавиши). Сочетания привязаны к
физическим клавишам и работают независимо от раскладки.

## Настройки и данные

| Каталог | Содержимое |
|---|---|
| `%APPDATA%\anvil` | `config.json`, сохранённая раскладка `session.json`, журнал `anvil.log` |
| `%LOCALAPPDATA%\anvil` | Общий кэш и расписание квот, блокировка опроса и временные статусы Claude |
| Диспетчер учётных данных Windows | Отдельные ключи квот `anvil/quota/<провайдер>` |

Настройки сохраняются при применении; `config.json` можно редактировать на
ходу. Схема, примеры квот и подробный список файлов — в
[справочнике](docs/REFERENCE.md#настройки-и-файлы).

## Документация

- [Справочник возможностей и настроек](docs/REFERENCE.md)
- [Безопасность и границы защиты](SECURITY.md)
- [Сторонние ресурсы и лицензии](THIRD-PARTY.md)
- [Происхождение ConPTY](vendor/conpty/README.md)
- [CI](https://github.com/RERCON0/ANVIL/actions/workflows/ci.yml) и [Security](https://github.com/RERCON0/ANVIL/actions/workflows/security.yml)

ANVIL распространяется под [GPL-3.0-or-later](LICENSE). Автор — rercon prod.
