//! Draws the ANVIL icon: a simple anvil silhouette with the project accent on
//! a transparent background. Writes icons/anvil-256.png (the runtime window
//! icon) and a multi-size icons/anvil.ico (the executable icon).
//!
//! Run from the repository root: `cargo run --example gen_icons`.

use std::fs::File;
use std::path::Path;

use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::{ExtendedColorType, RgbaImage};

/// Supersampled canvas; each output size is a box-filtered reduction.
const SS: u32 = 1024;
const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

const BODY: [u8; 3] = [0xC8, 0xC8, 0xC8];
const BODY_DARK: [u8; 3] = [0x9A, 0x9A, 0x9A];
const TOP: [u8; 3] = [0xE2, 0xE2, 0xE2];
const ACCENT: [u8; 3] = [0x59, 0xD6, 0x8C];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let canvas = draw();
    let dir = Path::new("icons");
    std::fs::create_dir_all(dir)?;

    let png = downsample(&canvas, 256);
    RgbaImage::from_raw(256, 256, png.clone()).expect("256x256 buffer").save(dir.join("anvil-256.png"))?;

    let mut frames = Vec::new();
    for size in SIZES {
        let pixels = downsample(&canvas, size);
        frames.push(IcoFrame::as_png(&pixels, size, size, ExtendedColorType::Rgba8)?);
    }
    let file = File::create(dir.join("anvil.ico"))?;
    IcoEncoder::new(file).encode_images(&frames)?;

    std::fs::write(dir.join("anvil.rc"), "1 ICON \"anvil.ico\"\n")?;
    println!("wrote icons/anvil-256.png and icons/anvil.ico ({SIZES:?})");
    Ok(())
}

/// A supersampled RGBA canvas with the anvil silhouette.
fn draw() -> Vec<u8> {
    let mut canvas = vec![0u8; (SS * SS * 4) as usize];
    let u = |v: f32| (v * SS as f32) as i32;
    // Horn, body, top face with the accent stripe, waist and base.
    polygon(&mut canvas, &[(u(0.10), u(0.34)), (u(0.34), u(0.29)), (u(0.34), u(0.45))], BODY);
    rectangle(&mut canvas, u(0.30), u(0.27), u(0.87), u(0.44), BODY);
    rectangle(&mut canvas, u(0.30), u(0.27), u(0.87), u(0.32), TOP);
    rectangle(&mut canvas, u(0.30), u(0.27), u(0.87), u(0.30), ACCENT);
    rectangle(&mut canvas, u(0.44), u(0.44), u(0.67), u(0.64), BODY_DARK);
    polygon(
        &mut canvas,
        &[(u(0.28), u(0.64)), (u(0.83), u(0.64)), (u(0.94), u(0.82)), (u(0.17), u(0.82))],
        BODY,
    );
    canvas
}

fn rectangle(canvas: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32, color: [u8; 3]) {
    polygon(canvas, &[(x0, y0), (x1, y0), (x1, y1), (x0, y1)], color);
}

/// Scanline fill of a convex or simple polygon.
fn polygon(canvas: &mut [u8], points: &[(i32, i32)], color: [u8; 3]) {
    let s = SS as i32;
    let ys: Vec<i32> = points.iter().map(|p| p.1).collect();
    let y0 = ys.iter().copied().min().unwrap_or(0).max(0);
    let y1 = ys.iter().copied().max().unwrap_or(0).min(s - 1);
    for y in y0..=y1 {
        let scan = y as f32 + 0.5;
        let mut nodes: Vec<f32> = Vec::new();
        let mut j = points.len() - 1;
        for i in 0..points.len() {
            let (xi, yi) = (points[i].0 as f32, points[i].1 as f32);
            let (xj, yj) = (points[j].0 as f32, points[j].1 as f32);
            if (yi < scan && scan <= yj) || (yj < scan && scan <= yi) {
                nodes.push(xi + (scan - yi) / (yj - yi) * (xj - xi));
            }
            j = i;
        }
        nodes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in nodes.chunks_exact(2) {
            let (xa, xb) = (pair[0], pair[1]);
            for x in (xa.round() as i32).max(0)..=(xb.round() as i32).min(s - 1) {
                let offset = ((y as u32 * SS + x as u32) * 4) as usize;
                canvas[offset] = color[0];
                canvas[offset + 1] = color[1];
                canvas[offset + 2] = color[2];
                canvas[offset + 3] = 255;
            }
        }
    }
}

/// Box-filtered reduction of the supersampled canvas.
fn downsample(canvas: &[u8], size: u32) -> Vec<u8> {
    let factor = SS / size;
    let mut out = vec![0u8; (size * size * 4) as usize];
    let area = factor * factor;
    for y in 0..size {
        for x in 0..size {
            let mut sum = [0u32; 4];
            for dy in 0..factor {
                for dx in 0..factor {
                    let offset = (((y * factor + dy) * SS + x * factor + dx) * 4) as usize;
                    for (channel, value) in sum.iter_mut().enumerate() {
                        *value += canvas[offset + channel] as u32;
                    }
                }
            }
            let offset = ((y * size + x) * 4) as usize;
            for (channel, value) in sum.iter().enumerate() {
                out[offset + channel] = (value / area) as u8;
            }
        }
    }
    out
}
