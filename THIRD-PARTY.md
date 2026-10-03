# Third-party notices

ANVIL itself is licensed **GPL-3.0-or-later** (see `LICENSE`).

The released binaries embed or ship the following third-party assets. Full
licence texts are kept in the repository and are staged into `LICENSES/` of the
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

The Rust dependencies in `Cargo.lock` are used under their own licences (mostly
MIT or Apache-2.0); `cargo metadata --format-version 1` reports the exact set,
and `cargo about`/`cargo deny` can generate a combined listing if one is needed.

`scripts/gen_seti_icons.py` regenerates the Seti icon table; its inputs are
vendored under `vendor/seti/` with provenance and a pinned `fontTools` version.
