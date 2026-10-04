//! Terminal and UI fonts. System fonts (Consolas, Segoe UI and the fallbacks)
//! are loaded at runtime from the Windows font registry, never embedded; only
//! the OFL Cascadia Mono fallback ships inside the exe.

use std::path::{Path, PathBuf};

use egui::{FontData, FontDefinitions, FontFamily, FontId};

pub(crate) const CASCADIA: &[u8] = include_bytes!("../fonts/CascadiaMono-Light.ttf");
/// Seti UI file icons (MIT, see fonts/seti-LICENSE.txt).
const SETI: &[u8] = include_bytes!("../fonts/seti.ttf");

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontFiles {
    pub regular: Option<PathBuf>,
    pub bold: Option<PathBuf>,
    pub italic: Option<PathBuf>,
    pub bold_italic: Option<PathBuf>,
}

/// Picks the four faces of `family` from registry entries such as
/// ("Consolas Bold (TrueType)", "consolab.ttf"). Relative file names are
/// resolved against `fonts_dir` (C:\Windows\Fonts).
pub fn match_family(entries: &[(String, String)], family: &str, fonts_dir: &Path) -> FontFiles {
    let family = family.trim().to_lowercase();
    let mut files = FontFiles::default();
    for (name, file) in entries {
        let name = name.to_lowercase();
        let face = name
            .trim_end_matches(" (truetype)")
            .trim_end_matches(" (opentype)")
            .trim();
        let path = if Path::new(file).is_absolute() { PathBuf::from(file) } else { fonts_dir.join(file) };
        let slot = if face == family || face == format!("{family} regular") {
            &mut files.regular
        } else if face == format!("{family} bold") {
            &mut files.bold
        } else if face == format!("{family} italic") {
            &mut files.italic
        } else if face == format!("{family} bold italic") {
            &mut files.bold_italic
        } else {
            continue;
        };
        slot.get_or_insert(path);
    }
    files
}

/// Where the data of one terminal face came from.
#[derive(Clone, Debug, PartialEq)]
pub enum FaceSource {
    File(PathBuf),
    /// The Cascadia Mono shipped inside the exe.
    Bundled,
}

/// The four terminal faces as `install` gave them to epaint, so the hinted
/// glyphs are drawn from exactly the faces the cell metrics are measured on.
#[derive(Clone, Debug, PartialEq)]
pub struct TermFaces {
    pub regular: FaceSource,
    pub bold: FaceSource,
    pub italic: FaceSource,
    pub bold_italic: FaceSource,
}

pub fn fonts_dir() -> PathBuf {
    std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows")).join("Fonts")
}

/// (value name, data) pairs of HKLM and HKCU `...\Windows NT\CurrentVersion\Fonts`.
#[cfg(windows)]
pub fn registry_font_entries() -> Vec<(String, String)> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ,
    };
    let subkey: Vec<u16> = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Fonts"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut out = Vec::new();
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated key name; `key` is closed below.
        if unsafe { RegOpenKeyExW(root, subkey.as_ptr(), 0, KEY_READ, &mut key) } != ERROR_SUCCESS {
            continue;
        }
        for index in 0.. {
            let mut name = [0u16; 512];
            let mut name_len = name.len() as u32;
            let mut data = [0u16; 1024];
            let mut data_len = (data.len() * 2) as u32;
            let mut kind = 0u32;
            // SAFETY: buffers and their lengths match.
            let rc = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null(),
                    &mut kind,
                    data.as_mut_ptr().cast(),
                    &mut data_len,
                )
            };
            if rc != ERROR_SUCCESS {
                break;
            }
            if kind != REG_SZ {
                continue;
            }
            let value = String::from_utf16_lossy(&name[..name_len as usize]);
            let chars = (data_len as usize / 2).min(data.len());
            let file = String::from_utf16_lossy(&data[..chars]).trim_end_matches('\0').to_owned();
            out.push((value, file));
        }
        // SAFETY: opened above.
        unsafe { RegCloseKey(key) };
    }
    out
}

