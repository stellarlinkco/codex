fn main() {
    // The Web UI is embedded via include_dir!(). Make sure Cargo rebuilds the crate when assets
    // change (for example after running `just write-serve-web-assets`).
    println!("cargo:rerun-if-changed=assets/web");
}
