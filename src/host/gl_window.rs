//! The winit window with its OpenGL context (after egui_glow's pure_glow example).

use std::num::NonZeroU32;

use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::HasWindowHandle;
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
    pub unsafe fn new(event_loop: &ActiveEventLoop, attributes: WindowAttributes) -> GlWindow {
        use glutin::context::NotCurrentGlContext;
        use glutin::display::{GetGlDisplay, GlDisplay};
        use glutin::prelude::GlSurface;

        let template = glutin::config::ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false);
        let (mut window, gl_config) = glutin_winit::DisplayBuilder::new()
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(attributes.clone()))
            .build(event_loop, template, |mut configs| configs.next().expect("no OpenGL config"))
            .expect("failed to create an OpenGL config");
        let gl_display = gl_config.display();
        let raw = window.as_ref().map(|w| w.window_handle().expect("window handle").as_raw());
        let attrs = glutin::context::ContextAttributesBuilder::new().build(raw);
        let gles = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::Gles(None))
            .build(raw);
        // SAFETY: the display and config come from the same DisplayBuilder.
        let not_current = unsafe {
            gl_display
                .create_context(&gl_config, &attrs)
                .unwrap_or_else(|_| gl_display.create_context(&gl_config, &gles).expect("OpenGL context"))
        };
        let window = window.take().unwrap_or_else(|| {
            glutin_winit::finalize_window(event_loop, attributes, &gl_config).expect("failed to create the window")
        });
        let (w, h): (u32, u32) = window.inner_size().into();
        let surface_attributes = glutin::surface::SurfaceAttributesBuilder::<glutin::surface::WindowSurface>::new()
            .build(
                window.window_handle().expect("window handle").as_raw(),
                NonZeroU32::new(w).unwrap_or(NonZeroU32::MIN),
                NonZeroU32::new(h).unwrap_or(NonZeroU32::MIN),
            );
        // SAFETY: the raw handle belongs to `window`, which outlives the surface.
        let gl_surface = unsafe { gl_display.create_window_surface(&gl_config, &surface_attributes).expect("GL surface") };
        let gl_context = not_current.make_current(&gl_surface).expect("make GL context current");
        let _ = gl_surface.set_swap_interval(&gl_context, glutin::surface::SwapInterval::Wait(NonZeroU32::MIN));
        GlWindow { window, gl_context, gl_display, gl_surface }
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
