# ANVIL icon

Selected concept 04: a freeform folded terminal chevron.

The mark has a near-white fill and a charcoal contour along its outer and
inner edges. The remaining canvas, including the open central area, is
transparent. The contour gives the white mark a visible boundary on light
backgrounds; the fill is visible on dark backgrounds. There is no enclosing
tile, badge, or background panel.

## Source

- `source/mark-chevron.png`: the selected transparent PNG master.
- `source/mark-chevron-prompt.txt`: the final edit prompt.

Created with the built-in ImageGen tool. Keep the alpha channel when exporting.

## Rebuild

Run from the repository root:

```powershell
cargo run --example gen_icons --locked --offline
```

The native exporter in `examples/gen_icons.rs` reinforces the contour with
size-aware alpha dilation, then downsamples at 4x resolution. This keeps the
small frames readable on light backgrounds while leaving the background and
central opening transparent. It produces:

- `anvil-256.png`: the window icon embedded by `src/app.rs`.
- `anvil.ico`: the executable icon embedded by `build.rs`.
- `anvil.rc`: the Windows resource declaration.

ICO sizes: 16, 20, 24, 32, 40, 48, 64, 96, 128, and 256 pixels.
