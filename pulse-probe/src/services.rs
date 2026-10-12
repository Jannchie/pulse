//! Windows 服务：枚举状态、启动类型、描述与所在进程；启动 / 停止 / 重启 / 修改启动类型。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::wide;
use windows::Win32::System::Services::{
    ChangeServiceConfig2W, ChangeServiceConfigW, CloseServiceHandle, ControlService,
    ENUM_SERVICE_STATUS_PROCESSW, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW,
    QUERY_SERVICE_CONFIGW, QueryServiceConfig2W, QueryServiceConfigW, QueryServiceStatusEx,
    SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_CONNECT, SC_MANAGER_ENUMERATE_SERVICE,
    SC_STATUS_PROCESS_INFO, SERVICE_ACCEPT_STOP, SERVICE_AUTO_START, SERVICE_BOOT_START,
    SERVICE_CHANGE_CONFIG, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, SERVICE_CONFIG_DESCRIPTION,
    SERVICE_CONTINUE_PENDING, SERVICE_CONTROL_STOP, SERVICE_DELAYED_AUTO_START_INFO,
    SERVICE_DEMAND_START, SERVICE_DESCRIPTIONW, SERVICE_DISABLED, SERVICE_NO_CHANGE,
    SERVICE_PAUSE_PENDING, SERVICE_PAUSED, SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS,
    SERVICE_RUNNING, SERVICE_START, SERVICE_START_PENDING, SERVICE_STATE_ALL, SERVICE_STATUS,
    SERVICE_STATUS_PROCESS, SERVICE_STOP, SERVICE_STOP_PENDING, SERVICE_STOPPED,
    SERVICE_SYSTEM_START, SERVICE_WIN32, StartServiceW,
};
use windows::core::{PCWSTR, PWSTR};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Stopped,
    Starting,
    Stopping,
    Running,
    Paused,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Stopped => crate::t!("已停止", "Stopped"),
            State::Starting => crate::t!("正在启动", "Starting"),
            State::Stopping => crate::t!("正在停止", "Stopping"),
            State::Running => crate::t!("正在运行", "Running"),
            State::Paused => crate::t!("已暂停", "Paused"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StartType {
    Automatic,
    /// 自动（延迟启动）
    Delayed,
    Manual,
    Disabled,
    /// 引导 / 系统（驱动级，通常不该改）
    Boot,
}

impl StartType {
    pub const EDITABLE: [StartType; 4] = [
        StartType::Automatic,
        StartType::Delayed,
        StartType::Manual,
        StartType::Disabled,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StartType::Automatic => crate::t!("自动", "Automatic"),
            StartType::Delayed => crate::t!("自动（延迟启动）", "Automatic (delayed start)"),
            StartType::Manual => crate::t!("手动", "Manual"),
            StartType::Disabled => crate::t!("禁用", "Disabled"),
            StartType::Boot => crate::t!("引导", "Boot"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Service {
    pub name: Arc<str>,
    pub display_name: Arc<str>,
    pub description: Option<Arc<str>>,
    pub state: State,
    /// 运行中服务所在的进程，停止时为 0。
    pub pid: u32,
    pub start_type: StartType,
    /// svchost 的服务组（`-k netsvcs`），其他服务为 None。
    pub group: Option<Arc<str>>,
    pub binary: Arc<str>,
    pub account: Arc<str>,
    pub can_stop: bool,
}

#[derive(Clone)]
struct Config {
    description: Option<Arc<str>>,
    start_type: StartType,
    group: Option<Arc<str>>,
    binary: Arc<str>,
    account: Arc<str>,
}

/// 状态每 `INTERVAL` 刷新一次；配置（启动类型、描述）按服务名缓存，修改后由调用方失效。
pub struct ServiceMonitor {
    configs: HashMap<Arc<str>, Config>,
    last: Arc<Vec<Service>>,
    at: Option<Instant>,
    interval: Duration,
}

impl Default for ServiceMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceMonitor {
    pub fn new() -> Self {
        Self {
            configs: HashMap::new(),
            last: Arc::new(Vec::new()),
            at: None,
            interval: Duration::from_secs(3),
        }
    }

    /// 服务页可见时频繁刷新，其余时候只为悬停提示保留一个慢速的 pid → 服务映射。
    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    /// 让下一次 `sample` 立即刷新，并重新读取某个服务的配置。
    pub fn invalidate(&mut self, name: Option<&str>) {
        self.at = None;
        if let Some(name) = name {
            self.configs.retain(|k, _| k.as_ref() != name);
        }
    }

    pub fn sample(&mut self) -> Arc<Vec<Service>> {
        if self.at.is_some_and(|t| t.elapsed() < self.interval) {
            return self.last.clone();
        }
        self.at = Some(Instant::now());
        let Ok(scm) = open_scm(SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE) else {
            return self.last.clone();
        };
        let mut out = Vec::new();
        for (name, display, status) in enumerate(scm.0) {
            let config = self
                .configs
                .entry(name.clone())
                .or_insert_with(|| query_config(scm.0, &name))
                .clone();
            out.push(Service {
                name,
                display_name: display,
                description: config.description,
                state: state(status.dwCurrentState.0),
                pid: status.dwProcessId,
                start_type: config.start_type,
                group: config.group,
                binary: config.binary,
                account: config.account,
                can_stop: status.dwControlsAccepted & SERVICE_ACCEPT_STOP != 0,
            });
        }
        out.sort_by_cached_key(|s| s.name.to_lowercase());
        // 没变化就沿用旧的 Arc：调用方用指针比较判断是否需要重绘、重建映射。
        if *self.last != out {
            self.last = Arc::new(out);
        }
        self.last.clone()
    }
}

fn state(raw: u32) -> State {
    match raw {
        x if x == SERVICE_STOPPED.0 => State::Stopped,
        x if x == SERVICE_START_PENDING.0 || x == SERVICE_CONTINUE_PENDING.0 => State::Starting,
        x if x == SERVICE_STOP_PENDING.0 || x == SERVICE_PAUSE_PENDING.0 => State::Stopping,
        x if x == SERVICE_RUNNING.0 => State::Running,
        x if x == SERVICE_PAUSED.0 => State::Paused,
        _ => State::Stopped,
    }
}

/// SCM 或服务句柄，离开作用域时关闭。
struct ScHandle(SC_HANDLE);

impl Drop for ScHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

fn open_scm(access: u32) -> windows::core::Result<ScHandle> {
    Ok(ScHandle(unsafe {
        OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), access)?
    }))
}

fn text(p: PWSTR) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { p.to_string() }.unwrap_or_default()
    }
}

