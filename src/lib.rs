pub mod app;
pub mod chrome;
pub mod claude_setup;
pub mod claude_status;
pub mod config;
pub mod file_icons;
pub mod fonts;
pub mod fsutil;
pub mod git;
pub mod graph;
pub mod host;
pub mod hotkeys;
pub mod jwt;
pub mod layout;
pub mod logging;
pub(crate) mod process;
pub mod procs;
pub mod profiles;
pub mod quota;
pub mod session;
pub mod settings_ui;
pub mod strings;
pub mod tabs;
pub mod term;
pub mod theme;
pub mod workspace;

/// Apply before any runtime DLL load in either distributed executable.
/// Working directories and PATH are not library search locations.
pub fn secure_dll_search() -> std::io::Result<()> {
    use windows_sys::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    // SAFETY: the caller applies this process-wide policy before spawning threads.
    if unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
mod agents;
