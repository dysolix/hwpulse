//! A summary of the PC's hardware for the clipboard, e.g. for a support request or a
//! forum post: mainboard, Windows version and every component with its fixed facts,
//! as plain text, Markdown or JSON. Live values and anything that identifies the
//! machine or its network (addresses, network name) are left out.

use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::core::HSTRING;

use crate::sensors::Group;

/// Facts that change while running or identify the machine or network.
const LEFT_OUT: [&str; 15] = [
    "Up time",
    "Processes",
    "Threads",
    "Handles",
    "Time left",
    "Power source",
    "Performance state",
    "Clocks limited by",
    "Link rate",
    "Network",
    "IPv4",
    "IPv6",
    "DNS",
    "Gateway",
    "MAC address",
];

pub struct Report {
    pub mainboard: Option<String>,
    pub windows: Option<String>,
    pub components: Vec<Component>,
}

pub struct Component {
    /// E.g. `"Processor"`.
    pub kind: &'static str,
    pub name: String,
    pub facts: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    Markdown,
    Json,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Text, Format::Markdown, Format::Json];

    pub fn label(self) -> &'static str {
        match self {
            Format::Text => "Plain text",
            Format::Markdown => "Markdown",
            Format::Json => "JSON",
        }
    }
}

impl Report {
    pub fn new(mainboard: Option<&str>, groups: &[Group]) -> Self {
        let components = groups
            .iter()
            .map(|g| Component {
                kind: g.kind.label(),
                name: g.title.clone(),
                facts: g
                    .info
                    .iter()
                    // Volumes show free space, which changes.
                    .filter(|f| !LEFT_OUT.contains(&f.label.as_str()) && !f.label.starts_with("Volume "))
                    .map(|f| match f.label.as_str() {
                        "PCIe link" => (f.label.clone(), pcie_link(&f.value)),
                        _ => (f.label.clone(), f.value.clone()),
                    })
                    .collect(),
            })
            .collect();
        Self { mainboard: mainboard.map(str::to_owned), windows: windows_version(), components }
    }

    pub fn format(&self, format: Format) -> String {
        match format {
            Format::Text => self.text(),
            Format::Markdown => self.markdown(),
            Format::Json => self.json(),
        }
    }

    fn system(&self) -> Vec<(&str, &str)> {
        [("Mainboard", &self.mainboard), ("Windows", &self.windows)]
            .into_iter()
            .filter_map(|(label, value)| Some((label, value.as_deref()?)))
            .collect()
    }

    fn text(&self) -> String {
        let mut out = String::new();
        for (label, value) in self.system() {
            out += &format!("{label}: {value}\n");
        }
        for c in &self.components {
            out += &format!("\n{}: {}\n", c.kind, c.name);
            for (label, value) in &c.facts {
                out += &format!("  {label}: {value}\n");
            }
        }
        out
    }

    fn markdown(&self) -> String {
        let mut out = String::from("## Hardware\n\n");
        for (label, value) in self.system() {
            out += &format!("- **{label}:** {value}\n");
        }
        for c in &self.components {
            out += &format!("\n### {}: {}\n\n", c.kind, c.name);
            for (label, value) in &c.facts {
                out += &format!("- **{label}:** {value}\n");
            }
        }
        out
    }

    fn json(&self) -> String {
        let mut out = String::from("{\n");
        for (label, value) in self.system() {
            out += &format!("  {}: {},\n", string(&label.to_lowercase()), string(value));
        }
        out += "  \"components\": [";
        for (i, c) in self.components.iter().enumerate() {
            out += if i == 0 { "\n" } else { ",\n" };
            out += &format!(
                "    {{\n      \"type\": {},\n      \"name\": {},\n      \"details\": {{",
                string(c.kind),
                string(&c.name)
            );
            for (j, (label, value)) in c.facts.iter().enumerate() {
                out += if j == 0 { "\n" } else { ",\n" };
                out += &format!("        {}: {}", string(label), string(value));
            }
            out += if c.facts.is_empty() { "}\n    }" } else { "\n      }\n    }" };
        }
        out += if self.components.is_empty() { "]\n}\n" } else { "\n  ]\n}\n" };
        out
    }
}

