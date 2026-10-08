# ANVIL audit — 2026-10-08

The review covered terminal/ConPTY I/O, pane and session ownership, file/Git
workers, subprocess execution, AI commit backends, credential and quota stores,
network parsing, configuration, packaging, dependencies and workflows.
Provider parsing was exercised with fixtures; no paid model task was submitted.

## Confirmed findings fixed

| Finding | Reproduction or consequence | Fix and evidence |
|---|---|---|
| File and diff previews could not be closed independently | Opening a file left its content attached to the panel | Separate close buttons release preview buffers; generation IDs prevent a delayed file-read response from reopening a closed or replaced preview. Regression tests cover both races. |
| Quadratic terminal URL trimming | A URL ending in a long run of closing punctuation repeatedly rescanned the whole string | One forward bracket count and one reverse pass; a 120,000-character suffix regression. URL activation also rejects controls and URLs over 32 KiB. |
| Unbounded Claude settings reads | Oversized settings reached UI/setup reads and could grow between metadata checks and the backup read | A 2 MiB handle-based read limit at UI, mutation and reread boundaries; an oversized settings test proves no backup or replacement occurs. |
| npm shim read could outgrow its metadata limit | A concurrent writer could enlarge a shim between the size check and read | Read through the opened handle with a 64 KiB bound. |
| Timestamp subtraction overflow | Extreme commit/reset timestamps could overflow signed subtraction | Saturating age/reset calculations; extreme-date regression coverage. |
| Portable executable required an unbundled Visual C++ runtime | PE imports included `vcruntime140.dll`, so a clean Windows machine could fail at launch | Static CRT in ANVIL/helper builds; package validation rejects redistributable-only runtime imports and checks x64/subsystem/ASLR/DEP. |
| Fresh build embedded the workstation path through generated OpenGL bindings | A release built outside the checkout retained `file!()` paths from the temporary target tree | Remap the absolute target directory as well as source/toolchain paths; reject external target paths in UTF-8/UTF-16. The first candidate was rejected before signing. |
| Package hashes did not authenticate the publisher | Replacing files and recomputing unsigned hashes produced no publisher identity | Independent Ed25519 key, exact signed payload inventory, clean source provenance, bounded verification and negative signature/ZIP/PE tests. |
| Default Git hooks bypassed repository trust | Stage, fetch or commit could execute hooks from an archived repository | Common/worktree hooks, configured paths and initialized submodules enter the approval fingerprint. Bounded exact bytes and directory membership are rechecked. Control files also invalidate caches when length/mtime are restored; effective worktree paths are rescanned. |
| Closing a pane, tab or window silently stopped live sessions | Long-running agents lost work without consent | Confirmation keeps ownership unchanged until acceptance and targets stable pane IDs; Enter/Escape cancel. |
| Narrow splits produced inverted terminal rectangles | The Git panel could overlap adjacent panes and resize a live terminal to two columns | Panel hides temporarily below its usable width and returns when space is available; geometry regressions cover narrow splits. |
| Binary previews caused lossy expansion and expensive wrapping | Binary data became megabytes of replacement characters on the UI thread | Refuse binary/non-UTF-8 content and keep truncated text at a valid UTF-8 boundary. |
| Unstage accepted files with no index changes | Untracked selections produced pathspec errors | Button and request paths require staged changes; staged renames retain both paths. |
| Saved display state and quota scheduling lost information | Maximisation/active tab could restore incorrectly; clock rollback delayed polling | Pane-index remapping, active-tab mapping over successful restores and backwards-clock scheduling regressions. |
| Graphics and helper DLL failures lacked proper handling | Unsupported drivers raised internal errors; the status helper lacked the main DLL policy | Fallible context/surface/painter startup with a driver requirement dialog; shared DLL setup runs first in both shipped binaries. |
| UI retries slept during transient file locks | Config/session saves could pause drawing for 500 ms | UI reads/writes make one attempt; failed saves remain pending. Worker retries remain bounded; synchronous OS I/O is still a boundary. |
| Per-frame repeated work and stale CI coverage | Style resolution, process indexing and run strings were recomputed; Seti regeneration was skipped | Bounded theme/OSC-aware colour cache, pooled strings across rows, one process index per snapshot, interned font names and paced pointer redraws. Hash-pinned fontTools now verifies both generated outputs. |
| Native asset provenance relied only on repository hashes | Packaging did not authenticate the vendored ConPTY publisher | Independently matched the upstream archive and require valid pinned Microsoft Authenticode signatures in Windows packaging/CI. |

