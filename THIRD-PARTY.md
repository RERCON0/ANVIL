# Third-party notices

ANVIL itself is licensed **GPL-3.0-or-later** (see `LICENSE`).

The released binaries embed or ship the following third-party assets. Full
licence texts in `fonts/`, `vendor/conpty/` and `LICENSES/egui-default-fonts/`
are staged into `LICENSES/` of the
distribution zip.

| Asset | Where it comes from | Licence | Notice |
|---|---|---|---|
| ConPTY 1.23 (`conpty.dll`, `OpenConsole.exe`) | Windows Terminal | MIT | `vendor/conpty/LICENSE` |
| Cascadia Mono Light | Microsoft | SIL OFL 1.1 | `fonts/OFL-notice.txt` (copyright notice) + `fonts/OFL-1.1.txt` (licence text) |
| Seti UI file icons (`fonts/seti.ttf`, `src/file_icons.rs`) | Seti UI icon theme | MIT | `fonts/seti-LICENSE.txt` |
| Hack Regular | Source Foundry | MIT/Bitstream Vera | `LICENSES/egui-default-fonts/Hack-Regular.txt` |
| Ubuntu Light | Canonical | Ubuntu Font Licence 1.0 | `LICENSES/egui-default-fonts/Ubuntu-Light-UFL.txt` |
| Noto Emoji Regular | Google | SIL OFL 1.1 | `LICENSES/egui-default-fonts/NotoEmoji-OFL.txt` |
| emoji-icon-font | John Slegers | MIT | `LICENSES/egui-default-fonts/emoji-icon-font-MIT.txt` |

The Rust crates compiled into the binaries are used under their own licences
(mostly MIT or Apache-2.0). `scripts/package.ps1` runs
`scripts/collect_licenses.py`, which lists the crates of the release build's
normal dependency graph (`cargo metadata --locked --filter-platform`) with
their licence text, and stages the result as `LICENSES/THIRD-PARTY-RUST.txt` in
the zip. The graph includes compile-time crates such as proc macros that are
not linked into the executables, so the list is wider than needed on purpose:
an extra notice is harmless, a missing one is not. `scripts/check_package.py`
rebuilds the listing and fails when the packaged file differs. A crate that
publishes no licence file is listed with its SPDX identifier and repository URL.

`scripts/gen_seti_icons.py` regenerates the Seti icon table; its inputs are
vendored under `vendor/seti/` with provenance and a pinned `fontTools` version.

`LICENSES/CascadiaMono-OFL.txt` is a historical duplicate notice, not a staged licence. The package uses `fonts/OFL-notice.txt` and `fonts/OFL-1.1.txt`. Seti keeps both its vendored source notice and the generated font notice; generation copies them together.
