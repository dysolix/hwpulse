//! Signed-in users, as on Task Manager's Users tab: one Remote Desktop Services
//! session each (fast user switching and RDP both make sessions). Disconnecting or
//! signing out someone else needs admin.

use windows::Win32::Foundation::ERROR_ACCESS_DENIED;
use windows::Win32::System::RemoteDesktop::{
    ProcessIdToSessionId, WTS_CONNECTSTATE_CLASS, WTS_CURRENT_SERVER_HANDLE, WTS_INFO_CLASS, WTS_SESSION_INFOW,
    WTSActive, WTSConnected, WTSDisconnectSession, WTSDisconnected, WTSEnumerateSessionsW, WTSFreeMemory,
    WTSLogoffSession, WTSQuerySessionInformationW, WTSUserName,
};
use windows::core::PWSTR;

use crate::tasks::Error;

#[derive(Clone, Debug)]
pub struct Session {
    pub id: u32,
    pub user: String,
    pub status: Status,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Active,
    Disconnected,
    Other,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Active => "Active",
            Status::Disconnected => "Disconnected",
            Status::Other => "",
        }
    }

    fn from(state: WTS_CONNECTSTATE_CLASS) -> Self {
        match state {
            s if s == WTSActive || s == WTSConnected => Status::Active,
            s if s == WTSDisconnected => Status::Disconnected,
            _ => Status::Other,
        }
    }
}

/// Sessions with a signed-in user (not the services session or RDP listeners).
pub fn sessions() -> Vec<Session> {
    let mut list: *mut WTS_SESSION_INFOW = std::ptr::null_mut();
    let mut count = 0u32;
    if unsafe { WTSEnumerateSessionsW(Some(WTS_CURRENT_SERVER_HANDLE), 0, 1, &mut list, &mut count) }.is_err() {
        return Vec::new();
    }
    let infos = unsafe { std::slice::from_raw_parts(list, count as usize) };
    let sessions = infos
        .iter()
        .filter_map(|info| {
            let user = query(info.SessionId, WTSUserName)?;
            (!user.is_empty()).then(|| Session { id: info.SessionId, user, status: Status::from(info.State) })
        })
        .collect();
    unsafe { WTSFreeMemory(list.cast()) };
    sessions
}

fn query(session: u32, class: WTS_INFO_CLASS) -> Option<String> {
    let mut buffer = PWSTR::null();
    let mut bytes = 0u32;
    unsafe { WTSQuerySessionInformationW(Some(WTS_CURRENT_SERVER_HANDLE), session, class, &mut buffer, &mut bytes) }
        .ok()?;
    let text = unsafe { buffer.to_string() }.ok();
    unsafe { WTSFreeMemory(buffer.0.cast()) };
    text
}

/// The session hwmon runs in.
pub fn current() -> u32 {
    let mut session = 0;
    let _ = unsafe { ProcessIdToSessionId(std::process::id(), &mut session) };
    session
}

/// Leaves the session running but shows the sign-in screen (or ends the remote connection).
pub fn disconnect(session: u32) -> Result<(), Error> {
    unsafe { WTSDisconnectSession(Some(WTS_CURRENT_SERVER_HANDLE), session, false) }.map_err(error)
}

/// Signs the user out; unsaved work in their apps is lost.
pub fn sign_out(session: u32) -> Result<(), Error> {
    unsafe { WTSLogoffSession(Some(WTS_CURRENT_SERVER_HANDLE), session, false) }.map_err(error)
}

fn error(e: windows::core::Error) -> Error {
    if e.code() == ERROR_ACCESS_DENIED.to_hresult() { Error::AccessDenied } else { Error::Other(e.message()) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn own_session_is_listed() {
        let own = super::current();
        let sessions = super::sessions();
        // CI runs tests as a service in session 0, which has no user to list.
        if own != 0 {
            assert!(sessions.iter().any(|s| s.id == own && s.status == super::Status::Active));
        }
    }
}
