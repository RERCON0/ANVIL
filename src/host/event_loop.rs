//! winit event loop: owns the window, feeds egui, and routes keyboard input
//! to hotkeys, the focused terminal or egui (see route.rs).

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Fullscreen, ResizeDirection, WindowId};

use crate::app::AnvilApp;
use crate::chrome::edges::Edge;
use crate::host::gl_window::GlWindow;
use crate::host::keys;
use crate::host::route::{route_key_press, KeyRoute};
use crate::host::WindowCommand;
use crate::theme;

#[derive(Debug)]
pub enum UserEvent {
    Repaint(Duration),
}

struct Host {
    proxy: EventLoopProxy<UserEvent>,
    gl: Option<GlWindow>,
    egui: Option<egui_glow::EguiGlow>,
    app: AnvilApp,
    modifiers: ModifiersState,
    repaint_at: Option<Instant>,
}

pub fn run(app: AnvilApp) {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().expect("event loop");
    let proxy = event_loop.create_proxy();
    let mut host = Host { proxy, gl: None, egui: None, app, modifiers: ModifiersState::empty(), repaint_at: None };
    if let Err(e) = event_loop.run_app(&mut host) {
        log::error!("event loop failed: {e}");
    }
}

fn resize_direction(edge: Edge) -> ResizeDirection {
    match edge {
        Edge::North => ResizeDirection::North,
        Edge::South => ResizeDirection::South,
        Edge::East => ResizeDirection::East,
        Edge::West => ResizeDirection::West,
        Edge::NorthEast => ResizeDirection::NorthEast,
        Edge::NorthWest => ResizeDirection::NorthWest,
        Edge::SouthEast => ResizeDirection::SouthEast,
        Edge::SouthWest => ResizeDirection::SouthWest,
    }
}

/// Where a drag-and-drop was released, in egui points. Windows sends the
/// window no mouse moves during an OLE drag, so the pointer position egui last
/// saw is where the cursor entered the window, not where the file landed.
#[cfg(windows)]
fn drop_point(window: &winit::window::Window, pixels_per_point: f32) -> Option<egui::Pos2> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut cursor = POINT { x: 0, y: 0 };
    // SAFETY: GetCursorPos only writes the POINT it is given.
    if unsafe { GetCursorPos(&mut cursor) } == 0 {
        return None;
    }
    let origin = window.inner_position().ok()?;
    Some(egui::Pos2::new(
        (cursor.x - origin.x) as f32 / pixels_per_point,
        (cursor.y - origin.y) as f32 / pixels_per_point,
    ))
}

#[cfg(not(windows))]
fn drop_point(_window: &winit::window::Window, _pixels_per_point: f32) -> Option<egui::Pos2> {
    None
}

