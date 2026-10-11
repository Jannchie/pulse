//! 登录会话（用户）：枚举、断开连接、注销。

use windows::Win32::System::RemoteDesktop::{
    ProcessIdToSessionId, WTS_CONNECTSTATE_CLASS, WTS_INFO_CLASS, WTS_SESSION_INFOW, WTSActive,
    WTSDisconnectSession, WTSDisconnected, WTSDomainName, WTSEnumerateSessionsW, WTSFreeMemory,
    WTSLogoffSession, WTSQuerySessionInformationW, WTSUserName,
};
use windows::core::PWSTR;

#[derive(Clone, Debug)]
pub struct Session {
    pub id: u32,
    pub user: String,
    pub domain: String,
    /// WinStation 名，如 Console、RDP-Tcp#0。
    pub station: String,
    pub state: SessionState,
    /// Pulse 自己所在的会话。
    pub current: bool,
}

/// 有用户登录的会话（不含 Services 会话 0 等无用户的会话）。
pub fn list() -> Vec<Session> {
    let mut current = 0u32;
    unsafe {
        let _ = ProcessIdToSessionId(std::process::id(), &mut current);
    }
    let mut ptr: *mut WTS_SESSION_INFOW = std::ptr::null_mut();
    let mut count = 0u32;
    if unsafe { WTSEnumerateSessionsW(None, 0, 1, &mut ptr, &mut count) }.is_err() {
        return Vec::new();
    }
    let raw = unsafe { std::slice::from_raw_parts(ptr, count as usize) };
    let out = raw
        .iter()
        .filter_map(|s| {
            let user = query(s.SessionId, WTSUserName);
            if user.is_empty() {
                return None;
            }
            Some(Session {
                id: s.SessionId,
                user,
                domain: query(s.SessionId, WTSDomainName),
                station: unsafe { s.pWinStationName.to_string() }.unwrap_or_default(),
                state: state(s.State),
                current: s.SessionId == current,
            })
        })
        .collect();
    unsafe { WTSFreeMemory(ptr.cast()) };
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SessionState {
    Active,
    Disconnected,
    Other,
}

impl SessionState {
    pub fn label(self) -> &'static str {
        match self {
            SessionState::Active => "活动",
            SessionState::Disconnected => "已断开",
            SessionState::Other => "其他",
        }
    }
}

fn state(s: WTS_CONNECTSTATE_CLASS) -> SessionState {
    match s {
        x if x == WTSActive => SessionState::Active,
        x if x == WTSDisconnected => SessionState::Disconnected,
        _ => SessionState::Other,
    }
}

fn query(session: u32, class: WTS_INFO_CLASS) -> String {
    let mut buf = PWSTR::null();
    let mut len = 0u32;
    if unsafe { WTSQuerySessionInformationW(None, session, class, &mut buf, &mut len) }.is_err()
        || buf.is_null()
    {
        return String::new();
    }
    let s = unsafe { buf.to_string() }.unwrap_or_default();
    unsafe { WTSFreeMemory(buf.0.cast()) };
    s
}

pub fn disconnect(session: u32) -> windows::core::Result<()> {
    unsafe { WTSDisconnectSession(None, session, false) }
}

pub fn logoff(session: u32) -> windows::core::Result<()> {
    unsafe { WTSLogoffSession(None, session, false) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn current_session_listed() {
        let sessions = super::list();
        assert!(sessions.iter().any(|s| s.current), "{sessions:?}");
    }
}
