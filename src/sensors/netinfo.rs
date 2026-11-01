//! Network adapter details (addresses, gateway, DNS, MAC) from the IP Helper API and
//! Wi-Fi connection details from the WLAN API. Both work without admin rights.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS, HANDLE};
use windows::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::WiFi::{
    WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory,
    WlanOpenHandle, WlanQueryInterface, wlan_interface_state_connected, wlan_intf_opcode_current_connection,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6};

pub struct Adapter {
    /// Hardware description, e.g. "Intel(R) Ethernet Controller (3) I225-V".
    pub description: String,
    /// Name in Windows settings, e.g. "Ethernet".
    pub name: String,
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
    pub mac: Option<String>,
}

impl Adapter {
    /// Whether this adapter is the one behind a PDH `Network Interface` instance,
    /// whose names replace `()#/\` with `[]_ _ _`.
    pub fn matches_pdh(&self, instance: &str) -> bool {
        let pdh: String = self
            .description
            .chars()
            .map(|c| match c {
                '(' => '[',
                ')' => ']',
                '#' | '/' | '\\' => '_',
                c => c,
            })
            .collect();
        pdh == instance
    }
}

pub fn adapters() -> Vec<Adapter> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
    let mut size = 16 * 1024u32;
    let mut buffer: Vec<u64> = Vec::new();
    let mut filled = false;
    for _ in 0..3 {
        buffer = vec![0; (size as usize).div_ceil(8)];
        let status = unsafe {
            GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(buffer.as_mut_ptr().cast()), &mut size)
        };
        if status == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if status != ERROR_SUCCESS.0 {
            return Vec::new();
        }
        filled = true;
        break;
    }
    // Still too small after three tries (adapters keep appearing): the buffer holds no
    // list, and reading it as one would follow garbage.
    if !filled {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut next = buffer.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    // SAFETY: a linked list inside `buffer`, which outlives the loop.
    while let Some(a) = unsafe { next.as_ref() } {
        next = a.Next;
        let mut adapter = Adapter {
            description: unsafe { a.Description.to_string() }.unwrap_or_default(),
            name: unsafe { a.FriendlyName.to_string() }.unwrap_or_default(),
            ipv4: Vec::new(),
            ipv6: Vec::new(),
            gateway: None,
            dns: Vec::new(),
            mac: None,
        };
        let mut unicast = a.FirstUnicastAddress;
        while let Some(u) = unsafe { unicast.as_ref() } {
            match socket_ip(u.Address.lpSockaddr) {
                Some(IpAddr::V4(ip)) => adapter.ipv4.push(ip.to_string()),
                // Link-local fe80:: addresses are noise for an overview.
                Some(IpAddr::V6(ip)) if !ip.is_unicast_link_local() => adapter.ipv6.push(ip.to_string()),
                _ => {}
            }
            unicast = u.Next;
        }
        let mut gateway = a.FirstGatewayAddress;
        while let Some(g) = unsafe { gateway.as_ref() } {
            if let Some(ip) = socket_ip(g.Address.lpSockaddr) {
                // Prefer the IPv4 gateway, the one people recognise.
                if adapter.gateway.is_none() || ip.is_ipv4() {
                    adapter.gateway = Some(ip.to_string());
                }
            }
            gateway = g.Next;
        }
        let mut dns = a.FirstDnsServerAddress;
        while let Some(d) = unsafe { dns.as_ref() } {
            if let Some(ip) = socket_ip(d.Address.lpSockaddr) {
                adapter.dns.push(ip.to_string());
            }
            dns = d.Next;
        }
        let mac = &a.PhysicalAddress[..(a.PhysicalAddressLength as usize).min(8)];
        if mac.len() == 6 {
            adapter.mac = Some(mac.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join("-"));
        }
        result.push(adapter);
    }
    result
}

fn socket_ip(addr: *const SOCKADDR) -> Option<IpAddr> {
    // SAFETY: the API hands out valid socket addresses of the family they declare.
    let family = unsafe { addr.as_ref() }?.sa_family;
    if family == AF_INET {
        let v4 = unsafe { &*(addr as *const SOCKADDR_IN) };
        Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(unsafe { v4.sin_addr.S_un.S_addr }))))
    } else if family == AF_INET6 {
        let v6 = unsafe { &*(addr as *const SOCKADDR_IN6) };
        Some(IpAddr::V6(Ipv6Addr::from(unsafe { v6.sin6_addr.u.Byte })))
    } else {
        None
    }
}

pub struct WifiLink {
    /// Interface description, matching [`Adapter::description`].
    pub description: String,
    pub ssid: String,
    /// 0–100, as Windows shows it in the network flyout.
    pub signal: u32,
    pub rx_mbps: f64,
    pub tx_mbps: f64,
    pub standard: Option<&'static str>,
}

/// Open WLAN client handle; `None` when the WLAN service is not running.
pub struct Wifi(HANDLE);

impl Wifi {
    pub fn open() -> Option<Self> {
        let mut version = 0u32;
        let mut handle = HANDLE::default();
        let status = unsafe { WlanOpenHandle(2, None, &mut version, &mut handle) };
        (status == ERROR_SUCCESS.0).then_some(Self(handle))
    }

    pub fn links(&self) -> Vec<WifiLink> {
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if unsafe { WlanEnumInterfaces(self.0, None, &mut list) } != ERROR_SUCCESS.0 || list.is_null() {
            return Vec::new();
        }
        let mut links = Vec::new();
        // SAFETY: the list holds `dwNumberOfItems` entries and is freed below.
        let interfaces =
            unsafe { std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), (*list).dwNumberOfItems as usize) };
        for interface in interfaces.iter().filter(|i| i.isState == wlan_interface_state_connected) {
            let mut size = 0u32;
            let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
            let status = unsafe {
                WlanQueryInterface(
                    self.0,
                    &interface.InterfaceGuid,
                    wlan_intf_opcode_current_connection,
                    None,
                    &mut size,
                    &mut data,
                    None,
                )
            };
            if status != ERROR_SUCCESS.0 || data.is_null() {
                continue;
            }
            let c = unsafe { &*(data as *const WLAN_CONNECTION_ATTRIBUTES) };
            let a = &c.wlanAssociationAttributes;
            let ssid = &a.dot11Ssid.ucSSID[..(a.dot11Ssid.uSSIDLength as usize).min(32)];
            links.push(WifiLink {
                description: String::from_utf16_lossy(&interface.strInterfaceDescription)
                    .trim_end_matches('\0')
                    .to_string(),
                ssid: String::from_utf8_lossy(ssid).into_owned(),
                signal: a.wlanSignalQuality,
                rx_mbps: a.ulRxRate as f64 / 1000.0,
                tx_mbps: a.ulTxRate as f64 / 1000.0,
                standard: match a.dot11PhyType.0 {
                    4 => Some("Wi-Fi 1 (802.11a)"),
                    6 => Some("Wi-Fi 3 (802.11g)"),
                    7 => Some("Wi-Fi 4 (802.11n)"),
                    8 => Some("Wi-Fi 5 (802.11ac)"),
                    10 => Some("Wi-Fi 6 (802.11ax)"),
                    11 => Some("Wi-Fi 7 (802.11be)"),
                    _ => None,
                },
            });
            unsafe { WlanFreeMemory(data) };
        }
        unsafe { WlanFreeMemory(list.cast()) };
        links
    }
}

impl Drop for Wifi {
    fn drop(&mut self) {
        unsafe { WlanCloseHandle(self.0, None) };
    }
}
