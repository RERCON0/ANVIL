//! Hinted terminal glyphs. epaint rasterizes with ab_glyph, which ignores a
//! font's hinting: a stem that falls between two pixels comes out as two grey
//! columns and one that lands on the grid as a single bright one, so the same
//! line of Consolas looked bold in one letter and thin in the next. Grid text
//! is rasterized by DirectWrite instead, with the hinting and the rendering
//! mode the font asks for (what Windows Terminal and Chromium draw), into an
//! atlas the terminal paints from. Coverage still becomes alpha the way epaint
//! does it for all other text: the atlas is an epaint font image.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use egui::epaint::FontImage;
use egui::{Color32, Context, Id, Mesh, Pos2, Rect, TextureHandle, TextureId, TextureOptions, Vec2};

use crate::fonts::TermFaces;

/// Atlas side in texels. One face at one size takes about a hundred glyphs of
/// a few hundred texels each, so every style at several zoom steps fits before
/// the atlas has to start over.
pub const ATLAS: usize = 1024;

// Missing/blank glyphs consume no atlas texels, so texture capacity alone does
// not bound the map, especially across many physical font sizes.
const MAX_CACHED_GLYPHS: usize = 16_384;

/// One of the four faces of `TermFaces`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl Face {
    pub fn for_style(bold: bool, italic: bool) -> Face {
        match (bold, italic) {
            (false, false) => Face::Regular,
            (true, false) => Face::Bold,
            (false, true) => Face::Italic,
            (true, true) => Face::BoldItalic,
        }
    }
}

/// A glyph in the atlas: its bitmap's top-left corner relative to the pen on
/// the baseline and its size, in physical pixels, and the corner's texel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub offset: [i32; 2],
    pub size: [usize; 2],
    pub texel: [usize; 2],
}

/// A rasterized glyph: coverage per pixel, row by row.
pub struct Bitmap {
    pub offset: [i32; 2],
    pub size: [usize; 2],
    pub coverage: Vec<u8>,
}

/// What the rasterizer made of a character.
pub enum Raster {
    Ink(Bitmap),
    /// The face has the glyph but it draws nothing (a space).
    Blank,
    /// The face has no glyph for it, or DirectWrite failed on it.
    Missing,
}

#[derive(Clone, Copy)]
enum Slot {
    Ink(Glyph),
    Blank,
    Missing,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    face: Face,
    ch: char,
    /// Em size in physical pixels, as f32 bits.
    em: u32,
}

/// Rows of glyphs filled left to right; a glyph that does not fit the current
/// row opens the next one.
#[derive(Debug, Default, PartialEq)]
struct Shelves {
    x: usize,
    y: usize,
    height: usize,
}

impl Shelves {
    /// Top-left texel for a `w`×`h` box, or None when the atlas is full.
    fn place(&mut self, w: usize, h: usize) -> Option<[usize; 2]> {
        if w > ATLAS {
            return None;
        }
        if self.x + w > ATLAS {
            self.y += self.height;
            self.x = 0;
            self.height = 0;
        }
        if self.y + h > ATLAS {
            return None;
        }
        let at = [self.x, self.y];
        self.x += w;
        self.height = self.height.max(h);
        Some(at)
    }
}

pub struct TermGlyphs {
    faces: TermFaces,
    raster: dwrite::Rasterizer,
    shelves: Shelves,
    texture: Option<TextureHandle>,
    glyphs: HashMap<Key, Slot>,
    /// The pass in which the atlas ran out of room. It starts over in a later
    /// pass, when no mesh of that pass points into it any more; until then new
    /// glyphs are left to epaint.
    full_since: Option<u64>,
}

impl TermGlyphs {
    /// The atlas texture, once a glyph has been stored in it.
    pub fn texture_id(&self) -> Option<TextureId> {
        self.texture.as_ref().map(|t| t.id())
    }

