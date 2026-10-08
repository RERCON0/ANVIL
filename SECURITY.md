# ANVIL security

## Reporting

Report credential leaks or code-execution problems privately to
[the author on Telegram](https://t.me/rercon). Include the version, a minimal
reproduction and expected behaviour; do not publish real tokens or keys.

## Automated checks

- CI runs formatting, Clippy with warnings denied, Rust/Python tests and real Windows ConPTY/Git integration.
- Packages are checked against their exact source revision, licences, file hashes, x64 GUI/CLI subsystems and ASLR/high-entropy ASLR/DEP. Redistributable-only Visual C++ runtime DLLs are rejected; Microsoft's pinned ConPTY uses the system UCRT.
- Security audits the complete locked dependency graph with RustSec; vulnerabilities, unsound and yanked dependencies fail the job. Informational advisories remain visible.
- Gitleaks scans the complete Git history with redacted output. The single exception is an exact synthetic test token, restricted to its test file and value.
- actionlint validates workflows and shell commands. Actions use full commit SHA pins; downloaded Gitleaks/actionlint archives have pinned SHA-256 hashes.
- Dependabot proposes Cargo and Actions updates. Updates are reviewed rather than automatically merged.

Workflows have only `contents: read`; checkout does not persist Git credentials.
CI receives no AI credentials or signing key and does not publish releases.
Dependency caches are saved only by pushes to `main`; workspace crates and
installed tools are excluded. Push and PR events avoid duplicate PR-branch runs.

## Credentials and AI commit messages

Built-in AI command profiles restrict tools, hooks and plugins. Codex commit
messages use the Responses protocol, without executing tool events. Requests use
system WinHTTP/TLS and fixed OpenAI endpoints; redirects are disabled.
API-key Codex inference is the default. `workspace.codexChatgptLogin` is off by
default and explicitly enables an undocumented ChatGPT subscription endpoint.
ANVIL identifies itself as ANVIL, not the official Codex client. Endpoint/auth
compatibility is not guaranteed; a supported API key has separate API billing.
This switch does not control quota display or agents launched in terminal panes.

A custom `workspace.aiCommitCommand` is trusted user code, not an OS sandbox.

The staged diff goes to the AI provider; external CLI backends also receive it
as a process argument. Do not stage secrets before requesting a message.
Known keys and tokens, including inherited CLI credentials, are redacted from
provider/HTTP/stream diagnostics. Temporary backend state is removed after use;
crash leftovers older than one hour are swept in the background at startup
and before another generation.

ANVIL does not refresh CLI OAuth tokens. Reauthenticate in the relevant CLI
when a login expires. Separate quota keys use Windows Credential Manager:
this protects at the Windows account boundary, not from other programs running
as the same user. Quota requests run off the UI thread with response limits,
timeouts and rate-limit backoff; application windows share polling/cache state.

## Terminal, Git and files

- OSC 52 is off by default. Paste filters control characters; multiline text requires a preview when bracketed paste is unavailable.
- Activated links use an allowlist of schemes, reject controls and are capped at 32 KiB. URL punctuation trimming runs in linear time.
- Git settings capable of launching programs require explicit trust. Approval is held in memory and invalidated by changes to hazardous settings or hook bytes/membership. Default hooks in the common Git directory, custom hook paths, linked worktrees and initialized submodules are covered. Control-file bytes are checked as well as metadata, so restoring a modification timestamp does not preserve a stale cache. `.sample` hooks do not count as runnable hooks. Hook links/reparse redirects and oversized trees fail closed.
- `core.pager` is disabled with `--no-pager`; Git output is piped. `core.askPass` is overridden by an empty `GIT_ASKPASS` and prompting is disabled. These keys do not need the repository trust dialog because their execution is independently suppressed.
- Git metadata preflight bounds size, traversal depth and time. Network/device paths are rejected before accessing their targets.
- Panel Git/AI commands use deadlines, bounded stdout/stderr and Windows Job Objects to clean up subprocess trees. These limits do not terminate user CLI sessions in terminal panes.
- DLL lookup is restricted to the application directory and `System32`. Vendored ConPTY hashes and valid pinned Microsoft Authenticode signatures are checked before Windows packaging. Both shipped ANVIL executables set the DLL policy at the start of `main`.
- File previews reject binary/non-UTF-8 content, have bounded reads and trim truncation at a UTF-8 boundary; closing a preview invalidates delayed responses. Claude integration settings are read through a handle with a 2 MiB limit, including mutation and reread boundaries.
- Confirmed file deletion uses the Windows Recycle Bin. Executable formats opened from the browser are revealed in Explorer.
- Background atomic file reads/replacements retry only transient Windows access/sharing/lock errors, within 500 ms. UI calls make one attempt; failed saves remain pending and retry later. Synchronous filesystem operations themselves can still wait on Windows.

Detailed Git/submodule trust rules are in the
[English reference](docs/REFERENCE.en.md#git-trust).

CLI restoration is separately disabled by default. Only fixed Codex/Claude/
OpenCode/OMP continue commands are restored; saved argv, prompts, tokens and
permission-bypass flags are not replayed. Installed programs resolve from
absolute PATH entries/known installation paths, never the saved project directory.
Native CLI trust, permissions and saved conversation behaviour still apply.
Closing live panes, tabs or the window asks before terminating their processes.

Quota login databases are opened read-only, but SQLite may create WAL/SHM
coordination files. This is not a promise of zero filesystem writes beside a
live database. `immutable=1` is unsuitable for a changing WAL database.

## Signed releases

Release packages have an Ed25519-signed inventory, with every payload hash and
source/build provenance. Verification uses an independently trusted publisher
key, checks bounded ZIP/JSON/PE data and does not execute or extract payloads.
The private key stays outside Git and CI. See
[RELEASING](docs/RELEASING.md) for the fingerprint and procedure.

This is not Authenticode, so Windows SmartScreen may still warn. CI/development
candidates remain explicitly unsigned. Hashes in an unsigned candidate establish
integrity against its metadata, not publisher identity.

## Known boundaries

Git/filesystem checks use snapshots. They do not atomically isolate a repository
from another process changing it between validation and execution. Installed
shells, CLIs and custom commands run as the current Windows user.

Scrollback and caches are bounded, but there is no overall memory ceiling:
combining characters inside one upstream Alacritty cell and queued input when a
PTY reader stops still need separate limits and backpressure. Initial layout of
a large preview can occupy the UI thread. See the
[long-session reference](docs/REFERENCE.md#долгие-cli-сессии).

External provider API compatibility is tested with fixtures; upstream changes
may require adapter updates. Passing checks and a valid package signature do not
establish the absence of all vulnerabilities.
