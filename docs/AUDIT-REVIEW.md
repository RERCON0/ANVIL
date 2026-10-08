# External audit review — 2026-10-08

Reports reviewed against `df54331` and the subsequent local changes. Several
reports read different moments of the working tree. Findings below describe
the final implementation, not an assumption that all reported line numbers
still identify an open bug. No paid provider task was used for verification.

## Confirmed fixes

`REAL-open` describes a defect present when triage began. Every such entry in
this table is fixed by the reviewed change; the Result column records its closure.

| Reports | Classification | Result |
|---|---|---|
| First audit: Codex identity/subscription login | REAL-open | ANVIL identity headers; API-key default and separate disabled-by-default `codexChatgptLogin` opt-in. Mixed login/key and malformed-login fixtures test selection. Undocumented endpoint compatibility is documented; account sanctions were not demonstrated |
| First audit: closing live sessions | REAL-open | Stable pane IDs and explicit confirmation for pane/tab/window close; Enter/Escape cancel. Ownership/PID regression covers cancellation and accepted close |
| First audit: default hooks outside trust | REAL-open | Default/common worktree hooks, custom paths and initialized submodules enter the trust fingerprint. Exact bytes and membership revoke approval; redirects and bounded trees fail closed. Worker regression verifies stage/commit/fetch stay gated |
| Follow-up independent hook review | REAL-open | Control-file bytes invalidate caches even with restored length/mtime; discovery follows effective `core.worktree` before resolving relative hook paths. Independent disposable-repository checks covered worktrees, submodules, binary hooks and scan bounds |
| First audit: OpenGL initialization | REAL-open | Fallible window/context/surface/painter initialization and a readable driver requirement dialog; explicit OpenGL 2.1 with GLES fallback. Failed initialization does not rewrite saved state |
| First audit: untyped Stroke widths | REAL-open | Float widths explicitly typed as f32; warnings are denied by Clippy |
| First audit: old Claude CLI | REAL-open | Claude 2.1.248+ documented; unsupported restricted flags produce an update hint, not an unrestricted fallback |
| First audit: Explorer comma filenames | REAL-open | Shell PIDLs replace `/select,<path>` string parsing. Unicode/comma path round-trip regression |
| Documentation H1 | REAL-open | Both anvil.exe and anvil-claude-status.exe call shared secure DLL setup before their application logic |
| Second audit: 500 ms UI sharing retries | REAL-open | UI read/write scopes make one attempt; worker retries remain bounded. Pending config/session writes retry later. Ordinary synchronous filesystem calls can still wait on Windows |
| Functional F-1 | REAL-open | Panel temporarily hides below usable combined width. Terminal/panel rectangles stay positive and inside the pane; width regression covers narrow splits |
| Functional F-2 | REAL-open | Unstage button and selected paths require index changes; staged rename includes both paths |
| Functional F-3 / F-8 | REAL-open | Binary/non-UTF-8 previews refused without lossy expansion; truncated UTF-8 ends at a valid boundary. Valid large text remains bounded, with documented first-layout cost |
| Functional F-4: maximisation | REAL-open | Maximised pane index round-trips with old-session compatibility |
| Functional F-5 | REAL-open | Backwards wall-clock steps restart scheduling even with a fresh snapshot, without changing the OS clock in tests |
| Functional F-6 | REAL-open | Active tab is mapped over successful restores rather than applied to the shortened list |
| Functional F-7 | REAL-open | NUL-separated log fields preserve control separators in subjects; ref parsing retains commas inside legal ref names |
| GHOST-01 | REAL-open | English Dark/Light aliases work alongside legacy Russian IDs; unknown/malformed schemes log the fallback |
| DUP-04 | REAL-open | Kimi accepts both reset/resets snake/camel timestamp spellings while retaining TTL fallback |
| SC-01 | overstated; hardened | ConPTY archive independently re-derived byte-for-byte. Valid Authenticode plus pinned Microsoft certificate now checked in Windows CI/packaging. A certificate subject alone is not a signature check |
| SC-02 | REAL-open | Event type separates scheduled, dispatch, push and PR concurrency groups |
| SC-04 / SC-05 | REAL-open | Synthetic path fixture; broader PEM/key ignores with explicit public-key exception. Key generation already refuses private keys inside the repository before creation |
| SC-06 | REAL-open | Seti font matched a pinned upstream revision; inherited mapping provenance honestly records the missing original generator revision. Checksummed inputs and hash-pinned fontTools run regeneration in CI, including Rust output. This caught and fixed an encoding-damaged generator comment |
| SC-07 / SC-11 / SC-12 | REAL-open / hardening | Developer/release-shaped builds distinguished; CI candidate glob requires a numeric version; verifier prints resolved OpenSSL path and version |

