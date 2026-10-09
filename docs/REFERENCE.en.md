# ANVIL reference

[Русский](REFERENCE.md) · [Getting started and building](../README.md) ·
[Security boundaries](../SECURITY.md) · [Release verification](RELEASING.md)

The default interface language is English. The **RU/EN** title-bar button
changes it immediately and saves `language: "en"` or `"ru"`. External CLI
applications control their own language.

## Tabs and panes

- Each project tab has a number, name, activity indicator and optional Claude
  badge. Drag tabs to reorder; use the context menu to rename or colour them.
  Blue, green, orange, purple, red and yellow marks are saved with the layout.
- Split right or down; navigate, maximise, collapse or close individual panes.
  Collapsed panes keep their live processes. Restoring one places it beside its
  previous neighbour, or the focused pane if that neighbour no longer exists.
- Drag a divider to resize its neighbours. Double-click it with the left mouse
  button to give those two neighbours equal width (vertical divider) or height
  (horizontal divider). Other panes, focus and running processes are unchanged;
  the new sizes are saved with the layout.
- Hold **Ctrl+Shift**, grab the pane label, and drag it to another pane edge or
  the outer tab edge. The green target shows the insertion location. Release
  the mouse before the modifiers; releasing the modifiers first cancels.
  Moving a pane preserves its process, scrollback, folder and Git panel.
  Dragging is available with multiple visible panes and no maximised pane.
- Inactive panes and their panels are dimmed. Each pane can use a different
  project directory, Git status and history.
- The tab list scrolls separately from its new-tab/profile buttons and Settings.
- A new window (`Ctrl+Shift+N`) is a separate process with a fresh tab. It does
  not clone live sessions or overwrite the main window's saved layout.
- Panes, tabs and the window close immediately, without an extra confirmation.
  Their processes stop. Restoring a closed layout starts new processes; it
  cannot resurrect work that was executing in a terminated agent.

## Restoring CLI agents

**Settings → Terminal** has separate switches:

| Setting | Default | Behaviour |
|---|---|---|
| Restore tabs and layout (`restoreSession`) | on | Restore tabs, colours, folders, splits, collapsed panes, maximised pane and Git panel state |
| Restore CLI agents and continue the last conversation (`restoreAgents`) | off | Start the detected supported agent in its saved folder, instead of a shell |

Agent restoration requires layout restoration. Supported commands are
`codex resume --last`, `claude --continue`, `opencode --continue` and
`omp --continue`. These use the CLI's own saved conversations, credentials,
configuration and permission controls. ANVIL saves only a canonical agent name
and folder; it does not replay a prompt, arbitrary command line or permission
bypass flag. Unknown names are ignored. A missing CLI or unavailable folder
shows an error rather than resuming from another directory.

Shell aliases such as `cx` are detected through the actual agent process, not
the typed command. The process list is refreshed before saving on window exit,
to avoid missing an agent launched since the last background poll. A failed
enumeration keeps the last known detection. Alias arguments and
permission-bypass flags are not replayed.

