//! Builds the ANVIL icons from the design in the repository root: writes
//! icons/anvil-256.png (the runtime window icon) and a multi-size
//! icons/anvil.ico (the executable icon).
//!
//! Run from the repository root: `cargo run --example gen_icons`.

use std::fs::File;
use std::path::Path;

use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::imageops::FilterType;
use image::{ExtendedColorType, RgbaImage};

/// The icon artwork: a rounded dark tile with the accent mark on a
/// transparent field.
const SOURCE: &str = "forge_20261003_173719.png";
const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = Path::new("icons");
    std::fs::create_dir_all(dir)?;
    let square = square_canvas(image::open(SOURCE)?.into_rgba8());

    image::imageops::resize(&square, 256, 256, FilterType::Lanczos3).save(dir.join("anvil-256.png"))?;

    let mut frames = Vec::new();
    for size in SIZES {
        let scaled = image::imageops::resize(&square, size, size, FilterType::Lanczos3);
        frames.push(IcoFrame::as_png(scaled.as_raw(), size, size, ExtendedColorType::Rgba8)?);
    }
    IcoEncoder::new(File::create(dir.join("anvil.ico"))?).encode_images(&frames)?;

    std::fs::write(dir.join("anvil.rc"), "1 ICON \"anvil.ico\"\n")?;
    println!("wrote icons/anvil-256.png and icons/anvil.ico ({SIZES:?}) from {SOURCE}");
    Ok(())
}

/// The artwork is not exactly square: pad it onto a transparent square so the
/// mark stays centred and keeps its proportions at every size.
fn square_canvas(source: RgbaImage) -> RgbaImage {
    let (width, height) = source.dimensions();
    let side = width.max(height);
    let mut canvas = RgbaImage::new(side, side);
    image::imageops::overlay(&mut canvas, &source, ((side - width) / 2) as i64, ((side - height) / 2) as i64);
    canvas
}
