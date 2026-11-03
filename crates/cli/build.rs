mod duckdb {
    include!("../../scripts/link-duckdb.rs");
    pub fn link() {
        main();
    }
}
fn main() {
    duckdb::link();
    identity::emit();
}
mod identity {
    include!("../../scripts/build-identity.rs");
    pub fn emit() { main(); }
}