Git Bash (including interactive custom Bash profiles) restores agents through
the same shell startup files as a manual launch. The canonical executable is
resolved using Bash's resulting `PATH`, `HOME` and `XDG_*` environment, not
ANVIL's separate Windows/npm CLI lookup. The saved folder is selected after
initialisation using the built-in `cd` (not a user's alias/function), with no
folder text interpolated into shell commands. Bash
aliases and functions are bypassed; only the fixed continuation arguments above
are used. Other profiles still use direct Windows CLI lookup. If Bash cannot
find the CLI, it reports the error and the pane returns to an interactive shell.

The CLI chooses its most recent conversation for that folder. Two panes using
the same agent in the same folder can therefore select the same conversation.
No active task or terminal scrollback is restored. The old program's dynamic
OSC title is not assigned to a fresh process. WSL agents are not identified by
Windows process discovery and are not automatically restored. Exiting a
restored agent starts the original shell in the same pane and folder, retaining
its layout and Git panel. Custom profiles that launch an agent directly or
through a shell startup command return to an available interactive shell
instead of relaunching the agent. The shell gets a fresh terminal buffer;
pane zoom is retained, and a pending paste for the old process is cancelled.

## Terminal

- `alacritty_terminal` supplies VTE parsing, indexed/truecolor, cursor and mouse
  modes, bracketed paste and OSC 8 links. OSC 7/1337 folder reports let the Git
  panel follow `cd`.
- Search the buffer with `Ctrl+Shift+F`; select with the mouse and paste with
  the middle button. Copy on selection is on by default. Releasing a selection
  copies without a toast and retains its highlight. Disable it with
  `terminal.copyOnSelect=false`. Hold Shift to select when an application has
  captured the mouse.
- Ctrl+click opens a link. In a mouse-capturing application use Ctrl+Shift+click
  so the click opens the link rather than being sent to the application.
- Paste strips control characters, including ESC and injected bracketed-paste
  terminators. Only ANVIL constructs the paste envelope. With bracketed paste
  off, multiline input shows a preview of the exact bytes; Enter cancels and
  insertion requires the explicit button.
- OSC 52 clipboard writes are disabled by default and can be enabled in Terminal
  settings. Activated links allow only supported schemes, reject control
  characters and have a 32 KiB bound.
- Block graphics are drawn on cell boundaries. Braille uses a fixed 2×4 dot
  grid, including OMP's spinner. These graphics do not need fallback fonts.
- Zoom with `Ctrl+=`, `Ctrl+-` and `Ctrl+0`. Interface text uses embedded
  Cascadia Mono; terminal text uses the chosen system font and lazy fallbacks.
  Fallback glyphs, filled and outline cursors align with the terminal cell and
  text baseline.

## Git and files

Each pane owns its panel; toggle it with `Ctrl+Shift+G`. When the pane is too
narrow to fit the panel and a usable terminal, the panel is temporarily hidden.
Its state is retained and it reappears when the pane is widened.

- Inspect status and diffs with line numbers, stage files or individual hunks,
  edit a commit message and commit. Unstage applies only to staged selections;
  staged renames restore both original and new paths.
- File previews and diffs each have their own **×** beside the filename. Closing
  one retains the panel, changes list and draft message. Generation IDs prevent
  a delayed worker response from reopening it.
- History shows up to 80 commits from HEAD and upstream, with truncation marked,
  branch labels and distinct outgoing/incoming/history sections. Commit details
  show the full message and files. Selecting a file opens only its patch; Back
  returns to the previous position, and Copy patch copies that selected file.
- Fetch and push update history. External HEAD/upstream changes are detected by
  background polling. First push considers `branch.<name>.pushRemote`,
  `remote.pushDefault`, then `branch.<name>.remote`, with `origin` as fallback.
- Switch branches from the panel: the branch name opens a filtered list (the
  current branch first, then local ones, then remote-tracking ones). Checking
  out a remote branch creates the local branch that tracks it, or uses the
  local branch of the same name when it already exists; a separate field
  creates a branch at HEAD and switches to it. Git carries uncommitted changes
  itself and refuses when they conflict, with its reason shown and nothing
  overwritten. Names are checked before git runs: a leading `-`, control
  characters, `..`, `@{`, `.lock`, Windows device names and other invalid ref
  forms are refused. `post-checkout` hooks and smudge filters are covered by
  the same repository trust approval as commit. After a switch the panel drops
  the diff, file list and history of the branch that was left and reads them
  again; the commit draft stays.
- The Files tab has a collapsible tree and fuzzy filter, text/Markdown previews,
  project line counting, create/rename and confirmed Recycle Bin deletion.
  Markdown renders headings, lists, quotes, code, tables and inline formatting.
- Preview reads are capped at 8 MiB. Binary or invalid UTF-8 content is refused;
  a truncated multibyte character is omitted rather than replaced with garbage.
  Truncation is visible. Rows are virtualised and wrapping/Markdown is cached,
  but initial layout or width/font/DPI changes can still occupy the UI thread.
- Line counting streams in a worker and skips inaccessible files, files above
  4 MiB and files containing NUL. A final line without a newline still counts.
- Renaming never overwrites an existing target. Deleting a folder includes its
  hidden/ignored contents and uses the Windows Recycle Bin. Cancellation or
  failure preserves it; permanent deletion is not a fallback.
- Open external uses an allowlist with signature checks for text, images and
  PDF. Executables, scripts, shortcuts and unknown formats are revealed in
  Explorer instead. Paths are checked for containment, ADS/device names and
  redirection before launch. Explorer selection uses a Shell item, so commas
  and Unicode in filenames do not become command-line delimiters.

## AI commit messages

Codex, Claude, OpenCode, Gemini and Aider are supported. Unless configured,
ANVIL identifies a supported CLI running in the pane. OMP restoration does not
make it an AI commit backend. External backends require their installed CLI.

Only the staged diff, staged filenames and recent commit subjects are sent.
Generation does not stage files or create a commit; inspect and edit the
proposed message. Large prompts are bounded and marked as truncated. The
message editor scrolls without pushing the buttons out of view.

**The staged diff goes to the provider. Check it for secrets and private data.**
External CLI backends also receive the prompt as a process argument.

Built-in backend profiles disable executable tools, hooks and plugins:

| Backend | Isolation |
|---|---|
| Claude | `--bare` with an API key, otherwise safe/restricted mode with tools, MCP, slash commands and hooks disabled; commit generation requires **2.1.248+**. Executable managed hooks cause refusal, not a permissions bypass |
| OpenCode | Isolated XDG state, pure mode and an agent with closed tool permissions; choose a connected `provider/model` in Git panel settings |
| Gemini | Empty core tool list; hooks, extensions and MCP disabled |
| Aider | Native inference through its venv Python, without `main()`; ANVIL reads `.aider.conf.yml` and projects an 11-key allowlist through the child environment. Aider's HOME points to private temporary state |
| Codex | Direct Responses requests with no tools and `tool_choice=none`; tool events are never executed |

API-key Codex inference is the default. **Git panel → Use the saved Codex ChatGPT
login for commit messages (experimental)** (`workspace.codexChatgptLogin`) is a separate,
disabled-by-default experiment using an undocumented subscription endpoint.
ANVIL identifies itself as ANVIL, not the official Codex client. Compatibility
is not guaranteed. Supported API usage has separate billing; ChatGPT quota
display is independent of this switch. ANVIL does not refresh CLI OAuth tokens;
reauthenticate through the CLI when needed.

Requests use WinHTTP, Windows TLS/certificates and system proxy settings, fixed
provider endpoints and no redirects. One deadline covers the job; synchronous
WinHTTP phases have coarse timeout granularity. A completed response is
required, so a broken stream is an error rather than a partial commit message.
Known secrets are redacted from diagnostics. Tokens travel through the child
environment rather than argv; model settings contain no credentials.

Private temporary backend state is removed after use. Crash leftovers older
than an hour are swept in the background at startup/before generation. Aider
keys are not written to its temporary script. Unrecognised flags/wrappers or
unreadable policies cause refusal, not a less restricted fallback. Custom
`workspace.aiCommitCommand` remains trusted user code, not an OS sandbox.

## Git trust

Program-running repository settings require approval. These include filters,
fsmonitor, external diff/textconv, signing/credential/SSH helpers, custom hooks,
includes, transport/protocol/URL rewrites and executable submodule updates.
Routine branch, remote URL, editor and line-ending settings do not themselves
require approval. `core.pager` is disabled by `--no-pager` and piped output;
`core.askPass` is overridden by an empty `GIT_ASKPASS`, with prompting disabled.

The fingerprint includes default hooks in the common Git directory, custom
hook directories, linked worktrees and initialized submodules. Hook bytes and
directory membership are rechecked even when size/date are unchanged. `.sample`
files are not treated as runnable hooks. Hook symlink/reparse redirects fail
closed. Limits: 2048 entries, 256 KiB per file, 4 MiB total and depth 8.

Approval is in memory for the repository across panes/tabs of that process.
Hazardous configuration/hook changes revoke it; a stale dialog cannot approve
the new state. Another ANVIL window asks independently. Changes to ordinary
tracking settings preserve approval. Until approval, the panel does not read
status/diffs/history or execute Git actions; the draft message is retained.

Preflight resolves Git metadata before running Git, including gitfiles, configs,
HEAD, alternates, refs/objects/reftable and submodules. Network, device and UNC
targets are refused before access, including link/junction targets. Git's
C-quoted/octal paths are decoded before validation. Control files are checked
by content as well as timestamps. Bounds are 8192 watched paths, 16384 aliases,
128 alternate stores, 256 repositories, depth 64 and 4194304 metadata entries.
Config reads are capped at 1 MiB per file/4 MiB total; config capture shares a
30-second budget. Exceeding a limit refuses the operation.

Panel Git/AI processes start suspended, enter a Windows Job Object and only
then run. Deadlines, bounded stdout/stderr and chunked stdin cover the job;
completion/error/timeout closes its process tree. This does not terminate
independent live CLI sessions in terminal panes. Filesystem checks are
snapshots, not atomic isolation from another process modifying the repository.

## Claude Code integration

The optional status line and tab badge display model, context usage, 5-hour/
7-day limits and agent. Status line fields also include folder and branch.
Their field selections are independent; hiding a badge does not uninstall
the integration.

Enable **Settings → Claude Code → ANVIL status line** and confirm the dialog.
Only `statusLine` in `%USERPROFILE%\.claude\settings.json` (or
`%CLAUDE_CONFIG_DIR%\settings.json`) changes. The dialog shows current and new
commands. Disable before removing ANVIL; after moving it, approve the updated
helper path. Disabling restores the previous line if it has not been replaced
by another program.

`anvil-claude-status.exe` serves Claude's stdin/stdout statusLine contract; it
does not launch a GUI or poll quota APIs. Inside ANVIL it also writes a pane
status JSON. Branch lookup uses bounded Git. Input above 1 MiB is rejected,
controls/bidi characters are removed from displayed labels, and unsafe cwd is
refused. `ANVIL_STATUS_DIR` and `ANVIL_PANE_ID` are internal environment fields
for the status directory and pane ID, not credentials. Both shipped ANVIL
executables restrict dynamic DLL lookup to their directory and System32.

## Provider quotas

Quotas are independent of Claude statusLine and commit generation. Enable
**Settings → Quotas → Show quotas**; the default is off. Windows share one
background polling leader and cache. Disabled providers are not polled.

Supported providers: Claude, ChatGPT, Z.ai, Zhipu, Kimi, Kimi (kimi.ai), MiniMax,
MiniMax CN, OpenCode Go, Synthetic, Ollama Cloud, Chutes, Command Code, Umans,
DeepSeek, OpenRouter, Kilo, OpenCode Zen and Charm Hyper. Regions have separate
entries. API data determines usage windows, resets, balances or spending.

- Polling is fixed at 30 seconds; `quota.interval` is not a setting. Use Refresh
  now or ↻ for manual refresh. Manual cycles also have a 30-second gap; requests
  inside the gap remain pending. Backwards clock steps do not stall polling.
- 429 respects Retry-After; 401/403 wait for changed credentials. Other providers
  continue. Last good data survives errors, dims when stale and includes the
  update time/error in a tooltip. An API pause cannot promise immediate refresh.
- Leadership is held by `quota.lock`; schedules/backoff/login rejection state
  survive restarts and transfer between windows.
- Logins come from supported user CLI stores, OMP/OpenCode stores, environment
  variables or user Claude env. Repository config is not searched for tokens.
  OAuth refresh remains the CLI's responsibility. Claude/ChatGPT quota queries
  require subscription OAuth; API keys do not substitute. Zen uses Console OAuth.
- Other provider keys can be stored in Windows Credential Manager under
  `anvil/quota/<provider>`. They are absent from config/cache JSON. This is a
  Windows account boundary, not isolation from other software of the same user.
- Live SQLite stores are opened read-only but SQLite may create WAL/SHM
  coordination files. This is not a zero-filesystem-writes guarantee.
- Provider requests use fixed HTTPS destinations, no redirects, bounded bodies,
  deadlines and backoff. Narrow quota bars omit reset times, show the most used
  window and fold extra providers into `+N`, with details available on hover.
  MCP/review windows are hidden by default and may be enabled separately.

## Settings and long sessions

Edits apply immediately; text fields can retain a draft until confirmation.
Opening config.json writes current settings rather than replacing them with
defaults. External config edits are reloaded without overwriting partial JSON.
WSL discovery runs in the background with a deadline; saved WSL profiles work
before discovery finishes. System fonts are cached and refreshed on request.

Scrollback defaults to 25000 lines per pane and applies to existing panes.
Larger histories/wider grids consume more memory. Pending pane events coalesce
title, clipboard, blink, bell and exit signals, and repaints are request-driven.
Pointer-motion repaints are paced at 16 ms; keyboard/click/output events remain
responsive. The frame overlay includes layout, tessellation and GL upload,
excluding vsync wait.

Font coverage caches are capped at 4096 per pane; shared DirectWrite cache at
16384 entries. Grid buffers, row String pools and bounded style caches are
reused; scheme and OSC colour changes invalidate cached styles. OSC sequences
above 1 MiB are dropped whole before the VTE parser; title OSC has a tighter
bound. There is no total memory ceiling: upstream per-cell combining marks and
input queued for a stopped PTY reader need separate storage/backpressure limits.
An agent's model context belongs to that agent, not ANVIL.

## Keyboard shortcuts

| Action | Default keys |
|---|---|
| New tab / window | Ctrl+Shift+T / Ctrl+Shift+N |
| Close / restore tab | Ctrl+Shift+W / Ctrl+Shift+Z |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab, also Ctrl+Shift+Right/Left |
| Tab by number | Alt+1…9 / Alt+0 |
| Rename tab | Ctrl+Shift+R |
| Split right / down | Ctrl+Shift+S / Ctrl+Shift+D |
| Navigate panes | Ctrl+Alt+Arrows, previous/next Ctrl+Alt+[/] |
| Move pane | Hold Ctrl+Shift and drag its label |
| Maximise pane | Ctrl+Alt+Enter |
| Close pane | Ctrl+Shift+L |
| Collapse / restore pane | Ctrl+Alt+C / Ctrl+Alt+R |
| Collapsed pane list | Ctrl+Alt+L |
| Git panel / terminal search | Ctrl+Shift+G / Ctrl+Shift+F |
| Copy / paste | Ctrl+Shift+C / Ctrl+Shift+V, also Shift+Insert for paste |
| Ctrl+C | Copy a selection, otherwise interrupt the terminal program |
| Select all / clear | Ctrl+Shift+A / Ctrl+K |
| Zoom in / out / reset | Ctrl+= / Ctrl+- / Ctrl+0 |
| Profile picker / Settings | Ctrl+Shift+E / Ctrl+, |
| Fullscreen / frame time | F11 (also Alt+Enter) / Ctrl+Shift+F12 |

Settings shows all actions and bindings. JSON uses English action IDs and
hyphenated chords, e.g. `"split-right": ["Ctrl-Alt-S"]`; an empty list disables
an action. Physical keys work across keyboard layouts. Conflicts/invalid chords
produce a toast/log entry; first binding wins, with user bindings before
defaults. Pane dragging uses fixed Ctrl+Shift modifiers and is not rebindable.
Outside pane-label drags, Ctrl+Shift+click retains terminal link behaviour.

## Configuration schema

Only non-default values are written to `%APPDATA%\anvil\config.json`.
Fields use camelCase. Omitted fields take their defaults.

| Field | Default / meaning |
|---|---|
| version | 1 |
| language | `en`; also `ru` |
| font.family / font.size | `Consolas` / 15 |
| colorScheme | Legacy dark identifier; `Dark`/`dark`, `Light`/`light` and legacy Russian names accepted; custom scheme name also allowed |
| customColorSchemes | Array of `name`, `foreground`, `background`, `cursor`, 16-entry `colors` hex values |
| defaultProfile | `git-bash`, with installed-shell fallback |
| profiles | Array of `id`, `name`, `command`, `args`, optional `cwd` and `env`; trusted user programs |
| terminal.scrollback | 25000; configurable up to 1000000 |
| terminal.cursor.shape / blink | `block` / false; also `bar`, `underline` |
| terminal.rightClick | `clipboard`; also `paste`, `menu` |
| terminal.pasteOnMiddleClick / copyOnSelect | true / true |
| terminal.allowOsc52 | false |
| terminal.wordSeparators | Space, parentheses, brackets, braces and quote characters |
| terminal.bell | `off`; also `visual` |
| hotkeys | Map of action ID to chord array |
| restoreSession / restoreAgents | true / false |
| workspace.aiCommitCommand | null: detect supported CLI; explicit custom command is trusted user code |
| workspace.aiCommitModel | null: OpenCode default, otherwise `provider/model` |
| workspace.codexChatgptLogin | false: experimental subscription inference opt-in |
| claudeStatus.enabled / badge | false / true |
| claudeStatus.lineFields | Booleans `model`, `dir`, `branch`, `context`, `fiveHour`, `sevenDay`, `agent` |
| claudeStatus.badgeFields | Booleans `model`, `context`, `fiveHour`, `sevenDay`, `agent` |
| quota.enabled | false |
| quota.providers.<provider>.enabled | Unset: use detected login; false: disable |
| quota.providers.<provider>.windows.<window> | Window/balance visibility overrides |

Claude's `declinedCommand`, `previousStatusLine`, `installedCommand` and
`installedSettingsPath` are integration bookkeeping. Let the confirmation
dialog maintain them; they are not authentication fields.

Merge settings with your existing file rather than replacing unrelated keys:

```json
{
  "version": 1,
  "restoreSession": true,
  "restoreAgents": true,
  "quota": {
    "enabled": true,
    "providers": {
      "chatgpt": { "windows": { "review": true } },
      "kimi": { "enabled": false }
    }
  }
}
```

## Data files

| Location | Data |
|---|---|
| %APPDATA%\anvil\config.json | Non-default settings |
| %APPDATA%\anvil\session.json | Main window, tabs, colours, visible/collapsed panes, folders, maximisation, panels and detected agent names |
| %APPDATA%\anvil\anvil.log | Log rotated at 2 MiB to anvil.log.1 |
| %LOCALAPPDATA%\anvil\quota.json | Shared last quota values, without keys/tokens |
| %LOCALAPPDATA%\anvil\quota-schedule.json | Poll times, failure counters, pauses and credential-change fingerprints, not credentials |
| %LOCALAPPDATA%\anvil\quota.lock / quota.refresh | Polling leader and refresh request |
| %LOCALAPPDATA%\anvil\run\anvil-<pid>-<suffix> | Temporary pane statuses; cleaned on normal exit |
| Windows Credential Manager | Quota keys: anvil/quota/<provider> |

Read/replace sharing retries are bounded on workers; UI calls make one attempt
and keep failed writes pending. Ordinary filesystem calls can still block on
Windows. See [SECURITY](../SECURITY.md) for the trust and concurrency boundaries.