impl Host {
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let Host { gl, egui, app, .. } = self;
        let (Some(gl), Some(egui)) = (gl.as_ref(), egui.as_mut()) else { return };
        let maximized = gl.window.is_maximized();
        let mut commands = Vec::new();
        egui.run(&gl.window, |ctx| commands = app.frame(ctx, maximized));
        if let Some(area) = app.ime_area() {
            gl.window.set_ime_cursor_area(
                winit::dpi::LogicalPosition::new(area.min.x, area.min.y),
                winit::dpi::LogicalSize::new(area.width(), area.height()),
            );
        }
        // SAFETY: the GL context is current on this thread.
        unsafe {
            use glow::HasContext as _;
            let [r, g, b, _] = theme::colors().chrome_bg.to_normalized_gamma_f32();
            egui.painter.gl().clear_color(r, g, b, 1.0);
            egui.painter.gl().clear(glow::COLOR_BUFFER_BIT);
        }
        egui.paint(&gl.window);
        gl.swap_buffers();
        gl.window.set_visible(true);
        self.execute(event_loop, commands);
    }

    fn execute(&mut self, event_loop: &ActiveEventLoop, commands: Vec<WindowCommand>) {
        let Some(gl) = self.gl.as_ref() else { return };
        let w = &gl.window;
        for command in commands {
            match command {
                WindowCommand::Drag => {
                    let _ = w.drag_window();
                }
                WindowCommand::Resize(edge) => {
                    let _ = w.drag_resize_window(resize_direction(edge));
                }
                WindowCommand::Minimize => w.set_minimized(true),
                WindowCommand::ToggleMaximize => w.set_maximized(!w.is_maximized()),
                WindowCommand::ToggleFullscreen => {
                    w.set_fullscreen(if w.fullscreen().is_some() { None } else { Some(Fullscreen::Borderless(None)) })
                }
                WindowCommand::Close => {
                    self.app.on_exit(Some(w));
                    event_loop.exit();
                    return;
                }
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gl.is_some() {
            return;
        }
        // SAFETY: called on the event-loop thread.
        let gl = unsafe { GlWindow::new(event_loop, self.app.window_attributes()) };
        let glow = Arc::new(gl.glow_context());
        let egui = egui_glow::EguiGlow::new(event_loop, glow, None, Some(gl.window.scale_factor() as f32), true);
        let proxy = egui::mutex::Mutex::new(self.proxy.clone());
        egui.egui_ctx.set_request_repaint_callback(move |info| {
            let _ = proxy.lock().send_event(UserEvent::Repaint(info.delay));
        });
        egui.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        // Fonts first: the style below references the "ui" families they install.
        self.app.on_start(&egui.egui_ctx);
        theme::apply(&egui.egui_ctx);
        gl.window.set_ime_allowed(true);
        self.gl = Some(gl);
        self.egui = Some(egui);
        // A hidden window never gets RedrawRequested on Windows: paint the
        // first frame now; redraw() shows the window after painting it.
        self.redraw(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if self.gl.is_none() || self.egui.is_none() {
            return;
        }
        match &event {
            WindowEvent::CloseRequested => {
                self.app.on_exit(self.gl.as_ref().map(|g| &g.window));
                event_loop.exit();
                return;
            }
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
                return;
            }
            WindowEvent::Resized(size) => {
                let gl = self.gl.as_ref().expect("window");
                gl.resize(*size);
                self.app.window_geometry(*size, gl.window.outer_position().ok(), gl.window.is_maximized());
            }
            WindowEvent::Moved(position) => {
                let gl = self.gl.as_ref().expect("window");
                self.app.window_geometry(gl.window.inner_size(), Some(*position), gl.window.is_maximized());
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::Focused(focused) => self.app.window_focus_changed(*focused),
            WindowEvent::KeyboardInput { event: key, .. } if key.state == ElementState::Pressed => {
                let ctx = self.egui.as_ref().expect("egui").egui_ctx.clone();
                let focus = self.app.key_focus(&ctx);
                let binding = keys::chord(key, self.modifiers).and_then(|c| self.app.keymap().lookup(&c).cloned());
                match route_key_press(binding.as_ref(), focus) {
                    KeyRoute::AppAction | KeyRoute::TerminalAction => {
                        let action = binding.expect("routed actions have a binding");
                        let commands = self.app.run_action(&action, &ctx);
                        self.execute(event_loop, commands);
                        ctx.request_repaint();
                        return;
                    }
                    KeyRoute::Terminal => {
                        self.app.send_key(&keys::key_press(key, self.modifiers));
                        ctx.request_repaint();
                        return;
                    }
                    KeyRoute::Egui => {}
                }
            }
            WindowEvent::DroppedFile(path) => {
                let ctx = self.egui.as_ref().expect("egui").egui_ctx.clone();
                let at = drop_point(&self.gl.as_ref().expect("window").window, ctx.pixels_per_point());
                self.app.drop_path(path, at);
                ctx.request_repaint();
                return;
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                let ctx = self.egui.as_ref().expect("egui").egui_ctx.clone();
                let focus = self.app.key_focus(&ctx);
                if focus.terminal_focused && !focus.egui_wants_keyboard {
                    self.app.send_text(text);
                    ctx.request_repaint();
                    return;
                }
            }
            _ => {}
        }
        let gl = self.gl.as_ref().expect("window");
        let response = self.egui.as_mut().expect("egui").on_window_event(&gl.window, &event);
        if response.repaint {
            gl.window.request_redraw();
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::Repaint(delay) = event;
        if delay.is_zero() {
            if let Some(gl) = &self.gl {
                gl.window.request_redraw();
            }
        } else if let Some(at) = Instant::now().checked_add(delay) {
            self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
        }
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            self.repaint_at = None;
            if let Some(gl) = &self.gl {
                gl.window.request_redraw();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(match self.repaint_at {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(egui) = self.egui.as_mut() {
            egui.destroy();
        }
    }
}
