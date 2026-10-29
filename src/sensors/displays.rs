//! Connected monitors (name, resolution, refresh rate, HDR, connection) via the
//! Display Configuration API, so the GPU page can list what it drives.

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Gdi::{DISPLAY_DEVICEW, EnumDisplayDevicesW};
use windows::core::PCWSTR;

pub struct Display {
    /// Graphics adapter driving it, e.g. "NVIDIA GeForce RTX 4070 Ti".
    pub adapter: String,
    pub monitor: String,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f64,
    pub hdr: bool,
    pub connection: Option<&'static str>,
}

impl Display {
    /// E.g. "2560 × 1440 · 165 Hz · HDR · DisplayPort".
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("{} × {}", self.width, self.height)];
        if self.refresh_hz > 0.0 {
            parts.push(format!("{:.0} Hz", self.refresh_hz));
        }
        if self.hdr {
            parts.push("HDR".into());
        }
        parts.extend(self.connection.map(String::from));
        parts.join(" · ")
    }
}

pub fn displays() -> Vec<Display> {
    let (mut path_count, mut mode_count) = (0u32, 0u32);
    if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count) } != ERROR_SUCCESS
    {
        return Vec::new();
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
    let status = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Vec::new();
    }
    paths.truncate(path_count as usize);
    let adapters = adapter_names();

    paths
        .iter()
        .map(|path| {
            let source = &path.sourceInfo;
            let target = &path.targetInfo;

            let mut source_name = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source_name.header = header(
                DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size_of_val(&source_name),
                source.adapterId,
                source.id,
            );
            unsafe { DisplayConfigGetDeviceInfo(&mut source_name.header) };
            let gdi = wide(&source_name.viewGdiDeviceName);

            let mut target_name = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            target_name.header = header(
                DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size_of_val(&target_name),
                target.adapterId,
                target.id,
            );
            unsafe { DisplayConfigGetDeviceInfo(&mut target_name.header) };

            let mut color = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
            color.header = header(
                DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                size_of_val(&color),
                target.adapterId,
                target.id,
            );
            let hdr = unsafe { DisplayConfigGetDeviceInfo(&mut color.header) } == 0
                && unsafe { color.Anonymous.value } & 0b10 != 0; // advancedColorEnabled

            let mode = unsafe { source.Anonymous.modeInfoIdx } as usize;
            let (width, height) = modes
                .get(mode)
                .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
                .map(|m| unsafe { (m.Anonymous.sourceMode.width, m.Anonymous.sourceMode.height) })
                .unwrap_or_default();
            let rate = target.refreshRate;

            Display {
                adapter: adapters.iter().find(|(name, _)| *name == gdi).map(|(_, a)| a.clone()).unwrap_or_default(),
                monitor: Some(wide(&target_name.monitorFriendlyDeviceName))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "Display".into()),
                width,
                height,
                refresh_hz: if rate.Denominator > 0 { rate.Numerator as f64 / rate.Denominator as f64 } else { 0.0 },
                hdr,
                connection: match target.outputTechnology.0 {
                    0 => Some("VGA"),
                    4 => Some("DVI"),
                    5 => Some("HDMI"),
                    10 | 11 => Some("DisplayPort"),
                    15 => Some("USB-C"),
                    i32::MIN => Some("Internal"),
                    _ => None,
                },
            }
        })
        .collect()
}

fn header(
    kind: windows::Win32::Devices::Display::DISPLAYCONFIG_DEVICE_INFO_TYPE,
    size: usize,
    adapter: windows::Win32::Foundation::LUID,
    id: u32,
) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: kind, size: size as u32, adapterId: adapter, id }
}

/// `(\\.\DISPLAYn, adapter name)` for every display output.
fn adapter_names() -> Vec<(String, String)> {
    (0..)
        .map_while(|i| {
            let mut device = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
            unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut device, 0) }
                .as_bool()
                .then(|| (wide(&device.DeviceName), wide(&device.DeviceString)))
        })
        .collect()
}

fn wide(chars: &[u16]) -> String {
    let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..end])
}
