#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let dir = anvil::config::app_dir();
    let level = if cfg!(debug_assertions) { log::LevelFilter::Debug } else { log::LevelFilter::Info };
    let _ = anvil::logging::init(dir.join("anvil.log"), level);
    std::panic::set_hook(Box::new(|info| {
        log::error!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture());
    }));
    anvil::host::run(anvil::app::AnvilApp::new());
}
