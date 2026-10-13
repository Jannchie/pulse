//! 认出同名进程各自在跑什么：node.exe → "vite · pulse"，Code.exe 的子进程 → "GPU 进程"。
//!
//! 只看命令行、工作目录和少量文件是否存在，进程出现时算一次。

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::t;

/// 进程的身份：比 exe 名更具体的说明。
#[derive(Clone, Debug, PartialEq)]
pub enum Identity {
    /// 解释器 / 运行时在跑的程序，以及所属项目（目录名）。
    Script {
        tool: String,
        project: Option<String>,
    },
    /// Chromium / Electron 应用的子进程角色。
    Role(Role),
    /// WSL 2 虚拟机，附运行中的发行版。
    Wsl(Vec<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Role {
    Renderer,
    Extension,
    Gpu,
    Network,
    Storage,
    Audio,
    NodeService,
    Crashpad,
    VideoCapture,
    /// 其他 utility 进程，带服务名（如 "data_decoder"）。
    Utility(String),
}

impl Identity {
    /// 界面上显示的文字（按当前语言）。
    pub fn label(&self) -> String {
        match self {
            Identity::Script {
                tool,
                project: Some(p),
            } => format!("{tool} · {p}"),
            Identity::Script {
                tool,
                project: None,
            } => tool.clone(),
            Identity::Role(r) => r.label(),
            Identity::Wsl(distros) if distros.is_empty() => "WSL".into(),
            Identity::Wsl(distros) => format!("WSL · {}", distros.join(", ")),
        }
    }
}

impl Role {
    pub fn label(&self) -> String {
        match self {
            Role::Renderer => t!("渲染进程", "Renderer", "レンダラー").into(),
            Role::Extension => t!("扩展进程", "Extension", "拡張機能").into(),
            Role::Gpu => t!("GPU 进程", "GPU process", "GPU プロセス").into(),
            Role::Network => t!("网络服务", "Network service", "ネットワーク サービス").into(),
            Role::Storage => t!("存储服务", "Storage service", "ストレージ サービス").into(),
            Role::Audio => t!("音频服务", "Audio service", "オーディオ サービス").into(),
            Role::NodeService => t!("Node 服务", "Node service", "Node サービス").into(),
            Role::Crashpad => t!("崩溃报告", "Crash reporter", "クラッシュ レポーター").into(),
            Role::VideoCapture => t!("视频采集", "Video capture", "ビデオ キャプチャ").into(),
            Role::Utility(name) => name.clone(),
        }
    }
}

/// 按 exe 名、命令行和工作目录识别进程。`exe_path` 用来判断 Python 是否来自虚拟环境，
/// `cwd` 在脚本路径看不出项目时作为项目目录。
pub fn identify(
    exe: &str,
    exe_path: Option<&str>,
    cmd: &str,
    cwd: Option<&str>,
) -> Option<Identity> {
    let args = split_args(cmd);
    if let Some(role) = chromium_role(&args) {
        return Some(Identity::Role(role));
    }
    let stem = exe_stem(exe);
    let rest = args.get(1..).unwrap_or_default();
    let (tool, script) = match stem.as_str() {
        "node" | "bun" | "deno" => node_like(rest, cwd)?,
        "python" | "pythonw" | "python3" | "py" => python(rest, cwd)?,
        "java" | "javaw" => (java(rest)?, None),
        "dotnet" => (dotnet(rest)?, None),
        "powershell" | "pwsh" => powershell(rest, cwd)?,
        "cmd" => (cmd_exe(cmd)?, None),
        _ => return None,
    };
    // 项目：工作目录（vite、pnpm 都在项目目录里启动）→ 脚本所在的项目根 → Python 虚拟环境所在目录。
    // 工作目录是用户目录、全局安装目录之类时不算项目。
    let project = cwd
        .and_then(cwd_project)
        .or_else(|| script.as_deref().and_then(project_of))
        .or_else(|| exe_path.and_then(venv_project));
    Some(Identity::Script { tool, project })
}

/// 需要读取工作目录来识别的进程（读工作目录要打开进程内存，只对这些进程做）。
pub fn wants_cwd(exe: &str) -> bool {
    matches!(
        exe_stem(exe).as_str(),
        "node"
            | "bun"
            | "deno"
            | "python"
            | "pythonw"
            | "python3"
            | "py"
            | "java"
            | "javaw"
            | "dotnet"
            | "powershell"
            | "pwsh"
            | "cmd"
    )
}

fn exe_stem(exe: &str) -> String {
    let lower = exe.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

/// 按 Windows 的规则粗略拆分命令行：双引号内的空白不分隔，引号本身去掉。
pub fn split_args(cmd: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut started = false;
    for c in cmd.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    args.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(cur);
    }
    args
}

/// 路径的各段（兼容 / 与 \ 混用）。
fn components(path: &str) -> Vec<&str> {
    path.split(['\\', '/']).filter(|s| !s.is_empty()).collect()
}

fn file_name(path: &str) -> &str {
    components(path).last().copied().unwrap_or(path)
}

/// 相对路径按工作目录补全。
fn absolute(path: &str, cwd: Option<&str>) -> String {
    let p = Path::new(path);
    match cwd {
        Some(cwd) if p.is_relative() => Path::new(cwd).join(p).to_string_lossy().into_owned(),
        _ => path.to_string(),
    }
}

/// `--type=renderer` 之类的 Chromium 子进程参数。
fn chromium_role(args: &[String]) -> Option<Role> {
    let value = |key: &str| {
        args.iter()
            .find_map(|a| a.strip_prefix(key).map(str::to_string))
    };
    let kind = value("--type=")?;
    Some(match kind.as_str() {
        "renderer" if args.iter().any(|a| a == "--extension-process") => Role::Extension,
        "renderer" => Role::Renderer,
        "gpu-process" => Role::Gpu,
        "crashpad-handler" => Role::Crashpad,
        "utility" => {
            let sub = value("--utility-sub-type=").unwrap_or_default();
            match sub.as_str() {
                "network.mojom.NetworkService" => Role::Network,
                "video_capture.mojom.VideoCaptureService" => Role::VideoCapture,
                "storage.mojom.StorageService" => Role::Storage,
                "audio.mojom.AudioService" => Role::Audio,
                "node.mojom.NodeService" => Role::NodeService,
                // "data_decoder.mojom.DataDecoderService" → "data_decoder"
                s => Role::Utility(s.split('.').next().unwrap_or("utility").to_string()),
            }
        }
        other => Role::Utility(other.to_string()),
    })
}

/// node / bun / deno：第一个非选项参数是脚本。
fn node_like(args: &[String], cwd: Option<&str>) -> Option<(String, Option<String>)> {
    // 带值的选项要连同值一起跳过。
    const WITH_VALUE: [&str; 6] = ["-r", "--require", "--import", "--loader", "-e", "--eval"];
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') {
        if args[i] == "-e" || args[i] == "--eval" {
            return Some(("-e".into(), None));
        }
        i += if WITH_VALUE.contains(&args[i].as_str()) {
            2
        } else {
            1
        };
    }
    // deno run x.ts、bun run x.ts：跳过子命令。
    if matches!(args.get(i).map(String::as_str), Some("run" | "x" | "task")) {
        i += 1;
        while i < args.len() && args[i].starts_with('-') {
            i += 1;
        }
    }
    let script = args.get(i)?;
    let after = &args[i + 1..];
    let tool = node_tool(script);
    let tool = match after.first() {
        // 工具的子命令（"tauri dev"、"pnpm dev"）有助于区分，参数和路径就不带了。
        Some(sub) if is_subcommand(sub) => format!("{tool} {sub}"),
        _ => tool,
    };
    Some((tool, Some(absolute(script, cwd))))
}

/// 脚本对应的工具名：node_modules 里取包名，否则取文件名。
/// `…/node_modules/vite/bin/vite.js` → "vite"；`@tauri-apps/cli/tauri.js` 的包名太笼统，取脚本名 "tauri"。
fn node_tool(script: &str) -> String {
    const GENERIC: [&str; 6] = ["cli", "core", "bin", "server", "index", "main"];
    let parts = components(script);
    let name = file_name(script);
    let stem = name
        .rsplit_once('.')
        .filter(|(_, ext)| matches!(*ext, "js" | "cjs" | "mjs" | "ts"))
        .map_or(name, |(stem, _)| stem);
    let Some(k) = parts
        .iter()
        .rposition(|c| c.eq_ignore_ascii_case("node_modules"))
    else {
        return name.to_string();
    };
    match parts.get(k + 1..k + 3) {
        // node_modules/.bin/vite：转发脚本，文件名就是工具名。
        Some([".bin", _]) | None => stem.to_string(),
        Some([scope, pkg]) if scope.starts_with('@') => {
            if !GENERIC.contains(pkg) {
                pkg.to_string()
            } else if !GENERIC.contains(&stem) {
                stem.to_string()
            } else {
                format!("{}/{pkg}", &scope[1..])
            }
        }
        Some([pkg, _]) => pkg.to_string(),
        Some(_) => stem.to_string(),
    }
}

/// 像 `dev`、`sync-local-runner` 这样的单词，而不是路径、选项或数字。
fn is_subcommand(arg: &str) -> bool {
    !arg.is_empty()
        && arg.len() <= 24
        && arg.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':')
}

