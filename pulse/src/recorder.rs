//! 记录线程：每秒看一次前台程序，每分钟把这一分钟的使用情况和性能平均值写进数据库。
//!
//! 采集线程每次采样后把整体占用和各进程的数值发过来（[`Recorder::perf`]）；前台程序、窗口标题、
//! 是否空闲由这里自己查。用户超过 [`IDLE_MS`] 没有任何键盘、鼠标、手柄输入，或屏幕已锁定时不计时。

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::history::{AppRef, Db, PerfMinute, Seconds, TopKind};

/// 超过这么久没有键盘鼠标输入就算离开，不再计时。
pub const IDLE_MS: u32 = 5 * 60 * 1000;
/// 每个类别保留占用最高的几个进程。
const TOP_N: usize = 5;

/// 一个进程在一次采样里的数值。
pub struct ProcSample {
    pub pid: u32,
    pub name: String,
    pub cpu: f32,
    pub memory: u64,
    pub disk: u64,
    pub net: u64,
    pub gpu: f32,
}

/// 采集线程的一次采样。
pub struct PerfSample {
    pub cpu: f32,
    pub mem: f32,
    pub gpu: f32,
    pub disk: u64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub procs: Vec<ProcSample>,
}

enum Msg {
    Perf(PerfSample),
    SetEnabled(bool),
    /// 立即写入当前这一分钟，写完后回复。
    Flush(Sender<()>),
}

#[derive(Clone)]
pub struct Recorder {
    tx: Sender<Msg>,
}

impl Recorder {
    pub fn start(enabled: bool) -> Self {
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || run(rx, enabled))
            .expect("spawn recorder");
        Self { tx }
    }

    pub fn perf(&self, s: PerfSample) {
        let _ = self.tx.send(Msg::Perf(s));
    }

    pub fn set_enabled(&self, on: bool) {
        let _ = self.tx.send(Msg::SetEnabled(on));
    }

    /// 把还没写入的这一分钟写进数据库（退出前调用），最多等两秒。
    pub fn flush(&self) {
        let (tx, rx) = channel();
        if self.tx.send(Msg::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(2));
        }
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn now_minute() -> i64 {
    now_secs() / 60
}

/// 从进程累计里取出某一类数值。
type Metric = fn(&ProcAcc) -> f64;

/// 一分钟里各进程的累计。
#[derive(Default)]
struct ProcAcc {
    name: String,
    cpu: f64,
    disk: f64,
    net: f64,
    gpu: f64,
    memory: u64,
}

#[derive(Default)]
struct PerfAcc {
    samples: u32,
    cpu: f64,
    mem: f64,
    gpu: f64,
    disk: u64,
    net_rx: u64,
    net_tx: u64,
    procs: HashMap<u32, ProcAcc>,
}

impl PerfAcc {
    fn add(&mut self, s: PerfSample) {
        self.samples += 1;
        self.cpu += s.cpu as f64;
        self.mem += s.mem as f64;
        self.gpu += s.gpu as f64;
        self.disk += s.disk;
        self.net_rx += s.net_rx;
        self.net_tx += s.net_tx;
        for p in s.procs {
            let a = self.procs.entry(p.pid).or_default();
            if a.name.is_empty() {
                a.name = p.name;
            }
            a.cpu += p.cpu as f64;
            a.disk += p.disk as f64;
            a.net += p.net as f64;
            a.gpu += p.gpu as f64;
            a.memory = a.memory.max(p.memory);
        }
    }

    /// 这一分钟的平均值与各类别前几名；没有采样（暂停更新）时为 None。
    fn finish(&mut self) -> Option<PerfMinute> {
        let n = std::mem::take(&mut self.samples);
        let procs = std::mem::take(&mut self.procs);
        if n == 0 {
            return None;
        }
        let f = n as f64;
        let mut top = Vec::new();
        let kinds: [(TopKind, Metric); 5] = [
            (TopKind::Cpu, |p| p.cpu),
            (TopKind::Memory, |p| p.memory as f64),
            (TopKind::Disk, |p| p.disk),
            (TopKind::Network, |p| p.net),
            (TopKind::Gpu, |p| p.gpu),
        ];
        for (kind, value) in kinds {
            let mut v: Vec<(&u32, &ProcAcc, f64)> = procs
                .iter()
                .map(|(pid, p)| {
                    // 内存取这一分钟里的最大值，其余取平均。
                    let x = if kind == TopKind::Memory {
                        value(p)
                    } else {
                        value(p) / f
                    };
                    (pid, p, x)
                })
                .filter(|x| x.2 > 0.0)
                .collect();
            v.sort_by(|a, b| b.2.total_cmp(&a.2));
            for (rank, (pid, p, x)) in v.into_iter().take(TOP_N).enumerate() {
                top.push((kind, rank as u8, exe_stem(&p.name).to_string(), *pid, x));
            }
        }
        let r = PerfMinute {
            cpu: (self.cpu / f) as f32,
            mem: (self.mem / f) as f32,
            gpu: (self.gpu / f) as f32,
            disk: self.disk / n as u64,
            net_rx: self.net_rx / n as u64,
            net_tx: self.net_tx / n as u64,
            top,
        };
        *self = Self::default();
        Some(r)
    }
}

