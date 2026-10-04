//! Builds the ANVIL icons from icons/source/mark-chevron.png: writes
//! icons/anvil-256.png (the runtime window icon) and a multi-size
//! icons/anvil.ico (the executable icon).
//!
//! Run from the repository root: `cargo run --example gen_icons`.

use std::fs::File;
use std::path::Path;

use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::imageops::FilterType;
use image::{ExtendedColorType, Rgba, RgbaImage};

/// White freeform terminal chevron with a charcoal contour and transparent field.
const SOURCE: &str = "icons/source/mark-chevron.png";
const SIZES: [u32; 10] = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = Path::new("icons");
    std::fs::create_dir_all(dir)?;
    let square = square_canvas(image::open(SOURCE)?.into_rgba8());

    icon_frame(&square, 256).save(dir.join("anvil-256.png"))?;

    let mut frames = Vec::new();
    for size in SIZES {
        let scaled = icon_frame(&square, size);
        frames.push(IcoFrame::as_png(scaled.as_raw(), size, size, ExtendedColorType::Rgba8)?);
    }
    IcoEncoder::new(File::create(dir.join("anvil.ico"))?).encode_images(&frames)?;

    std::fs::write(dir.join("anvil.rc"), "1 ICON \"anvil.ico\"\n")?;
    println!("wrote icons/anvil-256.png and icons/anvil.ico ({SIZES:?}) from {SOURCE}");
    Ok(())
}

/// Pad artwork onto a transparent square if needed so the
/// mark stays centred and keeps its proportions at every size.
fn square_canvas(source: RgbaImage) -> RgbaImage {
    let (width, height) = source.dimensions();
    let side = width.max(height);
    let mut canvas = RgbaImage::new(side, side);
    image::imageops::overlay(&mut canvas, &source, ((side - width) / 2) as i64, ((side - height) / 2) as i64);
    canvas
}

/// Keep the charcoal edge visible at taskbar sizes without adding a backdrop.
/// Work at 4x resolution so even a subpixel contour stays smoothly antialiased.
fn icon_frame(square: &RgbaImage, size: u32) -> RgbaImage {
    const SCALE: u32 = 4;
    let side = size * SCALE;
    let scaled = image::imageops::resize(square, side, side, FilterType::Lanczos3);
    let radius = ((size as f32 * 0.006).max(0.75) * SCALE as f32).ceil() as i32;
    let mut contour = RgbaImage::new(side, side);
    let offsets: Vec<_> = (-radius..=radius)
        .flat_map(|dy| (-radius..=radius).map(move |dx| (dx, dy)))
        .filter(|(dx, dy)| dx * dx + dy * dy <= radius * radius)
        .collect();
    for (x, y, pixel) in contour.enumerate_pixels_mut() {
        let alpha = offsets
            .iter()
            .filter_map(|(dx, dy)| {
                let sx = x as i32 + dx;
                let sy = y as i32 + dy;
                (sx >= 0 && sy >= 0 && sx < side as i32 && sy < side as i32)
                    .then(|| scaled.get_pixel(sx as u32, sy as u32)[3])
            })
            .max()
            .unwrap_or(0);
        *pixel = Rgba([17, 19, 22, alpha]);
    }
    image::imageops::overlay(&mut contour, &scaled, 0, 0);
    image::imageops::resize(&contour, size, size, FilterType::Lanczos3)
}
