//! One terminal pane: a ConPTY process, alacritty's terminal state and the
//! reader thread that connects them.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Osc52, Term};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{CursorStyle, Rgb};

use crate::term::pty::ScanPty;
use crate::term::style::Palette;

/// Grid size for `Term::new`/`Term::resize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub columns: usize,
    pub lines: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// What the UI needs to hear from a pane (drained once per frame).
#[derive(Clone, Debug, PartialEq)]
pub enum PaneEvent {
    Title(String),
    ResetTitle,
    Bell,
    /// OSC 52 copy request from the application.
    Clipboard(String),
    CursorBlinkingChange,
    /// The process exited; None when the code is unknown.
    Exited(Option<i32>),
}

struct Shared {
    sender: OnceLock<EventLoopSender>,
    size: Mutex<WindowSize>,
    palette: RwLock<Palette>,
    output: AtomicBool,
    exited: AtomicBool,
}

impl Shared {
    fn send(&self, bytes: Vec<u8>) {
        if let Some(sender) = self.sender.get() {
            let _ = sender.send(Msg::Input(bytes.into()));
        }
    }
}

/// Receives alacritty events on the PTY reader thread. Replies to the
/// application (device attributes, cursor position, colours) go straight back
/// to the PTY; everything else is queued for the UI and wakes it up.
#[derive(Clone)]
pub struct Listener {
    tx: Sender<PaneEvent>,
    shared: Arc<Shared>,
    repaint: Arc<dyn Fn() + Send + Sync>,
}

fn palette_rgb(palette: &Palette, index: usize) -> Rgb {
    let c = match index {
        0..=255 => palette.indexed(index as u8),
        257 => palette.background,
        258 => palette.cursor,
        _ => palette.foreground,
    };
    Rgb { r: c.r(), g: c.g(), b: c.b() }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => self.shared.send(text.into_bytes()),
            Event::ColorRequest(index, format) => {
                let rgb = palette_rgb(&self.shared.palette.read().unwrap_or_else(|e| e.into_inner()), index);
                self.shared.send(format(rgb).into_bytes());
            }
            Event::TextAreaSizeRequest(format) => {
                let size = *self.shared.size.lock().unwrap_or_else(|e| e.into_inner());
                self.shared.send(format(size).into_bytes());
            }
            Event::Wakeup => {
                self.shared.output.store(true, Ordering::Relaxed);
                (self.repaint)();
            }
            Event::Title(title) => self.queue(PaneEvent::Title(title)),
            Event::ResetTitle => self.queue(PaneEvent::ResetTitle),
            Event::Bell => self.queue(PaneEvent::Bell),
            Event::ClipboardStore(_, text) => self.queue(PaneEvent::Clipboard(text)),
            Event::CursorBlinkingChange => self.queue(PaneEvent::CursorBlinkingChange),
            Event::ChildExit(status) => {
                if !self.shared.exited.swap(true, Ordering::Relaxed) {
                    self.queue(PaneEvent::Exited(status.code()));
                }
            }
            Event::Exit => {
                if !self.shared.exited.swap(true, Ordering::Relaxed) {
                    self.queue(PaneEvent::Exited(None));
                }
            }
            Event::MouseCursorDirty | Event::ClipboardLoad(..) => {}
        }
    }
}

impl Listener {
    fn queue(&self, event: PaneEvent) {
        let _ = self.tx.send(event);
        (self.repaint)();
    }
}

pub struct SpawnOptions {
    pub pane_id: u64,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub columns: u16,
    pub lines: u16,
    pub cell_width: u16,
    pub cell_height: u16,
    pub scrollback: usize,
    pub word_separators: String,
    pub palette: Palette,
    /// Default cursor shape from the config; applications may override it.
    pub cursor_style: CursorStyle,
}

pub struct Pane {
    pub id: u64,
    pub term: Arc<FairMutex<Term<Listener>>>,
    /// Last directory the shell reported (OSC 7 / 9;9 / 1337).
    pub cwd: Arc<Mutex<Option<PathBuf>>>,
    pub shell_pid: u32,
    notifier: Notifier,
    events: Receiver<PaneEvent>,
    shared: Arc<Shared>,
    size: WindowSize,
    /// The full `Term` config, kept so settings can update it at runtime.
    config: Mutex<Config>,
}

