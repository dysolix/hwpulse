# HWPulse

A lightweight hardware monitor for Windows, written in Rust with [egui](https://github.com/emilk/egui).

- **Dashboard** with live gauges and history charts for GPU, CPU and memory; drives and network adapters can be switched on
- **Components** page in the style of Task Manager, including a per-thread CPU grid and top processes
- **Processes** page: every process with GPU use and 20+ optional columns, sortable, searchable, grouped by app and into Apps / Background / Windows processes; end (also the whole process tree), suspend, resume, efficiency mode, priority, affinity, dump files, run new task (restart as administrator for elevated and SYSTEM processes)
- **Services**, **Startup apps** and **Users** pages like Task Manager's: start and stop services, switch startup entries on and off, disconnect or sign out users
- **GPU** load, temperature, clocks, power, VRAM and NVENC/NVDEC usage (NVIDIA via NVML; AMD and Intel via DirectX)
- **Drives** identified by serial number, with HDD/SSD and bus type; USB drives come and go live
- Drag-and-drop layout, saved automatically; compact view, always on top, update speed; optional start with Windows
- Low overhead: one sample per second (adjustable or paused), and almost no CPU while in the background

## Install

Download from [Releases](../../releases):

- `hwpulse-setup-<version>.exe` installs for the current user (no admin rights) to `%LOCALAPPDATA%\Programs\hwpulse`, or, if you choose "Install for all users", to `Program Files` (admin rights). The all-users install is the safer choice if you use "Restart as administrator", since only admins can replace the exe there. Silent installs take `/CURRENTUSER` or `/ALLUSERS`.
- `hwpulse-<version>-portable.exe` runs from anywhere

Requires 64-bit Windows 10 or 11. The most detailed GPU readings need an NVIDIA GPU. Settings are stored in `%APPDATA%\hwpulse`.

## Build

```powershell
cargo run --release
```

`installer\build.ps1` builds the release exe and the installer (needs [Inno Setup 6](https://jrsoftware.org/isinfo.php)).

## Release

Run the **Version** workflow in GitHub Actions and pick the bump. It updates `Cargo.toml`, tags the commit and starts **Release**, which builds both files, signs them with [minisign](https://jedisct1.github.io/minisign/) and publishes them. The public key is `installer/hwpulse.pub`.
