//! Embeds the icon and version info into the exe (Explorer, Start Menu, taskbar pin).
//! The icon is drawn by `src/icon.rs`, the same code the app uses for its window icon,
//! and written to `OUT_DIR/hwmon.ico`, where `installer/build.ps1` also picks it up.

#[path = "src/icon.rs"]
mod icon;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let ico = std::path::Path::new(&std::env::var("OUT_DIR").expect("set by Cargo")).join("hwmon.ico");
        std::fs::write(&ico, icon::ico()).expect("writing the icon into OUT_DIR");
        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico.to_str().expect("OUT_DIR is valid UTF-8"))
            .set("FileDescription", "Hardware Monitor")
            .set("ProductName", "Hardware Monitor")
            .set("CompanyName", "dysolix");
        res.compile().expect("embedding the icon needs rc.exe from the Windows SDK");
    }
    // Load the DLLs the exe imports (version.dll, dbghelp.dll …) only from System32,
    // never from the exe's own folder: a portable copy usually sits in Downloads, where
    // a planted DLL would otherwise run, as administrator after "Restart as administrator".
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
}