## Usability and release preparation

English is the default UI language, with a saved EN/RU title-bar switch. Existing
configuration and saved colour-scheme IDs remain compatible. File and diff close
controls reserve space even for long names. README now explains multiple live
CLI agents per tab, lightweight design, Git workflows and signature limitations.
The screenshot uses real installed CLI processes in disposable repositories and
the existing quota snapshot; it does not invent model replies or usage figures.

The capture example and its helper entry points are debug-only. It does not
modify ANVIL's saved workspace or submit a model prompt. Third-party CLI trust
acceptance uses their normal interactive controls for the generated repository.

The README includes a ten-second framebuffer recording of project tabs, four
real agents, the Git panel and a live pane move. Agents are prestarted for
recording; it is not a startup benchmark. An English reference, configuration
schema and Windows Terminal/WezTerm comparison accompany the demo.

CLI restoration is a separate disabled-by-default setting. It records only
an allowlisted agent name and folder, then uses native continue/resume flags.
It never replays old argv, prompts or permission-bypass flags. Agent detection
changes trigger session persistence, including when no layout was changed.
Several panes of one agent in one folder can resume the same last conversation.
Codex API-key inference is the default; undocumented ChatGPT inference needs
a separate experimental opt-in and identifies the client as ANVIL.

The complete finding-by-finding triage and rejected recommendations are in
[AUDIT-REVIEW](AUDIT-REVIEW.md). Major dependency updates in PR #2 remain a
separate tested migration, described in [DEPENDENCIES](DEPENDENCIES.md).

## Verification

- `cargo test --locked --all-targets`: 589 passed (549 library, 1 helper, 10 ConPTY, 29 Git); one manual flood benchmark ignored.
- Formatting and Clippy with warnings denied on pinned Rust 1.92.0; Clippy also passes on Rust 1.99.0.
- Python: 19 script tests and 3 Aider helper tests, including real OpenSSL Ed25519 verification; no skipped icon-generation test.
- Pinned ConPTY hashes and valid Microsoft signatures, generated Seti font/Rust parity, documentation links and actionlint.
- Fresh RustSec database: 1,295 advisories checked against 337 dependencies; no findings with yanked/unsound denied.
- Gitleaks over all history and the current non-ignored source tree; the existing exact synthetic-token exception remains narrow.
- Real graphics startup/framebuffer capture and recorded drag verification with unchanged CLI process IDs.
- Release-memory stress run: 20 live panes in four tabs, 18 writers completing 2.16 million Unicode/truecolor lines; history reduction released roughly 1 GiB. Counter definitions, separate agent RAM and measured baseline are in [PERFORMANCE](PERFORMANCE.md).

Signed release preparation additionally requires a clean committed source tree,
a fresh build target directory, PE/import/payload checks and independent Ed25519
verification. No remote publication is part of this local preparation.

These checks are release gates, not a claim that all possible flaws were found.

The Microsoft `OpenConsole.exe` is a GUI console host (PE subsystem 2), not a
console-subsystem CLI. Its UCRT/API-set dependencies, also used by `conpty.dll`,
are [Windows 10/11 system components](https://learn.microsoft.com/en-us/cpp/windows/universal-crt-deployment?view=msvc-170).
Tests inspect the actual pinned vendor binaries as well as synthetic PE fixtures;
they continue to reject `vcruntime`, `msvcp` and other redistributable-only DLLs.

## Remaining boundaries

Filesystem trust checks use a snapshot; another process of the same Windows
user can still change files between verification and execution. Installed shells,
CLI tools and a configured custom AI command are trusted user programs.

Scrollback and caches are bounded, but there is no total memory ceiling for
combining characters in one upstream Alacritty cell or input queued while a PTY
reader stops. Initial layout of a large text preview can still occupy the UI
thread. See [SECURITY](../SECURITY.md) and the [session reference](REFERENCE.md#долгие-cli-сессии).