## Performance findings

No report measured steady-state frame time or pointer CPU. Severity estimates
are not performance measurements. Changes preserve terminal input/selection,
OSC colours, pane identities and individual untracked-file staging.

| ID | Classification and decision |
|---|---|
| O-1 | overstated: real unconditional pointer repaint. Pointer-only scheduling paced to 16 ms; all positions still reach egui, and click/key/output redraws remain immediate |
| O-2 | REAL-open: bounded style memo with a last-key fast path, invalidated for palette and OSC colour changes. Regression compares graphics/text classes and flags with uncached output |
| O-3 | false-positive for hyperlink String allocation: Alacritty 0.26 Hyperlink wraps an Arc. Whole snapshots are intentional for selection/cursor/scroll/resize state; output alone is not a sufficient invalidation key. No unsafe snapshot shortcut introduced |
| O-4 | overstated: cached section rows; visible text remains formatted when drawing. Galley shaping already cached; no measured evidence for the report's allocation/frame-time estimates |
| O-5 | overstated: removed unconditional context-menu path clone. Binary-search elision remains Unicode-safe; ASCII width summation would break proportional fonts and Unicode |
| O-6 | by-design: worker polling favours fresh Git state. `--untracked-files=normal` would remove individual nested untracked files from staging. Do not trade away that feature as a purported behaviour-preserving optimization |
| O-7 | REAL-open: paint keeps a String pool across rows; outer-Vec reuse alone was insufficient. Comments now describe the actual reuse |
| O-8 | overstated: divider IDs hash the borrowed path rather than clone it. Two bounded small-tree walks remain. Suggested direction/index-only IDs collide in nested splits |
| O-9 | REAL-open: one process index per adopted snapshot serves all panes; native agents at the process root are also identified |
| O-10 | REAL-open: font-family Arc names interned once |
| O-11 | REAL-open docs/measurement: historical local benchmarks labelled by revision/profile; no old figures reused as current measurements. Frame overlay now includes egui layout/tessellation/upload, excluding vsync |
| O-12 | by-design: settings edit a Config copy, preserving draft/live transition semantics. Quota settings consume an owned snapshot; no profiler evidence makes its bounded copy a defect |
| O-13 | REAL-open partial overhead: tooltip/folded text allocation moved into hover closures; layout jobs still needed to draw the bar |

## Closed with reasons