/// python：`-m 模块`、`-c`，否则第一个非选项参数是脚本。
fn python(args: &[String], cwd: Option<&str>) -> Option<(String, Option<String>)> {
    const WITH_VALUE: [&str; 3] = ["-W", "-X", "--check-hash-based-pycs"];
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') {
        match args[i].as_str() {
            "-m" => {
                let module = args.get(i + 1)?;
                let tool = match args.get(i + 2) {
                    // "uvicorn app:main" 的 app:main 是参数，不是子命令。
                    Some(sub) if is_subcommand(sub) && !sub.contains(':') => {
                        format!("{module} {sub}")
                    }
                    _ => module.clone(),
                };
                return Some((tool, None));
            }
            "-c" => return Some(("-c".into(), None)),
            a if WITH_VALUE.contains(&a) => i += 2,
            _ => i += 1,
        }
    }
    let script = args.get(i)?;
    // 虚拟环境里的入口：.venv\Scripts\uvicorn.exe、foo-script.py → "uvicorn"、"foo"。
    let name = file_name(script);
    let lower = name.to_ascii_lowercase();
    let tool = lower
        .strip_suffix(".exe")
        .or_else(|| lower.strip_suffix("-script.py"))
        .or_else(|| lower.strip_suffix("-script.pyw"))
        .map_or_else(|| name.to_string(), |s| name[..s.len()].to_string());
    Some((tool, Some(absolute(script, cwd))))
}

