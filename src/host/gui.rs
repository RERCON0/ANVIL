//! Fallible egui/glow host. Adapted from egui_glow's winit integration
//! (MIT OR Apache-2.0): the upstream constructor unwraps shader/renderer errors.
use egui_glow::egui_winit;
use std::sync::Arc;
use winit::{event::WindowEvent, event_loop::ActiveEventLoop, window::Window};

pub(super) struct Gui {
    pub egui_ctx: egui::Context,
    pub egui_winit: egui_winit::State,
    pub painter: egui_glow::Painter,
    viewport_info: egui::ViewportInfo,
    shapes: Vec<egui::epaint::ClippedShape>,
    pixels_per_point: f32,
    textures_delta: egui::TexturesDelta,
}

impl Gui {
    pub fn new(event_loop: &ActiveEventLoop, gl: Arc<glow::Context>, scale: f32) -> Result<Self, String> {
        let painter = egui_glow::Painter::new(gl, "", None, true).map_err(|e| format!("OpenGL renderer: {e}"))?;
        let egui_ctx = egui::Context::default();
        let egui_winit = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            event_loop,
            Some(scale),
            event_loop.system_theme(),
            Some(painter.max_texture_side()),
        );
        Ok(Self {
            egui_ctx,
            egui_winit,
            painter,
            viewport_info: Default::default(),
            shapes: Default::default(),
            pixels_per_point: scale,
            textures_delta: Default::default(),
        })
    }

    pub fn on_window_event(&mut self, window: &Window, event: &WindowEvent) -> egui_winit::EventResponse {
        self.egui_winit.on_window_event(window, event)
    }

    pub fn run(&mut self, window: &Window, run_ui: impl FnMut(&mut egui::Ui)) {
        let input = self.egui_winit.take_egui_input(window);
        let output = self.egui_ctx.run_ui(input, run_ui);
        for (_, viewport) in output.viewport_output {
            let mut actions = Default::default();
            egui_winit::process_viewport_commands(
                &self.egui_ctx,
                &mut self.viewport_info,
                viewport.commands,
                window,
                &mut actions,
            );
            for action in actions {
                log::warn!("Unsupported viewport action: {action:?}");
            }
        }
        self.egui_winit.handle_platform_output(window, output.platform_output);
        self.shapes = output.shapes;
        self.pixels_per_point = output.pixels_per_point;
        self.textures_delta.append(output.textures_delta);
    }

    pub fn paint(&mut self, window: &Window) {
        let delta = std::mem::take(&mut self.textures_delta);
        for (id, image) in delta.set {
            self.painter.set_texture(id, &image);
        }
        let primitives = self.egui_ctx.tessellate(std::mem::take(&mut self.shapes), self.pixels_per_point);
        self.painter.paint_primitives(window.inner_size().into(), self.pixels_per_point, &primitives);
        for id in delta.free {
            self.painter.free_texture(id);
        }
    }

    pub fn destroy(&mut self) {
        self.painter.destroy();
    }
}

pub(super) fn initialization_error(reason: &str) -> String {
    let explanation = crate::strings::pick(
        "ANVIL could not initialize its graphics. OpenGL 2.1 or a compatible OpenGL ES renderer is required. Update the graphics driver; in a VM or Remote Desktop session, enable a supported graphics adapter. Settings and saved layouts have not been overwritten.",
        "ANVIL не смог инициализировать графику. Нужен OpenGL 2.1 или совместимый рендерер OpenGL ES. Обновите видеодрайвер; в виртуальной машине или удалённом рабочем столе включите поддерживаемый графический адаптер. Настройки и сохранённые раскладки не перезаписаны.",
    );
    format!("{explanation}\n\n{reason}")
}

pub(super) fn show_initialization_error(reason: &str) {
    log::error!("Graphics initialization failed: {reason}");
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let message: Vec<u16> = initialization_error(reason).encode_utf16().chain([0]).collect();
    let title: Vec<u16> = "ANVIL".encode_utf16().chain([0]).collect();
    // SAFETY: both UTF-16 buffers are NUL-terminated and live until the modal call returns.
    unsafe {
        MessageBoxW(std::ptr::null_mut(), message.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

#[cfg(test)]
#[test]
fn driver_failure_explains_requirements_and_preserves_the_cause() {
    let message = initialization_error("No compatible OpenGL configuration");
    assert!(message.contains("OpenGL 2.1"));
    assert!(message.ends_with("No compatible OpenGL configuration"));
}