fn enumerate(scm: SC_HANDLE) -> Vec<(Arc<str>, Arc<str>, SERVICE_STATUS_PROCESS)> {
    let mut needed = 0u32;
    let mut count = 0u32;
    let mut resume = 0u32;
    let mut out = Vec::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let r = unsafe {
            EnumServicesStatusExW(
                scm,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                Some(&mut buf),
                &mut needed,
                &mut count,
                Some(&mut resume),
                PCWSTR::null(),
            )
        };
        let entries = unsafe {
            std::slice::from_raw_parts(
                buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW,
                count as usize,
            )
        };
        for e in entries {
            out.push((
                Arc::from(text(e.lpServiceName)),
                Arc::from(text(e.lpDisplayName)),
                e.ServiceStatusProcess,
            ));
        }
        match r {
            Ok(()) => break,
            // ERROR_MORE_DATA：用 resume 句柄继续取剩下的。
            Err(e) if e.code().0 as u32 == 0x8007_00EA => {
                if needed as usize > buf.len() {
                    buf.resize(needed as usize, 0);
                }
            }
            Err(_) => break,
        }
    }
    out
}

fn query_config(scm: SC_HANDLE, name: &str) -> Config {
    let fallback = Config {
        description: None,
        start_type: StartType::Manual,
        group: None,
        binary: Arc::from(""),
        account: Arc::from(""),
    };
    let w = wide(name);
    let Ok(svc) = (unsafe { OpenServiceW(scm, PCWSTR(w.as_ptr()), SERVICE_QUERY_CONFIG) }) else {
        return fallback;
    };
    let svc = ScHandle(svc);
    let mut buf = vec![0u64; 1024];
    let mut needed = 0u32;
    let ok = unsafe {
        QueryServiceConfigW(
            svc.0,
            Some(buf.as_mut_ptr().cast()),
            (buf.len() * 8) as u32,
            &mut needed,
        )
    }
    .is_ok();
    if !ok {
        return fallback;
    }
    let cfg = unsafe { &*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW) };
    let binary = text(cfg.lpBinaryPathName);
    let delayed = {
        let mut info = [0u8; 16];
        let mut n = 0u32;
        unsafe {
            QueryServiceConfig2W(
                svc.0,
                SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                Some(&mut info),
                &mut n,
            )
        }
        .is_ok()
            && info[0] != 0
    };
    let start_type = match cfg.dwStartType {
        t if t == SERVICE_AUTO_START && delayed => StartType::Delayed,
        t if t == SERVICE_AUTO_START => StartType::Automatic,
        t if t == SERVICE_DEMAND_START => StartType::Manual,
        t if t == SERVICE_DISABLED => StartType::Disabled,
        t if t == SERVICE_BOOT_START || t == SERVICE_SYSTEM_START => StartType::Boot,
        _ => StartType::Manual,
    };
    let account = text(cfg.lpServiceStartName);
    let mut dbuf = vec![0u8; 8192];
    let mut n = 0u32;
    let description =
        unsafe { QueryServiceConfig2W(svc.0, SERVICE_CONFIG_DESCRIPTION, Some(&mut dbuf), &mut n) }
            .is_ok()
            .then(|| {
                text(unsafe { (*(dbuf.as_ptr() as *const SERVICE_DESCRIPTIONW)).lpDescription })
            })
            .filter(|d| !d.is_empty() && !d.starts_with('@'))
            .map(Arc::from);
    Config {
        description,
        start_type,
        group: svchost_group(&binary).map(Arc::from),
        binary: Arc::from(binary),
        account: Arc::from(account),
    }
}