    /// The glyphs of a grid-aligned run, one character per cell: (index of
    /// the cell in the run, glyph) for every character that draws something.
    /// None when any of them has to be drawn by epaint instead (the face has
    /// no such glyph, or the atlas is full for this pass).
    pub fn run(&mut self, ctx: &Context, face: Face, em: f32, text: &str) -> Option<Vec<(usize, Glyph)>> {
        let mut out = Vec::with_capacity(text.len());
        for (i, ch) in text.chars().enumerate() {
            match self.slot(ctx, face, ch, em)? {
                Slot::Ink(glyph) => out.push((i, glyph)),
                Slot::Blank => {}
                Slot::Missing => return None,
            }
        }
        Some(out)
    }

    fn slot(&mut self, ctx: &Context, face: Face, ch: char, em: f32) -> Option<Slot> {
        let pass = ctx.cumulative_pass_nr();
        if self.full_since.is_some_and(|full| full != pass) {
            self.shelves = Shelves::default();
            self.glyphs.clear();
            self.full_since = None;
        }
        let key = Key { face, ch, em: em.to_bits() };
        if let Some(slot) = self.glyphs.get(&key) {
            return Some(*slot);
        }
        if self.glyphs.len() >= MAX_CACHED_GLYPHS {
            self.full_since = Some(pass);
        }
        if self.full_since.is_some() {
            return None;
        }
        let slot = match self.raster.rasterize(face, ch, em) {
            Raster::Missing => Slot::Missing,
            Raster::Blank => Slot::Blank,
            Raster::Ink(bitmap) => match self.store(ctx, &bitmap) {
                Some(glyph) => Slot::Ink(glyph),
                None => {
                    self.full_since = Some(pass);
                    return None;
                }
            },
        };
        self.glyphs.insert(key, slot);
        Some(slot)
    }

    fn store(&mut self, ctx: &Context, bitmap: &Bitmap) -> Option<Glyph> {
        let [w, h] = bitmap.size;
        // An empty texel right of and below every glyph keeps neighbours apart.
        let texel = self.shelves.place(w + 1, h + 1)?;
        let texture = self.texture.get_or_insert_with(|| {
            ctx.load_texture("term-glyphs", FontImage::new([ATLAS, ATLAS]), TextureOptions::NEAREST)
        });
        let mut image = FontImage::new([w + 1, h + 1]);
        for y in 0..h {
            for x in 0..w {
                image.pixels[y * (w + 1) + x] = bitmap.coverage[y * w + x] as f32 / 255.0;
            }
        }
        texture.set_partial(texel, image, TextureOptions::NEAREST);
        Some(Glyph { offset: bitmap.offset, size: bitmap.size, texel })
    }
}

/// Adds `glyph` in `color` to `mesh` with its pen at `pen`, a point on the
/// baseline in whole physical pixels: the bitmap maps onto the screen texel
/// for pixel.
pub fn add_quad(mesh: &mut Mesh, glyph: Glyph, pen: [i32; 2], pixels_per_point: f32, color: Color32) {
    let min = Pos2::new((pen[0] + glyph.offset[0]) as f32, (pen[1] + glyph.offset[1]) as f32);
    let size = Vec2::new(glyph.size[0] as f32, glyph.size[1] as f32);
    let rect = Rect::from_min_size((min.to_vec2() / pixels_per_point).to_pos2(), size / pixels_per_point);
    let texel = Pos2::new(glyph.texel[0] as f32, glyph.texel[1] as f32);
    let uv = Rect::from_min_size((texel.to_vec2() / ATLAS as f32).to_pos2(), size / ATLAS as f32);
    mesh.add_rect_with_uv(rect, uv, color);
}

#[derive(Clone)]
struct Shared(Arc<Mutex<TermGlyphs>>);

fn key() -> Id {
    Id::new("anvil-term-glyphs")
}

/// Pairs the terminal faces `fonts::install` just gave epaint with their
/// DirectWrite twins. Without DirectWrite the terminal keeps epaint's glyphs.
pub fn install(ctx: &Context, faces: TermFaces) {
    if shared(ctx).is_some_and(|current| current.lock().is_ok_and(|g| g.faces == faces)) {
        return;
    }
    match dwrite::Rasterizer::new(&faces) {
        Ok(raster) => {
            let glyphs = TermGlyphs {
                faces,
                raster,
                shelves: Shelves::default(),
                texture: None,
                glyphs: HashMap::new(),
                full_since: None,
            };
            ctx.data_mut(|d| d.insert_temp(key(), Shared(Arc::new(Mutex::new(glyphs)))));
        }
        Err(e) => {
            log::warn!("DirectWrite cannot load the terminal font, its text is drawn unhinted: {e}");
            ctx.data_mut(|d| d.remove::<Shared>(key()));
        }
    }
}