/// java：`-jar x.jar` → "x.jar"，否则是主类名的最后一段。
fn java(args: &[String]) -> Option<String> {
    const WITH_VALUE: [&str; 6] = [
        "-cp",
        "-classpath",
        "--class-path",
        "-p",
        "--module-path",
        "--add-opens",
    ];
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') {
        match args[i].as_str() {
            "-jar" => return args.get(i + 1).map(|j| file_name(j).to_string()),
            "-m" | "--module" => {
                let m = args.get(i + 1)?;
                return Some(m.rsplit(['/', '.']).next().unwrap_or(m).to_string());
            }
            a if WITH_VALUE.contains(&a) => i += 2,
            _ => i += 1,
        }
    }
    let class = args.get(i)?;
    Some(class.rsplit('.').next().unwrap_or(class).to_string())
}

/// dotnet：`dotnet x.dll` → "x"，`dotnet watch` → "dotnet watch"。
fn dotnet(args: &[String]) -> Option<String> {
    let first = args.iter().find(|a| !a.starts_with('-'))?;
    let name = file_name(first);
    match name.rsplit_once('.') {
        Some((stem, ext)) if ext.eq_ignore_ascii_case("dll") => Some(stem.to_string()),
        _ if is_subcommand(first) => Some(format!("dotnet {first}")),
        _ => None,
    }
}

/// powershell / pwsh：只认 `-File x.ps1`；`-Command` 的内容太杂，交给命令行摘要。
fn powershell(args: &[String], cwd: Option<&str>) -> Option<(String, Option<String>)> {
    let k = args
        .iter()
        .position(|a| a.eq_ignore_ascii_case("-file") || a.eq_ignore_ascii_case("-f"))?;
    let script = args.get(k + 1)?;
    Some((file_name(script).to_string(), Some(absolute(script, cwd))))
}

