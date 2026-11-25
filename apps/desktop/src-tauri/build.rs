fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        // WebKitGTK links ordinary SQLite. Keep the executable's bundled
        // SQLCipher symbols private so they cannot interpose on that library.
        let map = "src/database/linux-vault.map";
        println!("cargo:rerun-if-changed={map}");
        let root = std::env::var("CARGO_MANIFEST_DIR").expect("Cargo supplies the manifest path");
        println!("cargo:rustc-link-arg-bin=app=-Wl,--version-script={root}/{map}");
    }
    tauri_build::build()
}