/// `Gen 1 × 8 (max Gen 4 × 16)` → `× 8 lanes (card: Gen 4 × 16)`. The generation drops
/// while the GPU idles, so a copy would differ from one moment to the next; the lane
/// count stays and shows a card running on fewer lanes than it supports.
fn pcie_link(value: &str) -> String {
    let lanes = |s: &str| s.split('×').nth(1).map(|w| w.trim().to_owned());
    let (now, max) = match value.split_once(" (max ") {
        Some((now, max)) => (now, max.trim_end_matches(')')),
        None => (value, value),
    };
    match lanes(now) {
        Some(width) => format!("× {width} lanes (card: {max})"),
        None => value.to_owned(),
    }
}

/// A JSON string literal.
fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            '\r' => out += "\\r",
            '\t' => out += "\\t",
            c if (c as u32) < 0x20 => out += &format!("\\u{:04x}", c as u32),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// E.g. "Windows 11 Home 25H2 (build 26200)".
fn windows_version() -> Option<String> {
    let product = registry("ProductName")?;
    let build: u32 = registry("CurrentBuildNumber").and_then(|b| b.parse().ok()).unwrap_or(0);
    // Windows 11 still calls itself "Windows 10" here; build 22000 was its first.
    let product = if build >= 22000 { product.replacen("Windows 10", "Windows 11", 1) } else { product };
    Some(match registry("DisplayVersion") {
        Some(version) => format!("{product} {version} (build {build})"),
        None => format!("{product} (build {build})"),
    })
}

fn registry(value: &str) -> Option<String> {
    let mut buffer = [0u16; 256];
    let mut size = (buffer.len() * 2) as u32;
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &HSTRING::from(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
            &HSTRING::from(value),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    }
    .ok()
    .ok()?;
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end])).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Report {
        Report {
            mainboard: Some("ASUS B650".into()),
            windows: Some("Windows 11 Home".into()),
            components: vec![
                Component {
                    kind: "Processor",
                    name: "Ryzen \"X3D\"".into(),
                    facts: vec![("Cores".into(), "8".into()), ("Path".into(), r"C:\x".into())],
                },
                Component { kind: "Battery", name: "Battery".into(), facts: Vec::new() },
            ],
        }
    }

    #[test]
    fn text_and_markdown_list_every_fact() {
        let r = sample();
        assert!(r.text().contains("Processor: Ryzen \"X3D\"\n  Cores: 8\n"));
        assert!(r.markdown().contains("### Processor: Ryzen \"X3D\"\n\n- **Cores:** 8\n"));
    }

    #[test]
    fn json_is_escaped_and_well_formed() {
        let json = sample().json();
        assert!(json.contains(r#""name": "Ryzen \"X3D\"""#));
        assert!(json.contains(r#""Path": "C:\\x""#));
        assert!(json.contains(r#""details": {}"#));
        // Balanced brackets and no trailing commas.
        assert_eq!(json.matches('{').count(), json.matches('}').count());
        assert!(!json.contains(",\n  ]") && !json.contains(",\n      }"));
    }

    #[test]
    fn pcie_link_leaves_out_the_idle_generation() {
        assert_eq!(pcie_link("Gen 1 × 8 (max Gen 4 × 16)"), "× 8 lanes (card: Gen 4 × 16)");
        assert_eq!(pcie_link("Gen 4 × 8 (max Gen 4 × 16)"), "× 8 lanes (card: Gen 4 × 16)");
        assert_eq!(pcie_link("Gen 4 × 16"), "× 16 lanes (card: Gen 4 × 16)");
    }

    #[test]
    fn reads_the_windows_version() {
        assert!(windows_version().is_some_and(|v| v.starts_with("Windows")));
    }
}
