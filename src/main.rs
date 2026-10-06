// GUI exe: no console window, in debug as well (as in SNATCH). Panics and
// everything else go to %APPDATA%\anvil\anvil.log.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    if let Err(error) = secure_dll_search() {
        show_fatal(&format!("Не удалось защитить поиск DLL: {error}"));
        return;
    }
    let dir = anvil::config::app_dir();
    let level = if cfg!(debug_assertions) { log::LevelFilter::Debug } else { log::LevelFilter::Info };
    let _ = anvil::logging::init(dir.join("anvil.log"), level);
    // A windows-subsystem exe dies silently, so a panic on the main thread
    // also shows a dialog; panics in pane reader threads only reach the log.
    let main_thread = std::thread::current().id();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture());
        if std::thread::current().id() == main_thread {
            static SHOWN: std::sync::OnceLock<()> = std::sync::OnceLock::new();
            if SHOWN.set(()).is_ok() {
                show_fatal(&format!(
                    "ANVIL остановлен внутренней ошибкой.\n\n{info}\n\nПодробности: {}",
                    dir.join("anvil.log").display()
                ));
            }
        }
    }));
    // Credential copies from a crashed AI run are removed in the background:
    // startup never scans %TEMP% on the main thread.
    if let Err(error) =
        std::thread::Builder::new().name("ai-state-sweep".to_owned()).spawn(anvil::git::sweep_stale_ai_state)
    {
        log::info!("cannot start the AI state sweep: {error}");
    }
    anvil::host::run(anvil::app::AnvilApp::new());
}

/// Never search the working directory or PATH for implicitly loaded libraries.
/// The bundled ConPTY may load only beside ANVIL; missing bundles fall back to
/// the Windows pseudoconsole API, not a repository's conpty.dll.
fn secure_dll_search() -> std::io::Result<()> {
    use windows_sys::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    // SAFETY: process-wide loader policy is set before spawning threads or panes.
    if unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Modal error box for fatal startup failures (no console in this subsystem).
fn show_fatal(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let text = wide(message);
    let title = wide(anvil::strings::APP_TITLE);
    // SAFETY: NUL-terminated UTF-16 strings; owned for the duration of the call.
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_ICONERROR | MB_OK);
    }
}
