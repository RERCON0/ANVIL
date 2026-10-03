pub mod event_loop;
pub mod gl_window;
pub mod keys;
pub mod route;

pub use event_loop::run;

use crate::chrome::edges::Edge;

/// Window operations requested by the UI; executed by the host after the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowCommand {
    Drag,
    Resize(Edge),
    Minimize,
    ToggleMaximize,
    ToggleFullscreen,
    Close,
}
