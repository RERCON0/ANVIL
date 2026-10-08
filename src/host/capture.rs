//! Actual framebuffer recording for README examples; never compiled into releases.
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::app::{AnvilApp, ReadmeDemo};

pub(super) struct Capture {
    output: PathBuf,
    writer: super::event_loop::CaptureWriter,
    animation: bool,
    next: Instant,
    frame: usize,
    recording: bool,
    scene: Option<ReadmeDemo>,
    size: Option<winit::dpi::PhysicalSize<u32>>,
}

impl Capture {
    pub fn still(output: PathBuf, writer: super::event_loop::CaptureWriter) -> Self {
        Self {
            output,
            writer,
            animation: false,
            next: Instant::now() + Duration::from_secs(30),
            frame: 0,
            recording: false,
            scene: None,
            size: None,
        }
    }

    pub fn demo(directory: PathBuf, writer: super::event_loop::CaptureWriter) -> Self {
        // Refuse to overwrite an existing sequence or unrelated directory.
        std::fs::create_dir(&directory).expect("use a new demo output directory");
        Self { animation: true, ..Self::still(directory, writer) }
    }

    pub fn before_frame(&mut self, app: &mut AnvilApp, input: &mut egui::RawInput, ctx: &egui::Context) {
        self.recording = Instant::now() >= self.next;
        if self.animation && (self.recording || self.scene.is_some()) {
            let scene = self.scene.get_or_insert_with(|| ReadmeDemo::prepare(app));
            scene.step(app, input, ctx, self.frame);
        }
    }

    pub fn cursor(&self) -> Option<egui::Pos2> {
        self.scene.as_ref().and_then(|scene| scene.cursor)
    }

    /// The OS pointer is not in the framebuffer; draw the demo's actual input position.
    pub fn paint_cursor(ctx: &egui::Context, pos: egui::Pos2) {
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("demo-pointer")));
        let points = vec![pos, pos + egui::vec2(0.0, 20.0), pos + egui::vec2(5.0, 15.0), pos + egui::vec2(13.0, 14.0)];
        painter.add(egui::Shape::convex_polygon(
            points,
            egui::Color32::WHITE,
            egui::Stroke::new(1.0_f32, egui::Color32::BLACK),
        ));
    }

    pub fn after_frame(&mut self, app: &AnvilApp, gl: &glow::Context, size: winit::dpi::PhysicalSize<u32>) -> bool {
        if !self.recording {
            return false;
        }
        if let Some(previous) = self.size {
            assert_eq!(size, previous, "do not resize the window during demo recording");
        }
        self.size = Some(size);
        let mut pixels = vec![0; size.width as usize * size.height as usize * 4];
        // SAFETY: the current GL context and RGBA allocation match the viewport.
        unsafe {
            use glow::HasContext;
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(
                0,
                0,
                size.width as i32,
                size.height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
        }
        let path =
            if self.animation { self.output.join(format!("{:04}.png", self.frame)) } else { self.output.clone() };
        (self.writer)(&path, size.width, size.height, pixels).expect("save captured frame");
        self.frame += 1;
        self.next = Instant::now() + Duration::from_millis(80);
        if !self.animation || self.frame == 125 {
            if let Some(scene) = &self.scene {
                scene.verify(app);
            }
            println!("Captured {} actual framebuffer frame(s) to {}", self.frame, self.output.display());
            return true;
        }
        false
    }
}
