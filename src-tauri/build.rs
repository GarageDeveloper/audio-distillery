fn main() {
    // Native AU editor shim (AppKit/CoreAudioKit), macOS targets only.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("native/au_editor.m")
            .flag("-fobjc-arc")
            .flag("-fmodules")
            .compile("au_editor");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=AudioToolbox");
        println!("cargo:rustc-link-lib=framework=CoreAudioKit");
        println!("cargo:rerun-if-changed=native/au_editor.m");
    }
    tauri_build::build()
}
