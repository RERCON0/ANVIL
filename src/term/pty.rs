//! The PTY handed to alacritty's event loop: the real ConPTY plus a tap on
//! its output that picks up working-directory reports and bounds OSC strings
//! before VTE parses them (see osc_scan.rs).

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{self, ChildEvent, EventedPty, EventedReadWrite};

use crate::term::osc_scan::{OscGuard, OscScanner};

pub struct ScanningReader {
    pty: tty::Pty,
    scanner: OscScanner,
    guard: OscGuard,
    cwd: Arc<Mutex<Option<PathBuf>>>,
    poll: Option<Arc<polling::Poller>>,
}

impl ScanningReader {
    fn request_read(&self) -> io::Result<()> {
        // Windows' piper reader removes its waker after a successful read.
        // Yielding before another read must post readiness ourselves, even
        // after the LAST buffered byte: OSC completion can turn a tiny raw
        // read into >MAX_LOCKED_READ output and make the event loop stop before
        // the next raw read registers the waker for future output.
        #[cfg(windows)]
        if let Some(poll) = &self.poll {
            use polling::os::iocp::{CompletionPacket, PollerIocpExt};
            poll.post(CompletionPacket::new(polling::Event::readable(tty::PTY_READ_WRITE_TOKEN)))?;
        }
        Ok(())
    }
}

impl Read for ScanningReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let pending = self.guard.drain(buf);
        if pending != 0 || buf.is_empty() {
            if pending != 0 {
                self.request_read()?;
            }
            return Ok(pending);
        }
        let n = self.pty.reader().read(buf)?;
        if let Some(dir) = self.scanner.feed(&buf[..n]) {
            *self.cwd.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir);
        }
        if !self.guard.filter(&buf[..n]) {
            return Ok(n);
        }
        let filtered = self.guard.drain(buf);
        if n != 0 {
            self.request_read()?;
        }
        if filtered == 0 && n != 0 {
            // Buffering/discarding an OSC is not EOF. Yield to the poller rather
            // than reading an unlimited unterminated string in this call.
            return Err(io::ErrorKind::WouldBlock.into());
        }
        Ok(filtered)
    }
}

pub struct ScanPty {
    reader: ScanningReader,
}

impl ScanPty {
    pub fn new(pty: tty::Pty, cwd: Arc<Mutex<Option<PathBuf>>>) -> ScanPty {
        ScanPty { reader: ScanningReader { pty, scanner: OscScanner::new(), guard: OscGuard::default(), cwd, poll: None } }
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
        unsafe { self.reader.pty.register(poll, interest, mode)? };
        self.reader.poll = Some(poll.clone());
        Ok(())
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
        self.reader.poll = None;
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
