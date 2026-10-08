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
| Portable executable required an unbundled Visual C++ runtime | PE imports included `vcruntime140.dll`, so a clean Windows machine could fail at launch | Static CRT in normal and package builds; package validation rejects dynamic CRT imports and checks x64/subsystem/ASLR/DEP. |
| Package hashes did not authenticate the publisher | Replacing files and recomputing unsigned hashes produced no publisher identity | Independent Ed25519 key, exact signed payload inventory, clean source provenance, bounded verification and negative signature/ZIP/PE tests. |

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

## Verification

- Rust unit and integration suites: 568 passing tests, plus the existing ignored manual flood benchmark.
- Formatting and Clippy with warnings denied.
- Aider helper, packaging and real OpenSSL Ed25519 verification tests.
- Pinned ConPTY hashes, documentation links and workflow syntax checks.
- RustSec audit of the locked graph; Gitleaks over the full history and changed files.
- Fresh signed package build, PE/import inspection and independent signature verification.

These checks are release gates, not a claim that all possible flaws were found.

## Remaining boundaries

Filesystem trust checks use a snapshot; another process of the same Windows
user can still change files between verification and execution. Installed shells,
CLI tools and a configured custom AI command are trusted user programs.

Scrollback and caches are bounded, but there is no total memory ceiling for
combining characters in one upstream Alacritty cell or input queued while a PTY
reader stops. Initial layout of a large text preview can still occupy the UI
thread. See [SECURITY](../SECURITY.md) and the [session reference](REFERENCE.md#долгие-cli-сессии).
