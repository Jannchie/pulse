<p align="center">
  <img src="docs/icon.png" width="96" alt="Pulse">
  <h1 align="center">Pulse</h1>
  <p align="center">
    <a href="https://github.com/Jannchie/pulse/actions/workflows/ci.yml"><img src="https://github.com/Jannchie/pulse/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  </p>
  <p align="center">中文 · <a href="README.en.md">English</a></p>
  <img src="docs/screenshot.zh.png" alt="Pulse 性能页">
</p>

Pulse 是 Windows 任务管理器的替代品，使用 Rust 与 egui 原生实现，单个 exe，无需安装。界面支持简体中文与英文，默认跟随 Windows 显示语言，可在「设置 → 外观」中切换。

与系统任务管理器相比：

- 搜索覆盖进程名、描述、命令行和路径，支持模糊匹配、拼写容错和 `:端口` 查找占用者。
- 逐进程显示 GPU 利用率与显存、网络流量、监听端口和 CPU 时间。
- 性能页提供 NVMe 健康信息（剩余寿命、通电时间、累计写入）、内存条与大小核信息。
- 空闲时几乎不占用 CPU：只在数据或输入变化时重绘。

## 安装

从 [Releases](https://github.com/Jannchie/pulse/releases) 下载 `pulse-<版本>-windows-x64.exe` 后直接运行。系统要求 Windows 10 或 11（x64）。

## 快速上手

```powershell
pulse.exe :5173        # 打开后直接定位占用 5173 端口的进程
```

命令行参数会作为初始搜索词。启动后在进程页输入关键词即可筛选，右键进程可结束、挂起、调整优先级与相关性、创建转储文件或打开文件位置。

## 常用用法

- **查找端口占用**：搜索 `:8080`，或打开工具栏的「监听端口」只看正在监听 TCP 端口的进程。
- **组合搜索**：多个词以空格分隔且须同时命中；`!chrome` 排除、`^node` 匹配开头、`'vite` 精确匹配。
- **结束进程树**：右键进程选择「结束进程树」，先结束子进程再结束父进程。
- **替换 Ctrl+Shift+Esc**：在「设置 → 系统」中开启「替换任务管理器」，之后系统打开任务管理器时会启动 Pulse。

以下功能需要以管理员身份运行（工具栏提供「以管理员身份重启」）：逐进程网络统计、已修改与备用内存、结束系统进程、启停服务、修改所有用户的启动项、替换任务管理器。

## 从源码构建

需要 Rust 1.88 或更高版本，以及 MSVC 工具链（用于编译 exe 的图标资源）。

```powershell
cargo build --release   # 产物位于 target\release\pulse.exe
```

发布新版本：修改 `pulse/Cargo.toml` 中的版本号并推送同名标签（如 `v0.2.0`），CI 会构建并创建 Release。