/// cmd：`/c 命令` → 命令的第一个词（npm 的脚本经常这样跑：`cmd.exe /d /s /c "vite"`）。
/// 取原始命令行里 /c 之后的部分再拆分，保留其中的引号（`/c "C:\Program Files\x.exe" …`）。
/// 和 cmd 一样：以引号开头时（`/s /c ""C:\x.exe" args"`）先去掉第一个和最后一个引号。
fn cmd_exe(cmd: &str) -> Option<String> {
    let lower = cmd.to_ascii_lowercase();
    let k = [" /c ", " /k "]
        .iter()
        .filter_map(|m| lower.find(m))
        .min()?;
    let mut command = cmd[k + 4..].trim().to_string();
    if command.starts_with('"')
        && let Some(last) = command.rfind('"')
        && last > 0
    {
        command.remove(last);
        command.remove(0);
    }
    let first = split_args(&command).into_iter().next()?;
    let name = file_name(&first);
    let lower = name.to_ascii_lowercase();
    // 脚本和程序的扩展名不显示：pyright-langserver.cmd → pyright-langserver。
    let stem = [".cmd", ".bat", ".exe", ".com"]
        .iter()
        .find_map(|ext| lower.strip_suffix(ext))
        .map_or(name, |s| &name[..s.len()]);
    Some(stem.to_string())
}

/// 标识项目根目录的文件。
const PROJECT_MARKERS: [&str; 8] = [
    "package.json",
    "pyproject.toml",
    "Cargo.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "deno.json",
    ".git",
];

/// 脚本所属的项目：在 node_modules 里就是 node_modules 所在的目录，否则向上找项目标记。
/// 全局安装的包（npm 全局目录、pnpm 的 .tools 等）找不到项目，返回 None 交给工作目录。
fn project_of(script: &str) -> Option<String> {
    let parts = components(script);
    if let Some(k) = parts
        .iter()
        .position(|c| c.eq_ignore_ascii_case("node_modules"))
    {
        let root = parts[..k].join("\\");
        return (k > 0 && has_marker(&root)).then(|| parts[k - 1].to_string());
    }
    // .venv\Scripts\x.exe：虚拟环境所在目录。
    if let Some(p) = venv_project(script) {
        return Some(p);
    }
    // 从脚本所在目录往上最多找 6 层。
    let dirs = parts.len().saturating_sub(1);
    (1..=dirs).rev().take(6).find_map(|n| {
        let dir = parts[..n].join("\\");
        has_marker(&dir).then(|| parts[n - 1].to_string())
    })
}

fn has_marker(dir: &str) -> bool {
    // "E:" 这样的盘符拼上 \ 才是根目录。
    let dir = if dir.ends_with(':') {
        format!("{dir}\\")
    } else {
        dir.to_string()
    };
    PROJECT_MARKERS
        .iter()
        .any(|m| Path::new(&dir).join(m).exists())
}

/// 路径在 Python 虚拟环境里时（…\proj\.venv\Scripts\python.exe），返回虚拟环境所在的目录名。
fn venv_project(path: &str) -> Option<String> {
    let parts = components(path);
    let k = parts
        .iter()
        .position(|c| c.eq_ignore_ascii_case("Scripts"))?;
    if k < 2 {
        return None;
    }
    let venv = parts[..k].join("\\");
    Path::new(&venv)
        .join("pyvenv.cfg")
        .exists()
        .then(|| parts[k - 2].to_string())
}

/// 工作目录作为项目：盘符根目录、系统目录不算。
fn cwd_project(cwd: &str) -> Option<String> {
    let trimmed = cwd.trim_end_matches(['\\', '/']);
    let lower = trimmed.to_ascii_lowercase().replace('/', "\\");
    let home = std::env::var("USERPROFILE")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let skip = lower.ends_with(':')
        || (!home.is_empty() && (lower == home || lower.starts_with(&format!("{home}\\appdata"))))
        || [
            "\\node_modules",
            "\\program files",
            "\\programdata",
            ":\\windows",
        ]
        .iter()
        .any(|dir| lower.contains(dir));
    if skip {
        return None;
    }
    let name = dir_project(trimmed);
    (!name.is_empty()).then(|| name.to_string())
}

