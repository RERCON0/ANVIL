//! Test helper for tests/conpty.rs and tests/git.rs: a tiny console program that behaves like
//! the applications ANVIL must serve (omp, opencode). Not shipped.
//!
//! anvil-probe stdin <out>   raw VT stdin + CSI ? 9001 h; records stdin until 'q'
//! anvil-probe mouse         turns mouse tracking on, waits, turns it off, exits
//! anvil-probe queries <out> asks DA1 and CPR, records the replies
//! anvil-probe size <out>    waits for 's', writes "COLSxROWS"
//! anvil-probe cwd           reports C:\Windows via OSC 1337 and exits
//! anvil-probe pwd           prints its working directory (the AI-message runner)
//! anvil-probe sleep         sleeps for a minute (the AI-message timeout)

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Console::{
    GetConsoleMode, GetConsoleScreenBufferInfo, GetStdHandle, SetConsoleMode, CONSOLE_SCREEN_BUFFER_INFO,
    ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_VIRTUAL_TERMINAL_INPUT, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE,
};

fn raw_vt_stdin() {
    // SAFETY: plain console mode calls on this process's own stdin handle.
    unsafe {
        let h = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode = 0;
        GetConsoleMode(h, &mut mode);
        mode &= !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT);
        mode |= ENABLE_VIRTUAL_TERMINAL_INPUT;
        SetConsoleMode(h, mode);
    }
}

fn out(s: &str) {
    let mut o = std::io::stdout();
    o.write_all(s.as_bytes()).unwrap();
    o.flush().unwrap();
}

/// Reads stdin on a thread until `stop` is seen or `limit` passes.
fn record(stop: u8, limit: Duration) -> Vec<u8> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 256];
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + limit;
    let mut got = Vec::new();
    while Instant::now() < deadline {
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(50)) {
            got.extend(chunk);
            if got.contains(&stop) {
                break;
            }
        }
    }
    got
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let scenario = args.get(1).map(String::as_str).unwrap_or("");
    let path = args.get(2).cloned();
    match scenario {
        "stdin" => {
            raw_vt_stdin();
            out("\x1b[?9001h");
            out("READY\r\n");
            let got = record(b'q', Duration::from_secs(5));
            out("\x1b[?9001l");
            std::fs::write(path.unwrap(), got).unwrap();
        }
        "mouse" => {
            out("\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h");
            out("ON\r\n");
            std::thread::sleep(Duration::from_millis(700));
            out("\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?1006l");
        }
        "queries" => {
            raw_vt_stdin();
            out("\x1b[c\x1b[6n");
            let got = record(b'R', Duration::from_secs(3));
            std::fs::write(path.unwrap(), got).unwrap();
        }
        "size" => {
            raw_vt_stdin();
            out("READY\r\n");
            let _ = record(b's', Duration::from_secs(5));
            // SAFETY: zeroed out-struct for the console API.
            let info = unsafe {
                let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
                GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info);
                info
            };
            let cols = info.srWindow.Right - info.srWindow.Left + 1;
            let rows = info.srWindow.Bottom - info.srWindow.Top + 1;
            std::fs::write(path.unwrap(), format!("{cols}x{rows}")).unwrap();
        }
        "cwd" => {
            out("\x1b]1337;CurrentDir=C:\\Windows\x07");
            out("DONE\r\n");
            std::thread::sleep(Duration::from_millis(300));
        }
        "pwd" => out(&std::env::current_dir().unwrap().to_string_lossy()),
        "sleep" => std::thread::sleep(Duration::from_secs(60)),
        _ => {
            eprintln!("unknown scenario");
            std::process::exit(2);
        }
    }
}
