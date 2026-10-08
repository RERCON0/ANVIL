//! Bounded elision cache. Keep exact source text and invalidate on font/atlas resets.
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use egui::{Color32, FontId, Galley, Painter};

const MAX_ENTRIES: usize = 256;
const MAX_SOURCE_BYTES: usize = 16 * 1024;

#[derive(Hash, PartialEq, Eq)]
struct Key {
    text: u64,
    family: egui::FontFamily,
    font_size: u32,
    width: u32,
    color: Color32,
    front: bool,
}

struct Entry {
    source: String,
    galley: Arc<Galley>,
}

#[derive(Default)]
struct Cache {
    generation: Option<Arc<Galley>>,
    scale: u32,
    entries: HashMap<Key, Entry>,
}

pub(crate) fn elide_galley(
    painter: &Painter,
    text: &str,
    font: FontId,
    width: f32,
    color: Color32,
    front: bool,
) -> Arc<Galley> {
    let ctx = painter.ctx();
    // Retaining the empty galley prevents pointer reuse disguising an atlas reset.
    let generation = ctx.fonts_mut(|fonts| fonts.layout_job(egui::text::LayoutJob::default()));
    let scale = ctx.pixels_per_point().to_bits();
    let cache = ctx.data_mut(|data| {
        let id = egui::Id::new("anvil-elision-cache");
        data.get_temp::<Arc<Mutex<Cache>>>(id).unwrap_or_else(|| {
            let cache = Arc::new(Mutex::new(Cache::default()));
            data.insert_temp(id, cache.clone());
            cache
        })
    });
    let mut cache = cache.lock().unwrap_or_else(|error| error.into_inner());
    if cache.scale != scale || cache.generation.as_ref().is_none_or(|old| !Arc::ptr_eq(old, &generation)) {
        cache.entries.clear();
        cache.generation = Some(generation);
        cache.scale = scale;
    }
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    let key = Key {
        text: hasher.finish(),
        family: font.family.clone(),
        font_size: font.size.to_bits(),
        width: width.to_bits(),
        color,
        front,
    };
    if let Some(entry) = cache.entries.get(&key).filter(|entry| entry.source == text) {
        return entry.galley.clone();
    }
    let galley = layout_elided(painter, text, font, width, color, front);
    if text.len() <= MAX_SOURCE_BYTES {
        if cache.entries.len() >= MAX_ENTRIES {
            cache.entries.clear();
        }
        cache.entries.insert(key, Entry { source: text.to_owned(), galley: galley.clone() });
    }
    galley
}

fn layout_elided(painter: &Painter, text: &str, font: FontId, width: f32, color: Color32, front: bool) -> Arc<Galley> {
    let layout = |text: String| painter.layout_no_wrap(text, font.clone(), color);
    let full = layout(text.to_owned());
    if full.size().x <= width {
        return full;
    }
    let mut boundaries: Vec<usize> = text.char_indices().map(|(offset, _)| offset).collect();
    boundaries.push(text.len());
    let chars = boundaries.len() - 1;
    let mut scratch = String::with_capacity(text.len() + 3);
    let mut candidate = |count: usize| {
        scratch.clear();
        if front {
            scratch.push('…');
            scratch.push_str(&text[boundaries[chars - count]..]);
        } else {
            scratch.push_str(&text[..boundaries[count]]);
            scratch.push('…');
        }
        layout(scratch.clone())
    };
    let mut low = 0;
    let mut high = chars;
    while low < high {
        let mid = low + (high - low) / 2;
        if candidate(mid).size().x <= width {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    candidate(low.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_elision_tracks_content_width_font_color_and_dpi() {
        let ctx = egui::Context::default();
        let mut previous = None;
        for scale in [1.0, 1.0, 2.0] {
            ctx.set_pixels_per_point(scale);
            let _ = ctx.run_ui(Default::default(), |ui| {
                let painter = ui.painter();
                let font = FontId::proportional(14.0);
                let first =
                    elide_galley(painter, "a long Unicode title 中Ж", font.clone(), 80.0, Color32::WHITE, false);
                let again =
                    elide_galley(painter, "a long Unicode title 中Ж", font.clone(), 80.0, Color32::WHITE, false);
                assert!(Arc::ptr_eq(&first, &again));
                assert!(first.size().x <= 80.0);
                let front = elide_galley(painter, "a long Unicode title 中Ж", font.clone(), 80.0, Color32::WHITE, true);
                assert!(front.text().starts_with('…') && front.size().x <= 80.0);
                let changed = elide_galley(painter, "other", font.clone(), 80.0, Color32::WHITE, false);
                assert_eq!(changed.text(), "other");
                let wider =
                    elide_galley(painter, "a long Unicode title 中Ж", font.clone(), 800.0, Color32::WHITE, false);
                assert_eq!(wider.text(), "a long Unicode title 中Ж");
                let colored = elide_galley(painter, "a long Unicode title 中Ж", font, 80.0, Color32::RED, false);
                assert!(!Arc::ptr_eq(&first, &colored));
                if scale == 2.0 {
                    assert!(!Arc::ptr_eq(previous.as_ref().unwrap(), &first), "DPI invalidates atlas UVs");
                }
                previous = Some(first);
            });
        }
    }

    #[test]
    fn font_reset_invalidates_cached_atlas_coordinates_and_entries_stay_bounded() {
        let ctx = egui::Context::default();
        let mut before = None;
        let _ = ctx.run_ui(Default::default(), |ui| {
            before = Some(elide_galley(ui.painter(), "title", FontId::proportional(14.0), 80.0, Color32::WHITE, false));
        });
        let mut fonts = egui::FontDefinitions::default();
        let fallback = fonts.families[&egui::FontFamily::Monospace].clone();
        fonts.families.insert(egui::FontFamily::Name("reset-fixture".into()), fallback);
        ctx.set_fonts(fonts);
        let _ = ctx.run_ui(Default::default(), |ui| {
            let after = elide_galley(ui.painter(), "title", FontId::proportional(14.0), 80.0, Color32::WHITE, false);
            assert!(!Arc::ptr_eq(before.as_ref().unwrap(), &after), "old atlas UVs cannot survive set_fonts");
            for index in 0..MAX_ENTRIES * 2 {
                elide_galley(
                    ui.painter(),
                    &format!("entry {index}"),
                    FontId::proportional(14.0),
                    80.0,
                    Color32::WHITE,
                    false,
                );
            }
            let cache =
                ctx.data_mut(|data| data.get_temp::<Arc<Mutex<Cache>>>(egui::Id::new("anvil-elision-cache")).unwrap());
            let count = cache.lock().unwrap().entries.len();
            elide_galley(
                ui.painter(),
                &"x".repeat(MAX_SOURCE_BYTES + 1),
                FontId::proportional(14.0),
                80.0,
                Color32::WHITE,
                false,
            );
            assert_eq!(cache.lock().unwrap().entries.len(), count, "large sources must not be retained");
            assert!(count <= MAX_ENTRIES);
        });
    }
}