| ID | Classification and rationale |
|---|---|
| First audit / SQLite WAL note | by-design: read-only database handles can create coordination sidecars. Live WAL is tested with a missing SHM and latest committed rows; database/WAL bytes remain unchanged. `immutable=1` would ignore live changes/locking and is not an appropriate fix |
| Functional F-4: OSC title | by-design: a new shell must not impersonate the old agent's task title. Custom tab titles persist; agent restart is a separate opt-in |
| Functional F-9 | by-design: current request arms all clear the correct busy/generation flags. Speculation about a future variant is not a present user-visible defect |
| Security M-1 | overstated: pager/askpass execution is independently suppressed. Explicit no-pager and documented environment contract guard future refactors |
| Security M-2 | by-design: custom user command is trusted code, already documented. The staged diff is input, not authority to configure that command; arbitrary argv is not restored by the new agent feature |
| Security L-1 | by-design: NUL framing, literal pathspecs and argv protect staging. Snapshot races with another same-user writer remain the stated boundary; added canonicalisation would not make staging atomic |
| Security L-2 | already-covered: verbatim UNC fails closed with an existing regression. Ordinary drive-path carve-out is not an UNC exception |
| Security L-3 | by-design: external viewer associations and same-user reparse races are trust boundaries. Prefix sniffing is not a malware scanner; unsupported/executable formats are revealed rather than run |
| Security I-1 | by-design: bounded in-memory approval cache safely re-prompts at capacity |
| Security I-2 | already-covered: cleanup requires owned markers, bounded content and non-reparse entries; failed process enumeration does not delete current state |
| SC-03 | false-positive for “unpinned”: exact toolchain/tool versions are already pinned. Their separate MSRV and manual update trigger are documented in DEPENDENCIES.md |
| SC-08 | by-design: ASLR/DEP are checked; CFG/CET are not claimed. A linker flag alone is not evidence that Rust control-flow guards are effective |
| SC-09 | by-design: reviewed locked Cargo build scripts are trusted build inputs; additional exact version syntax does not change that model |
| SC-10 | by-design maintenance observation: withdrawn json5 advisory is not an active vulnerability. Parser upgrade has its own tested migration plan |
| SC-13 | by-design: ignored historical notes are not build inputs. Curated shipped references/review/maintenance plan are tracked and link-checked; no claim attests ignored local notes |
| SC-14 | by-design: source and generated Seti notices intentionally coexist and are regenerated together |
| DEAD-01 | REAL-open: unused kv_row removed |
| DEAD-02 | by-design: localisation/persisted compatibility names are not evidence of a dead feature. Scheme aliases fix the actual defect |
| GHOST-02 | already-fixed: capture/choreography are debug-only, absent from release binaries |
| ENCAP-01 | by-design: public library/test entry points are not dead code. No blanket visibility churn without an API change need |
| DUP-01/02/03/05/06/08 | by-design: similarly shaped provider adapters/egui rows are not proven duplicate semantics. Separate schemas retain explicit parsing; reset-key inconsistency fixed independently |
| CX-01/02/03, COV-01/02 | by-design observations: line/test-density counts do not demonstrate a bug. Added regressions target ownership, trust, parsing and restoration, not mirrored implementation or a blanket UI rewrite |
| COV-03 | REAL-open docs: configuration schema and helper environment contract now documented in the English reference |

## Documentation report

H1 was fixed in code. M1–M7 concern ignored historical working notes: they are
labelled as historical rather than replacing old measurements with invented
current numbers; obsolete `--features codex` build syntax was corrected.
Aider keys travel through env, not a temporary key file. The old notes and
their hashes/counts are not shipped documentation or current validation.

L1 uses actual executed results below rather than static test attributes. L2
clarifies the licence source/staging distinction. L3 labels README shortcuts
as selected defaults and links the full reference/settings. L4/L5/COV-03 are
covered by the configuration table and Aider behaviour. L6 clarifies that
routine Git keys themselves do not require trust; existing hooks can still
require it. L7 tracks/checks curated English docs. L8 now enforces a graphics
version request and handles failure. L9 keeps split-down terminology consistent.

## Verification

Validation results are recorded after the final local run in AUDIT.md. Runtime
limits: provider APIs remain fixture-tested; no model prompt was submitted.
RDP/VM driver failure and signing-key custody require their actual environment,
not a static code claim. The demo uses real prestarted sessions and a real pane
drag; it is not a cold-start benchmark.

The reports supplied useful overlapping trust, UI and file-preview findings.
The functional/security reviewers traced behaviour well; performance/dead-code
reports mixed real overhead with unmeasured severity, API misunderstandings and
style preferences. Those observations are closed with reasons here rather than
silently treated as proven vulnerabilities.
