# Dependency maintenance

## Reviewed upgrades (2026-10-12)

The changes proposed in Dependabot PRs #3, #4 and #5 are integrated with
compatibility fixes: Rust 1.99.0, egui/egui_glow 0.36.2, json5 1.3.1,
windows-sys 0.61 and windows/windows-core 0.62.2.

- **Graphics:** egui_glow 0.36.2 uses glow 0.17. ANVIL keeps that same version
  so its OpenGL context has the exact type expected by the painter. A standalone
  glow 0.18 upgrade creates incompatible context types.
- **Windows:** windows-core must match the `windows` crate that generates
  the COM interfaces. Upgrading core alone to 0.100 breaks that pair; both
  direct crates now use 0.62.2. The Recycle Bin progress sink retains its
  permanent-deletion veto with the new borrowed COM reference API.
- **JSON5:** error positions are converted from zero-based to one-based
  coordinates. Diagnostics still omit source text and credentials.
- **Input and textures:** modifiers arrive as input events; all texture
  updates for an atlas are applied in order before painting. Headless tests
  explicitly discard GPU updates after inspecting their UI output.
- **Glyph detection:** epaint 0.36's `has_glyph` can reject valid glyphs that
  share the replacement character's face. Terminal base-cell detection uses
  the actual glyph advance instead; combining marks are rendered separately.
  Tests cover supported primary-font text and a genuinely absent glyph so the
  upgrade does not trigger unnecessary fallback-font loading.

Dependabot reviews graphics, Windows bindings and other dependencies in
separate groups. Standalone minor/major upgrades of glow and windows-core are
held until their parent libraries use those versions; patch updates remain
enabled. Review these holds during the next egui_glow/windows migration.

Validation includes locked Clippy/tests, current RustSec checks, actual ConPTY
and Git integration, documentation and package checks in CI. Provider fixtures
do not claim live API compatibility. This maintenance update creates no release
and does not change existing signed packages.

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
