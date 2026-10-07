use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=vendor/conpty/x64/conpty.dll");
    println!("cargo:rerun-if-changed=vendor/conpty/x64/OpenConsole.exe");
    println!("cargo:rerun-if-changed=icons/anvil.rc");
    println!("cargo:rerun-if-changed=icons/anvil.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Only x64 has a vendored ConPTY; other architectures would silently
        // fall back to the system one and reintroduce the documented input bugs.
        let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
        if arch != "x86_64" {
            panic!("ANVIL ships a vendored x64 ConPTY; the {arch} Windows target is not supported");
        }
        if Path::new("icons/anvil.ico").exists() {
            embed_resource::compile_for("icons/anvil.rc", ["anvil"], embed_resource::NONE)
                .manifest_optional()
                .expect("failed to embed the ANVIL icon");
        } else {
            println!("cargo:warning=icons/anvil.ico missing, run `cargo run --example gen_icons`");
        }
    }
    copy_conpty();
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
            // A missing or unreadable vendored runtime must stop the build: with
            // no copy next to the binaries yet, both reads would fail, compare
            // equal and leave ANVIL on the system ConPTY without a word.
            let wanted = std::fs::read(&src).unwrap_or_else(|e| panic!("cannot read {}: {e}", src.display()));
            // Compare the contents, not just the sizes: a same-length update of
            // the vendored runtime must be re-copied.
            if std::fs::read(&dst).is_ok_and(|current| current == wanted) {
                continue;
            }
            match std::fs::copy(&src, &dst) {
                Ok(_) => {}
                Err(e) if dst.exists() => {
                    // The old copy may be in use by a running ANVIL or test.
                    println!("cargo:warning=could not replace {}: {e}", dst.display());
                }
                Err(e) => panic!("cannot copy {} next to the binaries: {e}", src.display()),
            }
        }
    }
}
