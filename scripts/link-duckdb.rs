fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let library = root.join(".build/duckdb");
    println!("cargo:rustc-link-search=native={}", library.display());
    println!("cargo:rustc-link-lib=dylib=duckdb");
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", library.display());
    println!("cargo:rerun-if-changed=../../scripts/link-duckdb.rs");
}
