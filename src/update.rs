//! Updates from GitHub Releases. Asks which release is the latest, and on the user's
//! click downloads it, checks its minisign signature and installs it.
//!
//! The signature is what makes this safe: its trusted comment names the version and
//! file, so neither GitHub, a proxy nor a tampered release can swap in other bytes, an
//! older version or the other download. Without a valid signature nothing runs.

use std::ffi::{OsString, c_void};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use minisign_verify::{PublicKey, Signature};
use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_URL, WINHTTP_QUERY_FLAG_NUMBER,
    WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryHeaders, WinHttpQueryOption, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetTimeouts,
};
use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;
use windows::core::{HSTRING, PCWSTR, w};

use crate::tasks;

const REPO: &str = "/dysolix/hwpulse";
const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Signed files are at most this big; anything larger is not a release.
const MAX_DOWNLOAD: usize = 64 * 1024 * 1024;
/// Not right at startup, which stays free of network waits.
const FIRST_CHECK: Duration = Duration::from_secs(10);
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// Folders in the temp directory that hold a downloaded setup.
const TEMP_PREFIX: &str = "hwpulse-update-";
/// Keys a release may be signed with.
const TRUSTED_KEYS: &[&str] = &[include_str!("../installer/hwpulse.pub")];

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Unknown,
    Checking,
    UpToDate,
    Available(String),
    Installing(String),
    Failed(String),
}

pub struct Updater {
    status: Arc<Mutex<Status>>,
    /// Set once setup runs or the exe was replaced; the window then closes.
    quit: Arc<AtomicBool>,
    started: Instant,
    last_check: Option<Instant>,
}

impl Updater {
    pub fn new() -> Self {
        Self {
            status: Arc::new(Mutex::new(Status::Unknown)),
            quit: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
            last_check: None,
        }
    }

    pub fn status(&self) -> Status {
        self.status.lock().expect("no panics while locked").clone()
    }

    /// Once per frame: closes the window for an install and starts automatic checks.
    /// Without frames (paused and in the background) a check waits for the next one.
    pub fn tick(&mut self, ctx: &egui::Context, automatic: bool) {
        if self.quit.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let due = match self.last_check {
            Some(last) => last.elapsed() >= CHECK_EVERY,
            None => self.started.elapsed() >= FIRST_CHECK,
        };
        if automatic && due && !matches!(self.status(), Status::Available(_) | Status::Installing(_)) {
            self.check(ctx);
        }
    }

    pub fn check(&mut self, ctx: &egui::Context) {
        self.last_check = Some(Instant::now());
        self.run(ctx, Status::Checking, || {
            latest_version().map(
                |latest| {
                    if is_newer(&latest, CURRENT) { Status::Available(latest) } else { Status::UpToDate }
                },
            )
        });
    }

    pub fn install(&mut self, ctx: &egui::Context, version: String) {
        let quit = self.quit.clone();
        self.run(ctx, Status::Installing(version.clone()), move || {
            install(&version)?;
            quit.store(true, Ordering::Relaxed);
            Ok(Status::Installing(version))
        });
    }

    /// Runs `work` on its own thread, with `busy` as the status until it is done.
    fn run(&self, ctx: &egui::Context, busy: Status, work: impl FnOnce() -> Result<Status, String> + Send + 'static) {
        {
            let mut status = self.status.lock().expect("no panics while locked");
            if matches!(*status, Status::Checking | Status::Installing(_)) {
                return;
            }
            *status = busy;
        }
        let (status, ctx) = (self.status.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = work().unwrap_or_else(Status::Failed);
            *status.lock().expect("no panics while locked") = result;
            ctx.request_repaint();
        });
    }
}

/// The release page of `version`, for "What's new".
pub fn release_url(version: &str) -> String {
    format!("https://github.com{REPO}/releases/tag/v{version}")
}

