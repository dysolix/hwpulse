//! Embeds the icon and version info into the exe (Explorer, Start Menu, taskbar pin).

fn main() {
    println!("cargo:rerun-if-changed=assets/hwmon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/hwmon.ico").set("FileDescription", "Hardware Monitor").set("ProductName", "Hardware Monitor").set("CompanyName", "dysolix");
        res.compile().expect("embedding the icon needs rc.exe from the Windows SDK");
    }
}