/// The hinted glyphs of the installed terminal font, if DirectWrite has them.
pub fn shared(ctx: &Context) -> Option<Arc<Mutex<TermGlyphs>>> {
    ctx.data(|d| d.get_temp::<Shared>(key())).map(|s| s.0)
}

#[cfg(windows)]
mod dwrite {
    use std::mem::ManuallyDrop;
    use std::os::windows::ffi::OsStrExt;

    use windows::core::{Interface, Result, PCWSTR};
    use windows::Win32::Foundation::{BOOL, DWRITE_E_FILEFORMAT};
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_TEXTURE_ALIASED_1x1, DWriteCreateFactory, IDWriteFactory5, IDWriteFontFace, IDWriteFontFace3,
        IDWriteFontFile, IDWriteInMemoryFontFileLoader, IDWriteRenderingParams, DWRITE_FACTORY_TYPE_ISOLATED,
        DWRITE_FONT_FACE_TYPE, DWRITE_FONT_FILE_TYPE, DWRITE_FONT_SIMULATIONS_NONE, DWRITE_GLYPH_RUN,
        DWRITE_GRID_FIT_MODE, DWRITE_MEASURING_MODE_NATURAL, DWRITE_OUTLINE_THRESHOLD_ANTIALIASED,
        DWRITE_RENDERING_MODE1, DWRITE_RENDERING_MODE1_DEFAULT, DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC,
        DWRITE_RENDERING_MODE1_OUTLINE, DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
    };

    use super::{Bitmap, Face, Raster};
    use crate::fonts::{FaceSource, TermFaces, CASCADIA};

    pub struct Rasterizer {
        factory: IDWriteFactory5,
        /// The system's text settings, which the recommended rendering mode
        /// follows (ClearType tuner, a disabled font smoothing).
        params: IDWriteRenderingParams,
        /// In `Face` order.
        faces: [IDWriteFontFace3; 4],
    }

    impl Rasterizer {
        pub fn new(faces: &TermFaces) -> Result<Rasterizer> {
            // SAFETY: plain DirectWrite calls; the generated bindings own every
            // interface they return, and the bundled font is 'static data that
            // DirectWrite copies (no owner object is passed).
            unsafe {
                // Isolated: the in-memory loader registered here dies with this
                // factory instead of piling up in the process-wide one.
                let factory: IDWriteFactory5 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_ISOLATED)?;
                let params = factory.CreateRenderingParams()?;
                let memory: IDWriteInMemoryFontFileLoader = factory.CreateInMemoryFontFileLoader()?;
                factory.RegisterFontFileLoader(&memory)?;
                let open = |source: &FaceSource| -> Result<IDWriteFontFace3> {
                    let file: IDWriteFontFile = match source {
                        FaceSource::File(path) => {
                            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
                            factory.CreateFontFileReference(PCWSTR(wide.as_ptr()), None)?
                        }
                        FaceSource::Bundled => memory.CreateInMemoryFontFileReference(
                            &factory,
                            CASCADIA.as_ptr().cast(),
                            CASCADIA.len() as u32,
                            None,
                        )?,
                    };
                    let mut supported = BOOL::default();
                    let mut file_type = DWRITE_FONT_FILE_TYPE::default();
                    let mut face_type = DWRITE_FONT_FACE_TYPE::default();
                    let mut count = 0;
                    file.Analyze(&mut supported, &mut file_type, Some(&mut face_type), &mut count)?;
                    if !supported.as_bool() {
                        return Err(DWRITE_E_FILEFORMAT.into());
                    }
                    // Face 0 of a collection, as epaint loads it.
                    factory.CreateFontFace(face_type, &[Some(file)], 0, DWRITE_FONT_SIMULATIONS_NONE)?.cast()
                };
                let faces =
                    [open(&faces.regular)?, open(&faces.bold)?, open(&faces.italic)?, open(&faces.bold_italic)?];
                Ok(Rasterizer { factory, params, faces })
            }
        }

        /// `ch` of `face` at `em` physical pixels, hinted and grey-scale
        /// anti-aliased, its bitmap placed relative to a pen at (0, 0) on the
        /// baseline.
        pub fn rasterize(&self, face: Face, ch: char, em: f32) -> Raster {
            let face = &self.faces[face as usize];
            match self.try_rasterize(face, ch, em) {
                Ok(raster) => raster,
                Err(e) => {
                    log::warn!("DirectWrite cannot draw {ch:?}: {e}");
                    Raster::Missing
                }
            }
        }

        fn try_rasterize(&self, face: &IDWriteFontFace3, ch: char, em: f32) -> Result<Raster> {
            // SAFETY: every pointer handed over points at a local that
            // outlives the call; the glyph run's face reference is released
            // right after the analysis is created.
            unsafe {
                let mut index = 0u16;
                face.GetGlyphIndices(&(ch as u32), 1, &mut index)?;
                if index == 0 {
                    return Ok(Raster::Missing);
                }
                let mut mode = DWRITE_RENDERING_MODE1::default();
                let mut grid_fit = DWRITE_GRID_FIT_MODE::default();
                face.GetRecommendedRenderingMode(
                    em,
                    96.0,
                    96.0,
                    None,
                    false,
                    DWRITE_OUTLINE_THRESHOLD_ANTIALIASED,
                    DWRITE_MEASURING_MODE_NATURAL,
                    &self.params,
                    &mut mode,
                    &mut grid_fit,
                )?;
                // Outlines are what DirectWrite recommends once a size is too
                // large for hinting to matter; an analysis cannot draw them.
                if mode == DWRITE_RENDERING_MODE1_OUTLINE || mode == DWRITE_RENDERING_MODE1_DEFAULT {
                    mode = DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC;
                }
                let advance = 0.0f32;
                let run = DWRITE_GLYPH_RUN {
                    fontFace: ManuallyDrop::new(Some(face.cast::<IDWriteFontFace>()?)),
                    fontEmSize: em,
                    glyphCount: 1,
                    glyphIndices: &index,
                    glyphAdvances: &advance,
                    glyphOffsets: std::ptr::null(),
                    isSideways: false.into(),
                    bidiLevel: 0,
                };
                let analysis = self.factory.CreateGlyphRunAnalysis(
                    &run,
                    None,
                    mode,
                    DWRITE_MEASURING_MODE_NATURAL,
                    grid_fit,
                    DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                    0.0,
                    0.0,
                );
                drop(ManuallyDrop::into_inner(run.fontFace));
                let analysis = analysis?;
                // Grey-scale analyses hand out one coverage byte per pixel as
                // the "aliased" texture.
                let bounds = analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)?;
                let (w, h) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
                if w <= 0 || h <= 0 {
                    return Ok(Raster::Blank);
                }
                let mut coverage = vec![0u8; (w * h) as usize];
                analysis.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &bounds, &mut coverage)?;
                Ok(Raster::Ink(Bitmap { offset: [bounds.left, bounds.top], size: [w as usize, h as usize], coverage }))
            }
        }
    }
}

#[cfg(not(windows))]
mod dwrite {
    use super::{Face, Raster};
    use crate::fonts::TermFaces;

    pub struct Rasterizer;

    impl Rasterizer {
        pub fn new(_faces: &TermFaces) -> Result<Rasterizer, &'static str> {
            Err("DirectWrite is Windows-only")
        }

        pub fn rasterize(&self, _face: Face, _ch: char, _em: f32) -> Raster {
            Raster::Missing
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelves_fill_rows_and_report_a_full_atlas() {
        let mut shelves = Shelves::default();
        assert_eq!(shelves.place(10, 20), Some([0, 0]));
        assert_eq!(shelves.place(12, 18), Some([10, 0]));
        assert_eq!(shelves.place(ATLAS - 22, 5), Some([22, 0]), "the row is filled exactly");
        assert_eq!(shelves.place(1, 1), Some([0, 20]), "the next row starts below the tallest glyph");
        assert_eq!(shelves.place(ATLAS + 1, 1), None, "wider than the atlas");
        assert_eq!(shelves.place(4, ATLAS), None, "no room left below");
    }
}
