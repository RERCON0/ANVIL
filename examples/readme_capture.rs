//! Generate the README screenshot using real CLI agents in an isolated sample Git workspace.
//! cargo run --locked --example readme_capture -- assets/workspace.png
//! Add --demo to capture a 10-second PNG sequence in a new output directory.
#[cfg(debug_assertions)]
fn main() {
    use std::path::Path;
    use std::process::Command;
    let fixture = tempfile::tempdir().expect("sample workspace");
    let root = fixture.path();
    // The automation shell sets NO_COLOR; a normally opened terminal does not.
    // Remove it only in this helper process, preserving the user's global settings.
    std::env::remove_var("NO_COLOR");
    std::env::remove_var("NODE_DISABLE_COLORS");
    // Applies only to this helper process and its disposable Git children.
    let global = root.join("gitconfig");
    std::fs::write(&global, "").unwrap();
    std::env::set_var("GIT_CONFIG_GLOBAL", global);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    let git = |folder: &Path, args: &[&str]| {
        let result = Command::new("git").current_dir(folder).args(args).output().expect("git");
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    };
    for project in ["ANVIL", "BEAT", "SNATCH"] {
        let folder = root.join(project);
        std::fs::create_dir_all(folder.join("src")).unwrap();
        git(&folder, &["init", "-q", "-b", "main"]);
        git(&folder, &["config", "user.name", "RERCON0"]);
        git(&folder, &["config", "user.email", "demo@example.invalid"]);
        for (index, subject) in [
            "Create the native workspace",
            "Add project tabs and split panes",
            "Add Git history and hunk staging",
            "Monitor AI provider quotas",
            "Keep pane layouts between sessions",
            "Guard Git configuration and file access",
        ]
        .iter()
        .enumerate()
        {
            std::fs::write(
                folder.join("src/main.rs"),
                format!("// {subject}\nfn main() {{ println!(\"revision {index}\"); }}\n"),
            )
            .unwrap();
            git(&folder, &["add", "."]);
            git(&folder, &["commit", "-qm", subject]);
        }
        std::fs::write(folder.join("src/main.rs"), "// Project workspace\nfn main() { println!(\"ANVIL\"); }\n")
            .unwrap();
        std::fs::write(folder.join("src/commands.rs"), "pub enum Command { ClosePreview, SplitPane }\n").unwrap();
        git(&folder, &["add", "src/commands.rs"]);
        std::fs::write(folder.join("README.md"), "# Project workspace\n\nTabs, Git, quotas and multiple CLI agents.\n")
            .unwrap();
    }
    let executable = std::env::current_exe().unwrap();
    for name in ["conpty.dll", "OpenConsole.exe"] {
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/conpty/x64").join(name),
            executable.parent().unwrap().join(name),
        )
        .unwrap();
    }
    let output = std::env::args_os().nth(1).map(std::path::PathBuf::from).expect("output PNG path");
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
    let app = anvil::app::AnvilApp::readme_scene(root.to_owned());
    if std::env::args().nth(2).as_deref() == Some("--demo") {
        anvil::host::event_loop::run_demo(app, output, write_frame);
    } else {
        anvil::host::event_loop::run_capture(app, output, write_frame);
    }
}

#[cfg(debug_assertions)]
fn write_frame(path: &std::path::Path, width: u32, height: u32, pixels: Vec<u8>) -> Result<(), String> {
    let mut image = image::RgbaImage::from_raw(width, height, pixels).ok_or("invalid framebuffer size")?;
    image::imageops::flip_vertical_in_place(&mut image);
    image.save(path).map_err(|error| error.to_string())
}

#[cfg(not(debug_assertions))]
fn main() {
    eprintln!("README capture is a debug-only development tool.");
}
