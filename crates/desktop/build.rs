fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    println!(
        "cargo:rustc-link-search=native={}",
        root.join(".build/duckdb").display()
    );
    println!("cargo:rustc-link-lib=dylib=duckdb");
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    println!(
        "cargo:rustc-link-arg=-Wl,-rpath,{}",
        root.join(".build/duckdb").display()
    );
    tauri_build::build();
}
