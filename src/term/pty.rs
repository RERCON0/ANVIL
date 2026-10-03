//! The PTY handed to alacritty's event loop: the real ConPTY plus a tap on
//! its output that picks up working-directory reports (see osc_scan.rs).

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{self, ChildEvent, EventedPty, EventedReadWrite};

use crate::term::osc_scan::OscScanner;

pub struct ScanningReader {
    pty: tty::Pty,
    scanner: OscScanner,
    cwd: Arc<Mutex<Option<PathBuf>>>,
}

impl Read for ScanningReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.pty.reader().read(buf)?;
        if let Some(dir) = self.scanner.feed(&buf[..n]) {
            *self.cwd.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir);
        }
        Ok(n)
    }
}

pub struct ScanPty {
    reader: ScanningReader,
}

impl ScanPty {
    pub fn new(pty: tty::Pty, cwd: Arc<Mutex<Option<PathBuf>>>) -> ScanPty {
        ScanPty { reader: ScanningReader { pty, scanner: OscScanner::new(), cwd } }
    }
}

impl EventedReadWrite for ScanPty {
    type Reader = ScanningReader;
    type Writer = <tty::Pty as EventedReadWrite>::Writer;

    unsafe fn register(
        &mut self,
        poll: &Arc<polling::Poller>,
        interest: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        // SAFETY: forwarded unchanged; the caller upholds the same contract.
        unsafe { self.reader.pty.register(poll, interest, mode) }
    }

    fn reregister(
        &mut self,
        poll: &Arc<polling::Poller>,
        interest: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        self.reader.pty.reregister(poll, interest, mode)
    }

    fn deregister(&mut self, poll: &Arc<polling::Poller>) -> io::Result<()> {
        self.reader.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut ScanningReader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self.reader.pty.writer()
    }
}

impl EventedPty for ScanPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.reader.pty.next_child_event()
    }
}

impl OnResize for ScanPty {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.reader.pty.on_resize(window_size);
    }
}