/// Removes what the previous update left behind, on its own thread: the replaced exe of
/// a portable copy (still running for a moment) and downloaded setups.
pub fn clean_up() {
    std::thread::spawn(|| {
        if let Ok(exe) = std::env::current_exe() {
            let old = sibling(&exe, "old");
            for _ in 0..10 {
                if !old.exists() || std::fs::remove_file(&old).is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        let temp = std::env::temp_dir();
        for entry in std::fs::read_dir(temp).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Checking

/// `/releases/latest` redirects to the tag of the latest release, which spares parsing
/// the JSON of the API and its rate limit. Drafts and pre-releases don't count.
fn latest_version() -> Result<String, String> {
    let response = request(w!("HEAD"), &format!("{REPO}/releases/latest"))?;
    match response.status {
        200 => version_from_url(&response.url).ok_or_else(|| "No published release found on GitHub.".to_owned()),
        // Also what a private repository answers.
        404 => Err("No public release found on GitHub.".to_owned()),
        status => Err(format!("GitHub answered with status {status}.")),
    }
}

fn version_from_url(url: &str) -> Option<String> {
    let (_, tag) = url.rsplit_once("/releases/tag/")?;
    let version = tag.strip_prefix('v')?;
    parse_version(version).map(|_| version.to_owned())
}

fn parse_version(version: &str) -> Option<[u32; 3]> {
    let mut parts = version.split('.').map(|p| p.parse::<u32>().ok());
    let parsed = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(parsed)
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Installing

fn install(version: &str) -> Result<(), String> {
    // Files from the user's temp folder must not run with more rights than the user has.
    if tasks::is_elevated() {
        return Err("Restart hwpulse without administrator rights to install updates.".to_owned());
    }
    if !is_newer(version, CURRENT) {
        return Err(format!("{version} is not newer than this version."));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let installed = exe.with_file_name("unins000.exe").exists();
    let file =
        if installed { format!("hwpulse-setup-{version}.exe") } else { format!("hwpulse-{version}-portable.exe") };
    let data = download_verified(version, &file)?;
    if installed {
        run_setup(&data, &file).map_err(|e| format!("Could not start setup: {e}"))
    } else {
        replace_exe(&exe, &data).map_err(|e| format!("Could not replace {}: {e}", exe.display()))?;
        std::process::Command::new(&exe).spawn().map(drop).map_err(|e| format!("Could not start the new version: {e}"))
    }
}

/// The file as verified bytes in memory, so nothing can change it between the check
/// and writing it out.
fn download_verified(version: &str, file: &str) -> Result<Vec<u8>, String> {
    let path = format!("{REPO}/releases/download/v{version}/{file}");
    let get = |path: &str| match request(w!("GET"), path)? {
        response if response.status == 200 => Ok(response.body),
        response => Err(format!("GitHub answered with status {}.", response.status)),
    };
    let data = get(&path)?;
    let signature = get(&format!("{path}.minisig"))?;
    let signature = String::from_utf8(signature).map_err(|_| "The signature file is damaged.".to_owned())?;
    verify(TRUSTED_KEYS, &data, &signature, &format!("hwpulse {version} {file}"))?;
    Ok(data)
}

fn verify(keys: &[&str], data: &[u8], signature: &str, comment: &str) -> Result<(), String> {
    let refused = || "The download has no valid signature and was not installed.".to_owned();
    let signature = Signature::decode(signature).map_err(|_| refused())?;
    // Legacy signatures are allowed because they are just as binding; which of the two
    // formats a release has only depends on the minisign version on the build machine.
    let signed =
        keys.iter().any(|key| PublicKey::decode(key).is_ok_and(|key| key.verify(data, &signature, true).is_ok()));
    if signed && signature.trusted_comment() == comment { Ok(()) } else { Err(refused()) }
}

/// Runs a downloaded setup, which updates the install in place and in its mode (asking
/// for administrator rights for an all-users install), then starts the new version.
fn run_setup(data: &[u8], file: &str) -> std::io::Result<()> {
    let nanos = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("{TEMP_PREFIX}{}-{nanos}", std::process::id()));
    std::fs::create_dir(&dir)?;
    let path = dir.join(file);
    std::fs::File::create_new(&path)?.write_all(data)?;
    // Held until setup runs, so that nothing can replace the file in between.
    let _lock = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ.0).open(&path)?;
    std::process::Command::new(&path).args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/RELAUNCH=1"]).spawn()?;
    Ok(())
}

/// Puts `data` in place of the exe. Windows lets a running exe be renamed but not
/// overwritten; [`clean_up`] removes the old one on the next start.
fn replace_exe(exe: &Path, data: &[u8]) -> std::io::Result<()> {
    let (new, old) = (sibling(exe, "new"), sibling(exe, "old"));
    let _ = std::fs::remove_file(&new);
    let _ = std::fs::remove_file(&old);
    std::fs::write(&new, data)?;
    let result = std::fs::rename(exe, &old).and_then(|()| {
        std::fs::rename(&new, exe).inspect_err(|_| {
            let _ = std::fs::rename(&old, exe);
        })
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&new);
    }
    result
}

/// `C:\x\hwpulse.exe` → `C:\x\hwpulse.exe.old`.
fn sibling(exe: &Path, extension: &str) -> PathBuf {
    let mut name = OsString::from(exe);
    name.push(".");
    name.push(extension);
    PathBuf::from(name)
}

// ---------------------------------------------------------------------------
// HTTPS through WinHTTP, part of Windows: no TLS library in the exe, and it follows the
// system's proxy and certificates. Redirects are followed, but never from HTTPS to HTTP.

struct Response {
    status: u32,
    /// After redirects.
    url: String,
    body: Vec<u8>,
}

struct Internet(*mut c_void);

impl Internet {
    fn new(handle: *mut c_void) -> Result<Self, String> {
        if handle.is_null() { Err(windows::core::Error::from_thread().message()) } else { Ok(Self(handle)) }
    }
}

impl Drop for Internet {
    fn drop(&mut self) {
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn request(verb: PCWSTR, path: &str) -> Result<Response, String> {
    request_inner(verb, path).map_err(|e| format!("Could not reach GitHub: {}", e.trim()))
}

fn request_inner(verb: PCWSTR, path: &str) -> Result<Response, String> {
    let message = |e: windows::core::Error| e.message();
    let session = Internet::new(unsafe {
        WinHttpOpen(w!("hwpulse"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0)
    })?;
    unsafe { WinHttpSetTimeouts(session.0, 10_000, 10_000, 30_000, 30_000) }.map_err(message)?;
    let connection = Internet::new(unsafe { WinHttpConnect(session.0, w!("github.com"), 443, 0) })?;
    let request = Internet::new(unsafe {
        WinHttpOpenRequest(
            connection.0,
            verb,
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        )
    })?;
    unsafe {
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(message)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(message)?;
    }

    let (mut status, mut len) = (0u32, size_of::<u32>() as u32);
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut len,
            std::ptr::null_mut(),
        )
    }
    .map_err(message)?;

    let mut url = [0u16; 2048];
    let mut len = size_of_val(&url) as u32;
    unsafe { WinHttpQueryOption(request.0, WINHTTP_OPTION_URL, Some(url.as_mut_ptr().cast()), &mut len) }
        .map_err(message)?;
    let url = String::from_utf16_lossy(&url[..len as usize / 2]).trim_end_matches('\0').to_owned();

    let mut body = Vec::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let mut read = 0;
        unsafe { WinHttpReadData(request.0, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read) }
            .map_err(message)?;
        if read == 0 {
            break;
        }
        if body.len() + read as usize > MAX_DOWNLOAD {
            return Err("the download is larger than any release.".to_owned());
        }
        body.extend_from_slice(&buffer[..read as usize]);
    }
    Ok(Response { status, url, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Throwaway keys, made only for these tests.
    const KEY: &str = "untrusted comment: minisign public key CBA76B7BCF714B96\n\
                       RWSWS3HPe2uny2HaQNNIFQH/9bS8sXjlkLDhknrnGAZzwpUXYGMRZk2J\n";
    const OTHER_KEY: &str = "untrusted comment: minisign public key 80FFCE2257F789A6\n\
                             RWSmifdXIs7/gGBYGfLQ4Me54/RMr09kfJd62U8/JHSz+TsxVFY2rymO\n";
    const DATA: &[u8] = b"hwpulse test file";
    const SIGNATURE: &str = "untrusted comment: signature from minisign secret key\n\
        RUSWS3HPe2uny4kP9Li89nUXIkbQDwzvwSQVy2+USFCdVj/9vl0gep+xYo21APgGtCU1qyD9jbJfHXfpDnniWmK+u/9EfkRvCQg=\n\
        trusted comment: hwpulse 9.9.9 f\n\
        FnBOUMHilIUYd8n40e7JdrCQK/8IoTfRB3S7AuypFObqzXOE+aIhDm3TonG6OXxv2t6R31CK6xQXEasXM7D9BA==\n";
    const COMMENT: &str = "hwpulse 9.9.9 f";

    #[test]
    fn accepts_only_the_signed_file_version_and_key() {
        assert!(verify(&[KEY], DATA, SIGNATURE, COMMENT).is_ok());
        assert!(verify(&[OTHER_KEY, KEY], DATA, SIGNATURE, COMMENT).is_ok());
        assert!(verify(&[KEY], b"hwpulse test filE", SIGNATURE, COMMENT).is_err());
        assert!(verify(&[OTHER_KEY], DATA, SIGNATURE, COMMENT).is_err());
        assert!(verify(&[], DATA, SIGNATURE, COMMENT).is_err());
        assert!(verify(&[KEY], DATA, SIGNATURE, "hwpulse 9.9.10 f").is_err());
        assert!(verify(&[KEY], DATA, SIGNATURE, "hwpulse 9.9.9 g").is_err());
        assert!(verify(&[KEY], DATA, "", COMMENT).is_err());
    }

    #[test]
    fn a_changed_trusted_comment_breaks_the_signature() {
        let relabeled = SIGNATURE.replace("hwpulse 9.9.9 f", "hwpulse 9.9.10 f");
        assert!(verify(&[KEY], DATA, &relabeled, "hwpulse 9.9.10 f").is_err());
    }

    #[test]
    fn the_release_key_can_be_read() {
        assert!(TRUSTED_KEYS.iter().all(|key| PublicKey::decode(key).is_ok()));
        assert!(verify(&[&KEY.replace('\n', "\r\n")], DATA, SIGNATURE, COMMENT).is_ok());
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.0.2", "0.0.1"));
        assert!(is_newer("0.0.10", "0.0.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.0.1", "0.0.1"));
        assert!(!is_newer("0.0.1", "0.0.2"));
        assert!(!is_newer("0.0.2-beta", "0.0.1"));
        assert!(!is_newer("garbage", "0.0.1"));
        assert!(!is_newer("0.0.2.1", "0.0.1"));
    }

    #[test]
    fn reads_the_version_from_the_redirect() {
        let url = "https://github.com/dysolix/hwpulse/releases/tag/v1.2.3";
        assert_eq!(version_from_url(url).as_deref(), Some("1.2.3"));
        assert_eq!(version_from_url("https://github.com/dysolix/hwpulse/releases"), None);
        assert_eq!(version_from_url("https://github.com/dysolix/hwpulse/releases/tag/nightly"), None);
    }

    #[test]
    fn replaces_an_exe_and_keeps_the_old_one_until_cleanup() {
        let dir = std::env::temp_dir().join(format!("hwpulse-test-swap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("copy.exe");
        std::fs::write(&exe, b"old").unwrap();
        replace_exe(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(sibling(&exe, "old")).unwrap(), b"old");
        assert!(!sibling(&exe, "new").exists());
        // A second update while the first old exe is still around.
        replace_exe(&exe, b"newer").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"newer");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Talks to GitHub: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn follows_the_redirect_of_a_public_repository() {
        let response = request(w!("HEAD"), "/sharkdp/bat/releases/latest").unwrap();
        assert_eq!(response.status, 200);
        assert!(version_from_url(&response.url).is_some(), "{}", response.url);
        let response = request(w!("GET"), "/sharkdp/bat/releases/latest").unwrap();
        assert_eq!(response.status, 200);
        assert!(!response.body.is_empty());
    }

    /// Talks to GitHub. While the repository is private, this is what users would see.
    #[test]
    #[ignore]
    fn reports_the_state_of_this_repository() {
        println!("{:?}", latest_version());
    }
}
