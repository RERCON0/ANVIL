//! Real ConPTY sessions without a window. Needs vendor/conpty next to the test
//! binary (build.rs copies it into target/<profile>/deps).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::TermMode;
use anvil::term::pane::{bundled_conpty_loaded, Pane, PaneEvent, SpawnOptions};
use anvil::term::style::Palette;

fn spawn(program: &str, args: &[&str]) -> Pane {
    let opts = SpawnOptions {
        pane_id: 1,
        program: program.to_owned(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: None,
        env: HashMap::new(),
        columns: 80,
        lines: 24,
        cell_width: 8,
        cell_height: 16,
        scrollback: 1000,
        word_separators: " ".into(),
        allow_osc52: false,
        palette: Palette::hardcore(),
        cursor_style: alacritty_terminal::vte::ansi::CursorStyle::default(),
    };
    Pane::spawn(opts, Arc::new(|| {})).expect("spawn")
}

fn probe(args: &[&str]) -> Pane {
    spawn(env!("CARGO_BIN_EXE_anvil-probe"), args)
}

fn screen_text(pane: &Pane) -> String {
    let term = pane.term.lock();
    let mut lines: Vec<String> = Vec::new();
    let mut current = None;
    for cell in term.renderable_content().display_iter {
        if current != Some(cell.point.line) {
            lines.push(String::new());
            current = Some(cell.point.line);
        }
        lines.last_mut().unwrap().push(cell.c);
    }
    lines.join("\n")
}

fn wait_until(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    cond()
}

fn wait_exit(pane: &Pane) -> Option<i32> {
    let mut code = None;
    let exited = wait_until(Duration::from_secs(10), || {
        for e in pane.drain_events() {
            if let PaneEvent::Exited(c) = e {
                code = Some(c);
            }
        }
        code.is_some()
    });
    assert!(exited, "process did not exit");
    code.flatten()
}

fn out_file(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("anvil-conpty-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn runs_a_command_and_reports_exit_code() {
    let pane = spawn("cmd.exe", &["/c", "echo anvil-ok"]);
    assert!(wait_until(Duration::from_secs(10), || screen_text(&pane).contains("anvil-ok")));
    assert_eq!(wait_exit(&pane), Some(0));
}

#[test]
fn uses_the_bundled_conpty() {
    let _pane = spawn("cmd.exe", &["/c", "exit"]);
    assert!(bundled_conpty_loaded(), "conpty.dll was not loaded: the system ConPTY is in use");
}

/// Regression (Helm 2026-10-03): with the system ConPTY an application in
/// win32-input-mode (omp) received ESC [ A as three per-character key events.
#[test]
fn input_reaches_win32_input_mode_apps_unchanged() {
    let out = out_file("stdin.bin");
    let pane = probe(&["stdin", out.to_str().unwrap()]);
    assert!(wait_until(Duration::from_secs(10), || screen_text(&pane).contains("READY")));
    pane.write(b"\x1b[A".to_vec());
    std::thread::sleep(Duration::from_millis(300));
    pane.write(b"q".to_vec());
    wait_exit(&pane);
    assert_eq!(std::fs::read(&out).unwrap(), b"\x1b[Aq");
}

/// Regression (Helm 2026-10-03): the system ConPTY dropped the mouse-mode
/// resets opencode writes on exit, so mouse motion was typed into bash.
#[test]
fn mouse_mode_resets_reach_the_terminal() {
    let pane = probe(&["mouse"]);
    assert!(wait_until(Duration::from_secs(10), || pane.term.lock().mode().contains(TermMode::MOUSE_MOTION)));
    wait_exit(&pane);
    assert!(wait_until(Duration::from_secs(2), || {
        let mode = *pane.term.lock().mode();
        !mode.intersects(TermMode::MOUSE_MODE) && !mode.contains(TermMode::SGR_MOUSE)
    }));
}

#[test]
fn answers_device_attributes_and_cursor_position() {
    let out = out_file("queries.bin");
    let pane = probe(&["queries", out.to_str().unwrap()]);
    wait_exit(&pane);
    let got = String::from_utf8_lossy(&std::fs::read(&out).unwrap()).into_owned();
    assert!(got.contains("\x1b[?"), "no DA1 reply in {got:?}");
    assert!(got.contains('c') && got.contains('R'), "replies missing in {got:?}");
}

#[test]
fn resize_reaches_the_console() {
    let out = out_file("size.txt");
    let mut pane = probe(&["size", out.to_str().unwrap()]);
    assert!(wait_until(Duration::from_secs(10), || screen_text(&pane).contains("READY")));
    pane.resize(100, 30, 8, 16);
    std::thread::sleep(Duration::from_millis(300));
    pane.write(b"s".to_vec());
    wait_exit(&pane);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "100x30");
}

#[test]
fn tracks_reported_working_directory() {
    let pane = probe(&["cwd"]);
    assert!(wait_until(Duration::from_secs(10), || screen_text(&pane).contains("DONE")));
    assert_eq!(pane.current_dir(), Some(PathBuf::from("C:\\Windows")));
}

/// The settings page applies at once, but a changed scrollback (and word
/// separators) used to reach only panes opened afterwards.
#[test]
fn scrollback_setting_reaches_a_running_pane() {
    let pane = spawn("cmd.exe", &["/c", "ping -n 2 127.0.0.1 >nul & for /L %i in (1,1,200) do @echo %i"]);
    pane.set_options(alacritty_terminal::vte::ansi::CursorStyle::default(), 20, " ", false);
    wait_exit(&pane);
    let history = pane.term.lock().grid().history_size();
    assert!(history <= 20, "history of {history} lines kept with a 20-line scrollback");
}

/// Benchmark (run with `cargo test --test conpty -- --ignored --nocapture`):
/// how fast the ConPTY -> Term pipeline swallows a two-million-line flood.
/// Measures the whole pane path, not the shell's own speed.
#[test]
#[ignore = "benchmark: run explicitly"]
fn flood_throughput_benchmark() {
    let start = Instant::now();
    let pane = spawn("cmd.exe", &["/c", "for /L %i in (1,1,2000000) do @echo %i"]);
    let mut code = None;
    let deadline = Instant::now() + Duration::from_secs(600);
    while Instant::now() < deadline {
        for event in pane.drain_events() {
            if let PaneEvent::Exited(status) = event {
                code = Some(status);
            }
        }
        if code.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let elapsed = start.elapsed();
    let text = screen_text(&pane);
    println!(
        "flood: {elapsed:?} to exit ({code:?}); last line on screen: {}",
        text.contains("1999999") || text.contains("2000000")
    );
    assert!(code.is_some(), "the flood must finish, timed out after {elapsed:?}");
}

/// Closing a pane while it floods output must not block the UI thread.
#[test]
fn dropping_a_busy_pane_does_not_hang() {
    let pane = spawn("cmd.exe", &["/c", "dir /s C:\\Windows"]);
    std::thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    drop(pane);
    assert!(started.elapsed() < Duration::from_secs(1), "drop took {:?}", started.elapsed());
}
