use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=vendor/conpty/x64/conpty.dll");
    println!("cargo:rerun-if-changed=vendor/conpty/x64/OpenConsole.exe");
    println!("cargo:rerun-if-changed=icons/anvil.rc");
    println!("cargo:rerun-if-changed=icons/anvil.ico");
    copy_conpty();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if Path::new("icons/anvil.ico").exists() {
            embed_resource::compile_for("icons/anvil.rc", ["anvil"], embed_resource::NONE)
                .manifest_optional()
                .expect("failed to embed the ANVIL icon");
        } else {
            println!("cargo:warning=icons/anvil.ico missing, run `cargo run --example gen_icons`");
        }
    }
}

/// Puts the bundled ConPTY next to the binaries (target/<profile>/) and the
/// test executables (target/<profile>/deps/): alacritty_terminal loads
/// conpty.dll from the executable's directory.
fn copy_conpty() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    // OUT_DIR = target/<profile>/build/<package>-<hash>/out
    let Some(profile_dir) = out_dir.ancestors().nth(3) else { return };
    for dir in [profile_dir.to_path_buf(), profile_dir.join("deps")] {
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        for name in ["conpty.dll", "OpenConsole.exe"] {
            let src = Path::new("vendor/conpty/x64").join(name);
            let dst = dir.join(name);
            let same = matches!(
                (std::fs::metadata(&src), std::fs::metadata(&dst)),
                (Ok(a), Ok(b)) if a.len() == b.len()
            );
            if !same {
                if let Err(e) = std::fs::copy(&src, &dst) {
                    // The old copy may be in use by a running ANVIL or test.
                    println!("cargo:warning=could not copy {}: {e}", src.display());
                }
            }
        }
    }
}
