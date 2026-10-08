# Performance report triage — 2026-10-08

This reviews the additional static-only performance report supplied on
2026-10-08, against the local ANVIL 0.1.1 sources after `51de931`. Its O-1…O-9
identifiers belong to that report, not the earlier audit's findings.

## Findings and disposition

| ID | Verified behaviour | Disposition and evidence |
|---|---|---|
| O-1 | The unconditional one-second repaint rebuilt visible terminal frames even when periodic discovery found nothing new. Cursor blinking adds redraws when requested, but the configured default is **off**. | **Fixed.** Config/status discovery now runs on a separate event-loop maintenance deadline, without a mandatory drawing pass. Changed config, badges, CLI detection, profiles and quota time labels request repaint; unchanged status does not. Blink scheduling also requires OS window focus. Tests cover config reload without drawing, unchanged badges, and preservation of a later animation deadline across a maintenance wake. |
| O-2 | Compiling the regex was cached, but preparing the query and scanning visible matches still happened each search frame. | **Fixed.** Cache the prepared query and up to 400 visible matches. The key includes query flags/generation, terminal output generation, scroll offset, history size and grid dimensions. Output generation is independent of the consumable activity flag and coalesced wake notifications. Tests use a real terminal to check output, scrolling, resizing and query changes. Explicit clear/resize/options changes invalidate the cache. |
| O-3 | Elision probes allocated/layout-hashed candidates repeatedly; file names were measured and drawn through separate layouts. | **Fixed for the repeated elision/layout work.** A bounded 256-entry cache retains exact-source galleys, keyed by font, width, color and direction; font/atlas or DPI changes invalidate it. File rows measure and draw the same galley. Clean display text can be borrowed; change statistics use one output buffer. Raw Git filenames/subjects remain intact. Time/hash labels and some small row formatting remain; no measured bottleneck justifies caching every label. Commit tooltips were already computed only while hovered. Tests cover Unicode elision, content/width/color/DPI/font changes and retention bounds. |
| O-4 | Tab metadata is rebuilt, and badge display parts were reformatted on each frame. | **Partly fixed; the reported title allocation was already absent.** `TabInfo.title` was already a `Cow<str>` borrowing the tab title: `.into()` does not allocate a `String` here. Badge parts now follow record/field changes, and tab/badge text uses the bounded galley cache. Small index labels and the metadata vector remain; no measurement establishes a need to complicate them. Badge tests cover record/field changes and replacing the tab at an index. |
| O-5 | Claude status paths and pane IDs were formatted at each one-second poll. | **Fixed allocations; polling retained by design.** Store the status path once per pane. File metadata is still checked once per second to preserve existing freshness and detect deletion/replacement. Moving this to a filesystem watcher would require reliable overflow/reconnect handling; it is not a prerequisite to removing forced drawing. |
| O-6 | Log rotation checks the open handle's size on each record. | **By design; recommendation rejected.** Several ANVIL processes can append to and rotate the same log. Another window can rename the current handle to the full `.1` file. A process-local byte counter would miss that change and grow the rotated file. Existing multi-handle/locked-rotation tests protect this behaviour. Release logs are sparse; no measurement demonstrates a bottleneck. |
| O-7 | ANVIL decoded its 256×256 PNG icon at startup. | **Fixed startup decode; dependency claim overstated.** Decode the icon to RGBA during `build.rs`; startup only constructs the native icon. PNG framebuffer writing lives in the development example, not the library. However, `image` remains a normal transitive dependency through arboard/egui's clipboard stack. This is not removal of the entire image library, and no startup/build-time saving is claimed without measurement. An exact pixel-equivalence regression covers the generated icon. |
| O-8 | Both high-level Windows bindings and `windows-sys` appear in the graph. | **By design; no material runtime defect demonstrated.** `cargo tree --locked -d --target x86_64-pc-windows-msvc` confirms multiple transitive versions. Generated COM/DirectWrite interfaces and flat Win32 FFI serve different purposes; changing the direct version cannot remove upstream glutin/winit/clipboard versions. A blanket major upgrade risks recycling/font behaviour. Dependency migration remains covered separately in [DEPENDENCIES](DEPENDENCIES.md). |
| O-9 | Settings edit a clone of the currently applied config. | **By design, with one real adjacent improvement.** The draft preserves old/new comparisons and live-setting transitions, covered by the settings regression. `keymap.describe()` was already a cached borrowed slice, not a newly built vector. Claude settings metadata/read is now throttled to maintenance ticks plus the initial settings-open refresh. No measurement warrants a new config editing model. |

## What the report does not prove

Its operation counts are useful leads, but its estimated CPU percentages and
millisecond savings are not measurements. The original application already
used very little CPU in the measured plain-shell idle case. Removing mandatory
drawing is observable in the scheduling logic; a material CPU improvement must
still be demonstrated on the target workload. See the recorded idle samples
and stress-test conditions in [PERFORMANCE](PERFORMANCE.md).

The statements “all I/O is off the UI thread” and “no allocations per frame” are
too broad. Maintenance still performs bounded synchronous metadata/settings
I/O, saves can call filesystem synchronization, and snapshots can allocate for
combining marks/hyperlinks. Existing buffers and limits reduce those costs;
they do not eliminate them. This change does not introduce a cache of the
entire terminal frame, which would need additional invalidation for selection,
palette and input state.

## Validation and assessment

Executed checks are recorded in [AUDIT](AUDIT.md). The regressions target stale
highlights, stale atlas coordinates, cache growth and missed discovery updates,
rather than asserting a particular implementation or unmeasured speedup.

One run concurrent with compilation and CLI capture failed the existing
process-tree test's one-second descendant-exit wait. The unchanged test then
passed in isolation and in the complete final suite. The cause of that transient
failure was not independently established; no assertion was weakened or skipped.

The reviewer found useful repeat-work candidates and correctly recognized the
bounded caches, workers and demand-driven rendering. The Cow title, cached
hotkey descriptions, clipboard dependency and shared-log rotation need more
careful type/dependency/ownership tracing. The report is useful for directing
measurement, but its severity and expected gains should not be presented as
established performance results.