/// 目录对应的项目名：一般就是目录名；`src-tauri`、`src`、`frontend` 这类通用的子目录名
/// 说明不了是哪个项目，往上取父目录的名字（最多两层）。
fn dir_project(dir: &str) -> &str {
    const GENERIC: [&str; 14] = [
        "src",
        "src-tauri",
        "app",
        "frontend",
        "backend",
        "web",
        "server",
        "client",
        "ui",
        "dist",
        "build",
        "bin",
        "scripts",
        "tools",
    ];
    let parts = components(dir);
    let mut n = parts.len();
    while n > 1
        && parts.len() - n < 2
        && GENERIC.iter().any(|g| parts[n - 1].eq_ignore_ascii_case(g))
    {
        n -= 1;
    }
    parts.get(n.wrapping_sub(1)).copied().unwrap_or(dir)
}

/// WSL 2 的虚拟机进程（vmmemWSL）上运行中的发行版。
/// 查询要启动 wsl.exe（约 100 ms），所以放到后台线程，结果缓存 15 秒。
#[derive(Default)]
pub struct WslWatcher {
    state: Arc<Mutex<WslState>>,
}

#[derive(Default)]
struct WslState {
    distros: Vec<String>,
    refreshed: Option<Instant>,
    refreshing: bool,
}

impl WslWatcher {
    /// 上次查到的发行版；过期时在后台重新查询。
    pub fn distros(&self) -> Vec<String> {
        let Ok(mut st) = self.state.lock() else {
            return Vec::new();
        };
        let stale = st
            .refreshed
            .is_none_or(|t| t.elapsed() > Duration::from_secs(15));
        if stale && !st.refreshing {
            st.refreshing = true;
            let state = self.state.clone();
            std::thread::spawn(move || {
                let distros = running_wsl_distros();
                if let Ok(mut st) = state.lock() {
                    st.distros = distros;
                    st.refreshed = Some(Instant::now());
                    st.refreshing = false;
                }
            });
        }
        st.distros.clone()
    }
}

