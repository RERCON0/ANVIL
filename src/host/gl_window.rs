//! The winit window with its OpenGL context (after egui_glow's pure_glow example).

use std::num::NonZeroU32;

use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::{Window, WindowAttributes};

pub struct GlWindow {
    pub window: Window,
    gl_context: glutin::context::PossiblyCurrentContext,
    gl_display: glutin::display::Display,
    gl_surface: glutin::surface::Surface<glutin::surface::WindowSurface>,
}

impl GlWindow {
    /// # Safety
    /// Must be called on the event-loop thread; the GL context is made current here.
    pub unsafe fn new(event_loop: &ActiveEventLoop, attributes: WindowAttributes) -> Result<GlWindow, String> {
        use glutin::context::NotCurrentGlContext;
        use glutin::display::{Display, DisplayApiPreference, GlDisplay};
        use glutin::prelude::GlSurface;

        // Create the window first for WGL extension discovery. Selecting the
        // config directly lets an empty iterator return an error, rather than
        // panicking inside DisplayBuilder's infallible picker callback.
        let window = event_loop.create_window(attributes).map_err(|e| format!("Window: {e}"))?;
        let raw = window.window_handle().map_err(|e| format!("Window handle: {e}"))?.as_raw();
        let display_handle = event_loop.display_handle().map_err(|e| format!("Display handle: {e}"))?.as_raw();
        let gl_display = unsafe { Display::new(display_handle, DisplayApiPreference::WglThenEgl(Some(raw))) }
            .map_err(|e| format!("Graphics display: {e}"))?;
        let template = glutin::config::ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false)
            .compatible_with_native_window(raw)
            .build();
        let gl_config = unsafe { gl_display.find_configs(template) }
            .map_err(|e| format!("OpenGL configuration: {e}"))?
            .next()
            .ok_or_else(|| "No compatible OpenGL configuration".to_owned())?;
        let attrs = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::OpenGl(Some(glutin::context::Version::new(2, 1))))
            .build(Some(raw));
        let gles = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::Gles(None))
            .build(Some(raw));
        // SAFETY: the config was obtained from this display and the window is alive.
        let not_current = unsafe {
            gl_display.create_context(&gl_config, &attrs).or_else(|_| gl_display.create_context(&gl_config, &gles))
        }
        .map_err(|e| format!("OpenGL context: {e}"))?;
        let (w, h): (u32, u32) = window.inner_size().into();
        let surface_attributes = glutin::surface::SurfaceAttributesBuilder::<glutin::surface::WindowSurface>::new()
            .build(raw, NonZeroU32::new(w).unwrap_or(NonZeroU32::MIN), NonZeroU32::new(h).unwrap_or(NonZeroU32::MIN));
        // SAFETY: the raw handle belongs to `window`, which outlives the surface.
        let gl_surface = unsafe { gl_display.create_window_surface(&gl_config, &surface_attributes) }
            .map_err(|e| format!("OpenGL surface: {e}"))?;
        let gl_context = not_current.make_current(&gl_surface).map_err(|e| format!("OpenGL activation: {e}"))?;
        let _ = gl_surface.set_swap_interval(&gl_context, glutin::surface::SwapInterval::Wait(NonZeroU32::MIN));
        Ok(GlWindow { window, gl_context, gl_display, gl_surface })
    }

    pub fn resize(&self, size: winit::dpi::PhysicalSize<u32>) {
        use glutin::surface::GlSurface;
        if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            self.gl_surface.resize(&self.gl_context, w, h);
        }
    }

    pub fn swap_buffers(&self) {
        use glutin::surface::GlSurface;
        if let Err(e) = self.gl_surface.swap_buffers(&self.gl_context) {
            log::warn!("swap_buffers failed: {e}");
        }
    }

    pub fn glow_context(&self) -> glow::Context {
        use glutin::display::GlDisplay;
        // SAFETY: the context is current on this thread.
        unsafe {
            glow::Context::from_loader_function(|s| {
                let s = std::ffi::CString::new(s).expect("GL symbol name");
                self.gl_display.get_proc_address(&s)
            })
        }
    }
}