/// Font ids of one terminal font size.
#[derive(Clone, Debug, PartialEq)]
pub struct TermFonts {
    pub regular: FontId,
    pub bold: FontId,
    pub italic: FontId,
    pub bold_italic: FontId,
    /// Only the primary face: `has_glyph` on it says whether a character can
    /// be drawn inside a grid-aligned run.
    pub primary: FontId,
}

impl TermFonts {
    pub fn new(size: f32) -> TermFonts {
        let id = |name: &str| FontId::new(size, FontFamily::Name(name.into()));
        TermFonts {
            regular: id("term"),
            bold: id("term-bold"),
            italic: id("term-italic"),
            bold_italic: id("term-bold-italic"),
            primary: id("term-primary"),
        }
    }

    pub fn for_style(&self, bold: bool, italic: bool) -> &FontId {
        match (bold, italic) {
            (false, false) => &self.regular,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (true, true) => &self.bold_italic,
        }
    }
}

/// What `install` could not find, for the log.
#[derive(Debug, Default)]
pub struct FontReport {
    pub missing: Vec<String>,
}

/// Installs the terminal family (`term*`) and the interface font, which is the
/// bundled Cascadia Mono — the same "Terminal Native" look as SNATCH/BEAT/
/// STRIKE. Call again after the font family changes in the settings.
///
/// `fallbacks` loads the heavy system fallbacks (`seguisym`, `seguiemj`,
/// `msyh`): they cost ~100 MB of working set, so the app starts without them
/// and re-installs once a character the primary font cannot draw shows up.
pub fn install(ctx: &egui::Context, family: &str, entries: &[(String, String)], fallbacks: bool) -> FontReport {
    let dir = fonts_dir();
    let mut report = FontReport::default();
    let mut defs = FontDefinitions::default();
    let load = |path: &Option<PathBuf>| {
        path.as_ref().and_then(|p| std::fs::read(p).ok().map(|bytes| (bytes, FaceSource::File(p.clone()))))
    };

    let files = match_family(entries, family, &dir);
    let (regular, regular_source) = load(&files.regular).unwrap_or_else(|| {
        report.missing.push(format!("{family} (regular): using Cascadia Mono"));
        (CASCADIA.to_vec(), FaceSource::Bundled)
    });
    let face = |path: &Option<PathBuf>| load(path).unwrap_or_else(|| (regular.clone(), regular_source.clone()));
    let (bold, bold_source) = face(&files.bold);
    let (italic, italic_source) = face(&files.italic);
    let (bold_italic, bold_italic_source) = face(&files.bold_italic);
    let faces = TermFaces { regular: regular_source, bold: bold_source, italic: italic_source, bold_italic: bold_italic_source };
    defs.font_data.insert("term-regular".into(), FontData::from_owned(regular));
    defs.font_data.insert("term-bold".into(), FontData::from_owned(bold));
    defs.font_data.insert("term-italic".into(), FontData::from_owned(italic));
    defs.font_data.insert("term-bold-italic".into(), FontData::from_owned(bold_italic));
    defs.font_data.insert("term-cascadia".into(), FontData::from_static(CASCADIA));

    let mut extra: Vec<String> = Vec::new();
    if fallbacks {
        for (name, file) in [("fallback-symbols", "seguisym.ttf"), ("fallback-emoji", "seguiemj.ttf"), ("fallback-cjk", "msyh.ttc")] {
            match std::fs::read(dir.join(file)) {
                Ok(bytes) => {
                    defs.font_data.insert(name.into(), FontData::from_owned(bytes));
                    extra.push(name.to_owned());
                }
                Err(_) => report.missing.push(file.to_owned()),
            }
        }
    }
    let mut fallbacks_list = extra.clone();
    fallbacks_list.push("term-cascadia".into());

    for (fam, first) in [
        ("term", "term-regular"),
        ("term-bold", "term-bold"),
        ("term-italic", "term-italic"),
        ("term-bold-italic", "term-bold-italic"),
    ] {
        let mut list = vec![first.to_owned()];
        list.extend(fallbacks_list.iter().cloned());
        defs.families.insert(FontFamily::Name(fam.into()), list);
    }
    defs.families.insert(FontFamily::Name("term-primary".into()), vec!["term-regular".into()]);

    // Interface font: bundled Cascadia Mono, with egui's defaults behind it for
    // glyphs Cascadia does not have. Body and field text must share one
    // baseline: rows mix labels with chips, dots and field values, and any
    // nudge on the body face leaves the text hanging below them. Titles sit a
    // hair higher.
    let ui = |name: &str, y_offset: f32| {
        let data = FontData::from_static(CASCADIA).tweak(egui::FontTweak { y_offset_factor: y_offset, ..Default::default() });
        (name.to_owned(), data)
    };
    for (name, data) in [ui("ui", 0.0), ui("ui-tight", 0.0), ui("ui-title", -0.08)] {
        defs.font_data.insert(name.clone(), data);
    }
    let default_proportional = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for name in ["ui", "ui-tight", "ui-title"] {
        let mut list = vec![name.to_owned()];
        list.extend(default_proportional.iter().cloned());
        defs.families.insert(FontFamily::Name(name.into()), list);
    }
    defs.families.entry(FontFamily::Proportional).or_default().insert(0, "ui".into());
    defs.families.entry(FontFamily::Monospace).or_default().insert(0, "ui-tight".into());

    // File-type icons (Seti): private-use codepoints, so a dedicated family.
    defs.font_data.insert("seti".into(), FontData::from_static(SETI));
    defs.families.insert(FontFamily::Name("icons".into()), vec!["seti".to_owned()]);
    ctx.set_fonts(defs);
    crate::term::glyphs::install(ctx, faces);
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<(String, String)> {
        [
            ("Consolas (TrueType)", "consola.ttf"),
            ("Consolas Bold (TrueType)", "consolab.ttf"),
            ("Consolas Italic (TrueType)", "consolai.ttf"),
            ("Consolas Bold Italic (TrueType)", "consolaz.ttf"),
            ("Cascadia Mono Regular (TrueType)", "C:\\Users\\me\\Fonts\\CascadiaMono.ttf"),
            ("Consolas Nerd (TrueType)", "nerd.ttf"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
    }

    /// Every symbol the chrome paints must exist in the interface font chain,
    /// otherwise a button renders as an invisible box (epaint's warning).
    #[test]
    fn ui_glyphs_are_available() {
        let ctx = egui::Context::default();
        install(&ctx, "Consolas", &registry_font_entries(), false);
        // Fonts become available with the first frame.
        let _ = ctx.run(Default::default(), |_| {});
        // Exactly the symbols src/strings.rs and the chrome paint.
        let used = "≡×↑↓▸▾◂⟳‹›·—…−─□»↺▓░";
        let missing: Vec<char> = used
            .chars()
            .filter(|c| !ctx.fonts(|fonts| fonts.has_glyph(&FontId::new(13.0, FontFamily::Name("ui".into())), *c)))
            .collect();
        assert!(missing.is_empty(), "glyphs missing from the ui family: {missing:?}");
    }

    #[test]
    fn finds_all_four_faces() {
        let f = match_family(&entries(), "consolas", Path::new("C:\\Windows\\Fonts"));
        assert_eq!(f.regular, Some(PathBuf::from("C:\\Windows\\Fonts\\consola.ttf")));
        assert_eq!(f.bold, Some(PathBuf::from("C:\\Windows\\Fonts\\consolab.ttf")));
        assert_eq!(f.italic, Some(PathBuf::from("C:\\Windows\\Fonts\\consolai.ttf")));
        assert_eq!(f.bold_italic, Some(PathBuf::from("C:\\Windows\\Fonts\\consolaz.ttf")));
    }

    #[test]
    fn regular_suffix_and_absolute_paths() {
        let f = match_family(&entries(), "Cascadia Mono", Path::new("C:\\Windows\\Fonts"));
        assert_eq!(f.regular, Some(PathBuf::from("C:\\Users\\me\\Fonts\\CascadiaMono.ttf")));
        assert_eq!(f.bold, None);
        assert_eq!(match_family(&entries(), "Nope", Path::new("C:\\")), FontFiles::default());
    }
}