/// `wsl.exe --list --running --quiet` 的输出（UTF-16LE，每行一个发行版）。
fn running_wsl_distros() -> Vec<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(out) = std::process::Command::new("wsl.exe")
        .args(["--list", "--running", "--quiet"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let wide: Vec<u16> = out
        .stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&wide)
        .lines()
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}' || c == '\0'))
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(exe: &str, cmd: &str, cwd: Option<&str>) -> Option<String> {
        identify(exe, None, cmd, cwd).map(|i| i.label())
    }

    #[test]
    fn splits_quoted_args() {
        assert_eq!(
            split_args(r#""node"   "C:\a b\x.js" "--stdio" """#),
            vec!["node", r"C:\a b\x.js", "--stdio", ""]
        );
    }

    #[test]
    fn node_tools_and_subcommands() {
        // 全局包：项目来自工作目录。
        assert_eq!(
            id(
                "node.exe",
                r#""node"   "C:\Users\u\AppData\Roaming\npm\\node_modules\pyright\langserver.index.js" "--stdio""#,
                Some(r"E:\code\pulse")
            )
            .as_deref(),
            Some("pyright · pulse")
        );
        assert_eq!(
            id(
                "node.exe",
                r"C:\nvm4w\nodejs\node.exe C:\Users\u\AppData\Local\pnpm/.tools/pnpm/10.11.1/node_modules/pnpm/bin/pnpm.cjs dev --port 5173",
                Some(r"E:\code\melo-crafter\")
            )
            .as_deref(),
            Some("pnpm dev · melo-crafter")
        );
        assert_eq!(
            id(
                "node.exe",
                r"node C:\x\node_modules\.pnpm\@tauri-apps+cli@2\node_modules\@tauri-apps/cli/tauri.js dev",
                None
            )
            .as_deref(),
            Some("tauri dev")
        );
        assert_eq!(id("node.exe", "node -e \"1\"", None).as_deref(), Some("-e"));
        assert_eq!(
            id("node.exe", "node --inspect server.js", None).as_deref(),
            Some("server.js")
        );
        assert_eq!(id("node.exe", "node", None), None);
    }

    #[test]
    fn python_modules_and_scripts() {
        assert_eq!(
            id(
                "python.exe",
                "python -m uvicorn app:main",
                Some(r"E:\code\api")
            )
            .as_deref(),
            Some("uvicorn · api")
        );
        assert_eq!(
            id("python.exe", "python -X utf8 -m pip install x", None).as_deref(),
            Some("pip install")
        );
        assert_eq!(
            id(
                "python.exe",
                r#""C:\p\.venv\Scripts\python.exe" "C:\p\.venv\Scripts\uvicorn.exe" app:main"#,
                None
            )
            .as_deref(),
            Some("uvicorn")
        );
        assert_eq!(
            id("python.exe", "python -c print(1)", None).as_deref(),
            Some("-c")
        );
    }

    #[test]
    fn other_runtimes() {
        assert_eq!(
            id(
                "java.exe",
                "java -Xmx1g -cp a.jar;b.jar org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.5",
                None
            )
            .as_deref(),
            Some("GradleDaemon")
        );
        assert_eq!(
            id("java.exe", r"java -jar C:\x\server.jar nogui", None).as_deref(),
            Some("server.jar")
        );
        assert_eq!(
            id("dotnet.exe", r"dotnet C:\a\Api.dll", None).as_deref(),
            Some("Api")
        );
        assert_eq!(
            id("dotnet.exe", "dotnet watch run", None).as_deref(),
            Some("dotnet watch")
        );
        assert_eq!(
            id(
                "cmd.exe",
                r#"C:\WINDOWS\system32\cmd.exe /d /s /c "vite""#,
                None
            )
            .as_deref(),
            Some("vite")
        );
        assert_eq!(
            id(
                "cmd.exe",
                r#"C:\WINDOWS\system32\cmd.exe /d /s /c ""C:\Program Files\A B\run.exe" chrome-extension://x/ --parent-window=0" < \\.\pipe\in"#,
                None
            )
            .as_deref(),
            Some("run")
        );
        assert_eq!(
            id(
                "cmd.exe",
                r#"cmd.exe /q /d /s /c "C:\npm\pyright-langserver.cmd ^"--stdio^"""#,
                None
            )
            .as_deref(),
            Some("pyright-langserver")
        );
        assert_eq!(
            id("pwsh.exe", r"pwsh -NoProfile -File C:\s\build.ps1", None).as_deref(),
            Some("build.ps1")
        );
        assert_eq!(id("pwsh.exe", "pwsh -Command Get-Date", None), None);
        assert_eq!(id("explorer.exe", r"C:\Windows\explorer.exe", None), None);
    }

    #[test]
    fn chromium_roles() {
        let role = |cmd: &str| identify("chrome.exe", None, cmd, None);
        assert_eq!(
            role("chrome.exe --type=gpu-process --x"),
            Some(Identity::Role(Role::Gpu))
        );
        assert_eq!(
            role("chrome.exe --type=renderer --extension-process"),
            Some(Identity::Role(Role::Extension))
        );
        assert_eq!(
            role("code.exe --type=utility --utility-sub-type=node.mojom.NodeService"),
            Some(Identity::Role(Role::NodeService))
        );
        assert_eq!(
            role(
                "msedge.exe --type=utility --utility-sub-type=data_decoder.mojom.DataDecoderService"
            ),
            Some(Identity::Role(Role::Utility("data_decoder".into())))
        );
        assert_eq!(role("chrome.exe --no-startup-window"), None);
    }

    #[test]
    fn cwd_project_skips_roots() {
        assert_eq!(cwd_project(r"C:\"), None);
        assert_eq!(cwd_project(r"C:\WINDOWS\system32"), None);
        assert_eq!(cwd_project(r"E:\code\pulse\").as_deref(), Some("pulse"));
        assert_eq!(
            cwd_project(r"c:\Users\x\AppData\Roaming\npm\node_modules\pyright\dist\"),
            None
        );
        assert_eq!(cwd_project(r"C:\Program Files\WindowsApps\x\iCloud\"), None);
    }

    #[test]
    fn repo_name_skips_generic_subdirs() {
        assert_eq!(dir_project(r"E:\code\yomu\src-tauri"), "yomu");
        assert_eq!(dir_project(r"E:\code\shop\server\src"), "shop");
        assert_eq!(dir_project(r"E:\code\melo-crafter"), "melo-crafter");
        assert_eq!(dir_project(r"E:\a\src\src\src"), "src");
    }
}