/// `C:\Windows\system32\svchost.exe -k netsvcs -p` → `netsvcs`
pub fn svchost_group(binary: &str) -> Option<String> {
    let lower = binary.to_lowercase();
    if !lower.contains("svchost.exe") {
        return None;
    }
    let mut it = binary.split_whitespace();
    it.by_ref().find(|t| t.eq_ignore_ascii_case("-k"))?;
    it.next().map(str::to_string)
}

fn open_service(name: &str, access: u32) -> windows::core::Result<ScHandle> {
    let scm = open_scm(SC_MANAGER_CONNECT)?;
    let w = wide(name);
    Ok(ScHandle(unsafe {
        OpenServiceW(scm.0, PCWSTR(w.as_ptr()), access)?
    }))
}

fn current_state(svc: &ScHandle) -> Option<u32> {
    let mut buf = [0u8; size_of::<SERVICE_STATUS_PROCESS>()];
    let mut n = 0u32;
    unsafe { QueryServiceStatusEx(svc.0, SC_STATUS_PROCESS_INFO, Some(&mut buf), &mut n) }.ok()?;
    Some(unsafe {
        (*(buf.as_ptr() as *const SERVICE_STATUS_PROCESS))
            .dwCurrentState
            .0
    })
}

/// 等服务到达目标状态，最长 `timeout`。
fn wait_for(svc: &ScHandle, target: u32, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if current_state(svc) == Some(target) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// 启动服务并等它进入运行状态。阻塞，应在后台线程调用。
pub fn start(name: &str) -> Result<(), String> {
    let svc = open_service(name, SERVICE_START | SERVICE_QUERY_STATUS).map_err(|e| e.message())?;
    unsafe { StartServiceW(svc.0, None) }.map_err(|e| e.message())?;
    if wait_for(&svc, SERVICE_RUNNING.0, Duration::from_secs(30)) {
        Ok(())
    } else {
        Err(crate::t!(
            "等待服务启动超时",
            "Timed out waiting for the service to start"
        )
        .into())
    }
}

/// 停止服务并等它停下。阻塞，应在后台线程调用。
pub fn stop(name: &str) -> Result<(), String> {
    let svc = open_service(name, SERVICE_STOP | SERVICE_QUERY_STATUS).map_err(|e| e.message())?;
    let mut status = SERVICE_STATUS::default();
    unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut status) }.map_err(|e| e.message())?;
    if wait_for(&svc, SERVICE_STOPPED.0, Duration::from_secs(30)) {
        Ok(())
    } else {
        Err(crate::t!(
            "等待服务停止超时",
            "Timed out waiting for the service to stop"
        )
        .into())
    }
}

pub fn restart(name: &str) -> Result<(), String> {
    stop(name)?;
    start(name)
}

pub fn set_start_type(name: &str, t: StartType) -> Result<(), String> {
    let svc = open_service(name, SERVICE_CHANGE_CONFIG).map_err(|e| e.message())?;
    let raw = match t {
        StartType::Automatic | StartType::Delayed => SERVICE_AUTO_START,
        StartType::Manual => SERVICE_DEMAND_START,
        StartType::Disabled => SERVICE_DISABLED,
        StartType::Boot => {
            return Err(crate::t!(
                "不支持修改为引导启动",
                "Changing to boot start isn't supported"
            )
            .into());
        }
    };
    unsafe {
        ChangeServiceConfigW(
            svc.0,
            windows::Win32::System::Services::ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
            raw,
            windows::Win32::System::Services::SERVICE_ERROR(SERVICE_NO_CHANGE),
            PCWSTR::null(),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )
    }
    .map_err(|e| e.message())?;
    if matches!(t, StartType::Automatic | StartType::Delayed) {
        let info = SERVICE_DELAYED_AUTO_START_INFO {
            fDelayedAutostart: (t == StartType::Delayed).into(),
        };
        unsafe {
            ChangeServiceConfig2W(
                svc.0,
                SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                Some((&info as *const SERVICE_DELAYED_AUTO_START_INFO).cast()),
            )
        }
        .map_err(|e| e.message())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svchost_groups() {
        assert_eq!(
            svchost_group(r"C:\Windows\system32\svchost.exe -k netsvcs -p").as_deref(),
            Some("netsvcs")
        );
        assert_eq!(
            svchost_group(r"C:\Windows\System32\svchost.exe -k LocalServiceNetworkRestricted"),
            Some("LocalServiceNetworkRestricted".into())
        );
        assert_eq!(svchost_group(r#""C:\Program Files\foo.exe""#), None);
    }

    #[test]
    fn enumerates_services() {
        let mut m = ServiceMonitor::new();
        let list = m.sample();
        assert!(list.len() > 50, "only {} services", list.len());
        // 事件日志服务总在运行，且托管在 svchost 里。
        let ev = list
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case("EventLog"))
            .expect("EventLog");
        assert_eq!(ev.state, State::Running);
        assert!(ev.pid > 0);
        assert!(ev.group.is_some());
    }
}
