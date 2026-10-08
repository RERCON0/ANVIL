# Dependency maintenance

## Dependabot PR 2

[PR #2](https://github.com/RERCON0/ANVIL/pull/2), reviewed at
`d5a20373109e5b30ea9556be313039c5220c3e3a`, proposes egui/egui_glow 0.36.2,
glow 0.18, json5 1.3, windows-sys 0.61 and windows-core 0.100.
The Windows job fails because the graphics stack needs Rust 1.95 while ANVIL
pins 1.92. These are deliberate major upgrades, not a safe automatic merge.

Review in separate groups:

1. **Graphics and MSRV:** choose the new Rust minimum explicitly, update
   egui/egui_glow/glow together, and run narrow-pane, terminal glyph, input,
   IME, drag and graphics-initialisation checks on Windows.
2. **Windows bindings:** windows-core must match the `windows` crate used by
   COM macros. Do not upgrade core independently while windows stays 0.58.
   Exercise Recycle Bin, Explorer selection, Credential Manager and process I/O.
3. **Configuration parser:** evaluate json5 independently with malformed,
   bounded and existing configuration fixtures. The old 0.4 line is a
   maintenance concern; its withdrawn unmaintained advisory is not a current
   RustSec vulnerability finding.

Every group needs locked Clippy/tests, current RustSec checks, actual ConPTY
integration, package validation and signed release verification. Provider
fixtures are not a claim of live API compatibility. Local audit fixes do not
silently merge or close the remote PR.

## Build and security tools

The product toolchain is pinned in `rust-toolchain.toml`. Security CI separately
pins Rust 1.99.0 and cargo-audit 0.22.2 to build the audit tool; they are not
unpinned inputs or the application's minimum Rust version. Dependabot does
not update versions embedded in workflow `run` text: review these pins when
the tool's MSRV changes or the scheduled security job fails.

Gitleaks/actionlint archives and icon generation have pinned hashes. Actions
have full SHA pins. ConPTY source archive/publisher provenance is recorded
under `vendor/conpty`; Seti inputs and historical generator limits are described
under `vendor/seti`. Build scripts are trusted locked Cargo inputs, not sandboxed
code. An exact root-crate version range would not remove that trust boundary.
