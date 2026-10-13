//! 开机自启：登录后在通知区域启动 Pulse（命令行带 `--tray`，不弹出窗口）。
//!
//! 以管理员身份开启时注册"以最高权限运行"的计划任务：登录后直接以管理员身份启动、不弹 UAC，
//! 逐进程网络统计等需要管理员的功能都可用。普通权限开启时写当前用户的 Run 键。

use std::os::windows::process::CommandExt;
use std::process::Command;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteValueW,
    RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::core::{PCWSTR, w};

/// 自启时附加的参数：只显示托盘图标。
pub const TRAY_FLAG: &str = "--tray";
const TASK: &str = "Pulse";
const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 是否已开启（计划任务或 Run 键任一存在）。会启动 schtasks.exe，放在后台线程调用。
pub fn enabled() -> bool {
    run_value().is_some() || task_exists()
}

/// 开启或关闭。`elevated` 决定用计划任务还是 Run 键；关闭时两者都删。
pub fn set(on: bool, elevated: bool) -> Result<(), String> {
    if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe = exe.to_string_lossy();
        if elevated {
            create_task(TASK, &exe)?;
            // 计划任务已经会启动它，Run 键再启动一次就重复了。
            set_run_value(None)
        } else {
            set_run_value(Some(&format!("\"{exe}\" {TRAY_FLAG}")))
        }
    } else {
        set_run_value(None)?;
        if task_exists() {
            schtasks(&["/Delete", "/TN", TASK, "/F"])?;
        }
        Ok(())
    }
}

fn task_exists() -> bool {
    schtasks(&["/Query", "/TN", TASK]).is_ok()
}

fn schtasks(args: &[&str]) -> Result<(), String> {
    let out = Command::new("schtasks.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        // schtasks 按控制台代码页输出错误信息。
        Err(crate::os::decode_console(&out.stderr).trim().to_string())
    }
}

/// 用 XML 注册任务：命令行参数建出来的任务默认"只在接通电源时运行"且"运行超过 3 天就停止"，
/// 都不适合常驻程序，必须在 XML 里关掉。
fn create_task(name: &str, exe: &str) -> Result<(), String> {
    let user = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    let xml = task_xml(exe, &user);
    let path = std::env::temp_dir().join(format!("pulse-task-{}.xml", std::process::id()));
    // schtasks 要求 XML 是带 BOM 的 UTF-16。
    let bytes: Vec<u8> = [0xFEFFu16]
        .into_iter()
        .chain(xml.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let path_s = path.to_string_lossy().into_owned();
    let r = schtasks(&["/Create", "/TN", name, "/XML", &path_s, "/F"]);
    let _ = std::fs::remove_file(&path);
    r
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn task_xml(exe: &str, user: &str) -> String {
    let (exe, user) = (escape(exe), escape(user));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Start Pulse in the notification area at sign-in.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>"{exe}"</Command>
      <Arguments>{TRAY_FLAG}</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

fn run_value() -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            w!("Pulse"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let n = (len as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// 写入（Some）或删除（None）Run 键里的 Pulse 值。
fn set_run_value(value: Option<&str>) -> Result<(), String> {
    let mut key = HKEY::default();
    let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key) };
    if r != ERROR_SUCCESS {
        return Err(crate::win32_message(r.0));
    }
    let r = match value {
        Some(v) => {
            let data: Vec<u8> = v
                .encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect();
            unsafe { RegSetValueExW(key, w!("Pulse"), None, REG_SZ, Some(&data)) }
        }
        None => unsafe { RegDeleteValueW(key, w!("Pulse")) },
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    // 删除一个本来就不存在的值（ERROR_FILE_NOT_FOUND）也算成功。
    if r == ERROR_SUCCESS || (value.is_none() && r.0 == 2) {
        Ok(())
    } else {
        Err(crate::win32_message(r.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_xml_escapes_paths() {
        let xml = task_xml(r"C:\A & B\pulse.exe", r"PC\me");
        assert!(xml.contains(r#"<Command>"C:\A &amp; B\pulse.exe"</Command>"#));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(xml.contains(r"<UserId>PC\me</UserId>"));
    }

    /// 真正向任务计划程序注册一个测试任务（指向不存在的程序），确认 XML 被接受，随后删除。
    /// 需要管理员权限：`cargo test -p pulse-probe -- --ignored registers_task`。
    #[test]
    #[ignore]
    fn registers_task() {
        let name = "PulseAutostartTest";
        create_task(name, r"C:\pulse-autostart-test\missing.exe").unwrap();
        assert!(schtasks(&["/Query", "/TN", name]).is_ok());
        schtasks(&["/Delete", "/TN", name, "/F"]).unwrap();
        assert!(schtasks(&["/Query", "/TN", name]).is_err());
    }
}