fn exe_stem(name: &str) -> &str {
    name.len()
        .checked_sub(4)
        .filter(|&n| name.is_char_boundary(n) && name[n..].eq_ignore_ascii_case(".exe"))
        .map_or(name, |n| &name[..n])
}

/// 一秒钟的前台记录：前台程序与标题；不在电脑前（空闲、锁屏）时为 None。
fn sample_foreground() -> Option<(AppRef, String)> {
    // 键盘鼠标和手柄都超过 IDLE_MS 没动才算离开（系统的空闲时间不含手柄）。
    crate::input::poll_xinput();
    if pulse_probe::foreground::idle_ms() >= IDLE_MS && crate::input::gamepad_idle_ms() >= IDLE_MS {
        return None;
    }
    let fg = pulse_probe::foreground::foreground()?;
    let stem = fg.stem()?;
    // 锁屏界面。
    if stem.eq_ignore_ascii_case("LockApp") {
        return None;
    }
    Some((AppRef::new(stem, fg.path.as_deref()), fg.title))
}

fn run(rx: Receiver<Msg>, mut enabled: bool) {
    let mut db = Db::open().ok();
    let mut usage: HashMap<(AppRef, String), Seconds> = HashMap::new();
    let mut perf = PerfAcc::default();
    let mut minute = now_minute();
    let mut next_tick = Instant::now() + Duration::from_secs(1);
    let mut pruned_day = 0i64;

    let mut write = |db: &mut Option<Db>,
                     minute: i64,
                     usage: &mut HashMap<(AppRef, String), Seconds>,
                     perf: &mut PerfAcc| {
        let p = perf.finish();
        let Some(db) = db.as_mut() else {
            usage.clear();
            return;
        };
        if !usage.is_empty() && db.meta("recording_since").is_none() {
            let _ = db.set_meta("recording_since", &minute.to_string());
        }
        if !usage.is_empty() || p.is_some() {
            let _ = db.write_minute(minute, usage, p.as_ref());
        }
        usage.clear();
        let day = minute / 1440;
        if day != pruned_day {
            pruned_day = day;
            let _ = db.prune(minute);
        }
    };

    loop {
        let timeout = next_tick.saturating_duration_since(Instant::now());
        match rx.recv_timeout(timeout) {
            Ok(Msg::Perf(s)) => {
                if enabled {
                    perf.add(s);
                }
                continue;
            }
            Ok(Msg::SetEnabled(on)) => {
                enabled = on;
                if !on {
                    usage.clear();
                    perf = PerfAcc::default();
                }
                continue;
            }
            Ok(Msg::Flush(done)) => {
                write(&mut db, minute, &mut usage, &mut perf);
                let _ = done.send(());
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => {
                write(&mut db, minute, &mut usage, &mut perf);
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        // 每秒一次。睡眠、休眠醒来后跳过错过的秒数，不补记。
        next_tick += Duration::from_secs(1);
        if next_tick < Instant::now() {
            next_tick = Instant::now() + Duration::from_secs(1);
        }
        let now = now_minute();
        if now != minute {
            write(&mut db, minute, &mut usage, &mut perf);
            minute = now;
        }
        let keys = crate::input::take_keys();
        if !enabled {
            continue;
        }
        if let Some(key) = sample_foreground() {
            let s = usage.entry(key).or_default();
            s.active = (s.active + 1).min(60);
            if keys > 0 {
                s.typing = (s.typing + 1).min(60);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, name: &str, cpu: f32, memory: u64) -> ProcSample {
        ProcSample {
            pid,
            name: name.into(),
            cpu,
            memory,
            disk: 0,
            net: 0,
            gpu: 0.0,
        }
    }

    #[test]
    fn perf_minute_averages_and_ranks() {
        let mut acc = PerfAcc::default();
        for cpu in [10.0, 30.0] {
            acc.add(PerfSample {
                cpu,
                mem: 0.5,
                gpu: 0.0,
                disk: 100,
                net_rx: 0,
                net_tx: 0,
                procs: vec![proc(1, "a.exe", cpu, 100), proc(2, "b.exe", 1.0, 900)],
            });
        }
        let m = acc.finish().unwrap();
        assert_eq!(m.cpu, 20.0);
        assert_eq!(m.disk, 100);
        let cpu: Vec<_> = m.top.iter().filter(|t| t.0 == TopKind::Cpu).collect();
        assert_eq!(cpu[0].2, "a");
        assert_eq!(cpu[0].4, 20.0);
        let mem: Vec<_> = m.top.iter().filter(|t| t.0 == TopKind::Memory).collect();
        assert_eq!(mem[0].2, "b");
        // 没有数值的类别不记录。
        assert!(!m.top.iter().any(|t| t.0 == TopKind::Disk));
        assert!(acc.finish().is_none());
    }
}
