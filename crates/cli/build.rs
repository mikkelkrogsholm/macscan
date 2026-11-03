mod duckdb {
    include!("../../scripts/link-duckdb.rs");
    pub fn link() {
        main();
    }
}
fn main() {
    duckdb::link();
    // Build-time only: packaged discovery never consults a working tree.
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_owned())
    };
    let revision = git(&["describe", "--always", "--dirty", "--exclude=*"])
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MACSCAN_BUILD_REVISION={revision}");
    for item in ["HEAD", "index"] {
        if let Some(path) = git(&["rev-parse", "--git-path", item]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git(&["rev-parse", "--git-path", &reference]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rerun-if-changed=../../crates");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-changed=build.rs");
}
