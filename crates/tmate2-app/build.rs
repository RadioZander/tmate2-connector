//! Records which commit the connector is built from (`git describe`), for
//! the version shown in the window, `--version` and the log. See
//! `src/version.rs`.

use std::path::Path;
use std::process::Command;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // Look again after a commit, a checkout, a new tag or an edited source.
    // A missing path would make Cargo rerun this every build.
    for p in [
        ".git/HEAD",
        ".git/index",
        ".git/refs",
        ".git/packed-refs",
        "crates",
    ] {
        let p = root.join(p);
        if p.exists() {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }
    // A copy of the source without its git history can say what it is.
    println!("cargo:rerun-if-env-changed=TMATE2_BUILD");
    let describe = std::env::var("TMATE2_BUILD").ok().or_else(|| {
        let out = Command::new("git")
            .args(["describe", "--tags", "--long", "--dirty", "--match", "v[0-9]*"])
            .current_dir(&root)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    });
    println!("cargo:rustc-env=TMATE2_DESCRIBE={}", describe.unwrap_or_default());
}
