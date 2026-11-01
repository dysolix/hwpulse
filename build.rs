//! Embeds the icon and version info into the exe (Explorer, Start Menu, taskbar pin).

fn main() {
    println!("cargo:rerun-if-changed=assets/hwmon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/hwmon.ico")
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