impl Pane {
    /// Starts the process. `repaint` is called from the reader thread whenever
    /// the UI should draw again (it must be cheap and thread-safe).
    pub fn spawn(opts: SpawnOptions, repaint: Arc<dyn Fn() + Send + Sync>) -> io::Result<Pane> {
        let size = WindowSize {
            num_lines: opts.lines.max(1),
            num_cols: opts.columns.max(2),
            cell_width: opts.cell_width.max(1),
            cell_height: opts.cell_height.max(1),
        };
        let options = tty::Options {
            shell: Some(tty::Shell::new(opts.program, opts.args)),
            working_directory: opts.cwd,
            drain_on_exit: true,
            env: opts.env,
            escape_args: true,
        };
        let pty = tty::new(&options, size, opts.pane_id)?;
        let shell_pid = pty.child_watcher().pid().map(|p| p.get()).unwrap_or(0);
        let cwd = Arc::new(Mutex::new(None));
        let pty = ScanPty::new(pty, cwd.clone());

        let (tx, events) = mpsc::channel();
        let shared = Arc::new(Shared {
            sender: OnceLock::new(),
            size: Mutex::new(size),
            palette: RwLock::new(opts.palette),
            output: AtomicBool::new(false),
            exited: AtomicBool::new(false),
        });
        let listener = Listener { tx, shared: shared.clone(), repaint };
        let config = Config {
            scrolling_history: opts.scrollback,
            semantic_escape_chars: opts.word_separators,
            kitty_keyboard: false,
            osc52: Osc52::OnlyCopy,
            default_cursor_style: opts.cursor_style,
            ..Config::default()
        };
        let grid = GridSize { columns: size.num_cols as usize, lines: size.num_lines as usize };
        let term = Arc::new(FairMutex::new(Term::new(config.clone(), &grid, listener.clone())));
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let sender = event_loop.channel();
        let _ = shared.sender.set(sender.clone());
        // The thread ends by itself after Msg::Shutdown or when the process exits.
        let _ = event_loop.spawn();

        Ok(Pane { id: opts.pane_id, term, cwd, shell_pid, notifier: Notifier(sender), events, shared, size, config: Mutex::new(config) })
    }

    pub fn write(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.notifier.notify(bytes);
        }
    }

    pub fn grid_size(&self) -> (u16, u16) {
        (self.size.num_cols, self.size.num_lines)
    }

    /// Resizes the terminal and the ConPTY when the cell grid changes.
    pub fn resize(&mut self, columns: u16, lines: u16, cell_width: u16, cell_height: u16) {
        let size = WindowSize {
            num_lines: lines.max(1),
            num_cols: columns.max(2),
            cell_width: cell_width.max(1),
            cell_height: cell_height.max(1),
        };
        let same = |a: WindowSize, b: WindowSize| {
            (a.num_lines, a.num_cols, a.cell_width, a.cell_height) == (b.num_lines, b.num_cols, b.cell_width, b.cell_height)
        };
        if same(size, self.size) {
            return;
        }
        self.size = size;
        *self.shared.size.lock().unwrap_or_else(|e| e.into_inner()) = size;
        self.term.lock().resize(GridSize { columns: size.num_cols as usize, lines: size.num_lines as usize });
        self.notifier.on_resize(size);
    }

    pub fn set_palette(&self, palette: Palette) {
        *self.shared.palette.write().unwrap_or_else(|e| e.into_inner()) = palette;
    }

    /// Changes the cursor shape new applications see; a running application's
    /// own DECSCUSR request still wins until it resets to the default.
    pub fn set_cursor_style(&self, style: CursorStyle) {
        let mut config = self.config.lock().unwrap_or_else(|e| e.into_inner());
        if config.default_cursor_style == style {
            return;
        }
        config.default_cursor_style = style;
        self.term.lock().set_options(config.clone());
    }

    pub fn drain_events(&self) -> Vec<PaneEvent> {
        self.events.try_iter().collect()
    }

    /// True once since the last call if the process printed anything.
    pub fn take_output_flag(&self) -> bool {
        self.shared.output.swap(false, Ordering::Relaxed)
    }

    pub fn current_dir(&self) -> Option<PathBuf> {
        self.cwd.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// True when the ConPTY from vendor/conpty (not the system one) is loaded.
/// Meaningful after the first pane was spawned.
pub fn bundled_conpty_loaded() -> bool {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    let name: Vec<u16> = "conpty.dll".encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: NUL-terminated UTF-16 name; the handle is only compared to null.
    unsafe { !GetModuleHandleW(name.as_ptr()).is_null() }
}

impl Drop for Pane {
    fn drop(&mut self) {
        // Closing the pseudoconsole ends the console processes in this pane.
        // The reader thread drops the PTY itself; never join it from the UI.
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}
