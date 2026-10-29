# Hardware Monitor

A lightweight hardware monitor for Windows, written in Rust with [egui](https://github.com/emilk/egui).

- **Dashboard** with live gauges and history charts for GPU, CPU and memory; drives and network adapters can be switched on
- **Performance** page in the style of Task Manager, including a per-thread CPU grid
- **GPU** load, temperature, clocks, power, VRAM and NVENC/NVDEC usage (NVIDIA, via NVML)
- **Drives** identified by serial number, with HDD/SSD and bus type; USB drives come and go live
- Drag-and-drop layout, saved automatically; optional start with Windows
- Low overhead: one sample per second, and almost no CPU while in the background

## Install

Download from [Releases](../../releases):

- `hwmon-setup-<version>.exe` installs for the current user (no admin rights) to `%LOCALAPPDATA%\Programs\hwmon`
- `hwmon-<version>-portable.exe` runs from anywhere

Requires 64-bit Windows 10 or 11. GPU readings need an NVIDIA GPU and driver. Settings are stored in `%APPDATA%\hwmon`.

## Build

```powershell
cargo run --release
```

`installer\build.ps1` builds the release exe and the installer (needs [Inno Setup 6](https://jrsoftware.org/isinfo.php)).

## Release

Run the **Version** workflow in GitHub Actions and pick the bump. It updates `Cargo.toml`, tags the commit and starts **Release**, which builds both files and publishes them.
