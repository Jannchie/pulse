//! 进程规则：按 exe 名记住优先级、处理器相关性与效率模式，程序每次启动时由采集线程自动套用。
//!
//! 保存在 `%APPDATA%\Pulse\rules.ini`，每行一个程序：`chrome.exe=priority:below-normal,affinity:ff,efficiency`。

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use pulse_probe::Process;
use pulse_probe::control::{self, Priority};
use pulse_probe::{t, tf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rule {
    pub priority: Option<Priority>,
    /// 处理器相关性掩码；套用时与系统可用的处理器取交集。
    pub affinity: Option<u64>,
    pub efficiency: bool,
}

impl Rule {
    pub fn is_empty(&self) -> bool {
        self.priority.is_none() && self.affinity.is_none() && !self.efficiency
    }

    /// 列在设置页里的说明，如"优先级 高 · 4 个处理器 · 效率模式"。
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(p) = self.priority {
            parts.push(tf!("优先级 {}", "Priority {}", "優先度 {}", p.label()));
        }
        if let Some(mask) = self.affinity {
            parts.push(affinity_label(mask));
        }
        if self.efficiency {
            parts.push(t!("效率模式", "Efficiency mode", "効率モード").to_string());
        }
        parts.join(" · ")
    }

    fn encode(&self) -> String {
        let mut parts = Vec::new();
        if let Some(p) = self.priority {
            parts.push(format!("priority:{}", priority_key(p)));
        }
        if let Some(mask) = self.affinity {
            parts.push(format!("affinity:{mask:x}"));
        }
        if self.efficiency {
            parts.push("efficiency".to_string());
        }
        parts.join(",")
    }

    fn decode(s: &str) -> Self {
        let mut rule = Rule::default();
        for part in s.split(',').map(str::trim) {
            match part.split_once(':') {
                Some(("priority", v)) => {
                    rule.priority = Priority::ALL.into_iter().find(|&p| priority_key(p) == v);
                }
                Some(("affinity", v)) => {
                    rule.affinity = u64::from_str_radix(v, 16).ok().filter(|&m| m != 0);
                }
                _ if part == "efficiency" => rule.efficiency = true,
                _ => {}
            }
        }
        rule
    }

    /// 套用到一个进程。效率模式会把优先级降到"低"，所以先设效率模式、再设优先级。
    pub fn apply(&self, pid: u32) -> control::Result<()> {
        let mut result = Ok(());
        let mut keep = |r: control::Result<()>| {
            if result.is_ok() {
                result = r;
            }
        };
        if self.efficiency {
            keep(control::set_efficiency_mode(pid, true));
        }
        if let Some(p) = self.priority {
            keep(control::set_priority(pid, p));
        }
        if let Some(mask) = self.affinity {
            match control::affinity(pid) {
                Ok((_, system)) if mask & system != 0 => {
                    keep(control::set_affinity(pid, mask & system))
                }
                Ok(_) => {}
                Err(e) => keep(Err(e)),
            }
        }
        result
    }
}

fn priority_key(p: Priority) -> &'static str {
    match p {
        Priority::Idle => "idle",
        Priority::BelowNormal => "below-normal",
        Priority::Normal => "normal",
        Priority::AboveNormal => "above-normal",
        Priority::High => "high",
        Priority::Realtime => "realtime",
    }
}

pub fn affinity_label(mask: u64) -> String {
    tf!(
        "{} 个处理器",
        "{} processors",
        "{} 個のプロセッサ",
        mask.count_ones()
    )
}

/// 所有规则，键是小写的 exe 文件名（含扩展名，与进程名一致）。
#[derive(Clone, Debug, Default)]
pub struct Rules {
    map: BTreeMap<String, Rule>,
}

/// 采集线程与界面共用。
pub type Shared = Arc<RwLock<Rules>>;

impl Rules {
    fn path() -> Option<PathBuf> {
        Some(
            PathBuf::from(std::env::var_os("APPDATA")?)
                .join("Pulse")
                .join("rules.ini"),
        )
    }

    pub fn load() -> Self {
        let text = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        Self::parse(&text)
    }

    fn parse(text: &str) -> Self {
        let map = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim().to_lowercase(), Rule::decode(v)))
            .filter(|(k, r)| !k.is_empty() && !r.is_empty())
            .collect();
        Self { map }
    }

    fn serialize(&self) -> String {
        self.map
            .iter()
            .map(|(k, r)| format!("{k}={}\n", r.encode()))
            .collect()
    }

    fn save(&self) -> std::io::Result<()> {
        let Some(p) = Self::path() else {
            return Ok(());
        };
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(p, self.serialize())
    }

    pub fn get(&self, name: &str) -> Option<&Rule> {
        self.map.get(&name.to_lowercase())
    }

    /// 设置（空规则即删除）并保存。
    pub fn set(&mut self, name: &str, rule: Rule) -> std::io::Result<()> {
        let key = name.to_lowercase();
        if rule.is_empty() {
            self.map.remove(&key);
        } else {
            self.map.insert(key, rule);
        }
        self.save()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Rule)> {
        self.map.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// 采集线程里用：每个进程（pid + 创建时间）只套用一次，之后用户手动改了也不会被改回去。
#[derive(Default)]
pub struct Applier {
    seen: HashSet<(u32, i64)>,
}

impl Applier {
    pub fn run(&mut self, rules: &Rules, procs: &[Process]) {
        let mut next = HashSet::with_capacity(procs.len());
        for p in procs {
            let key = (p.pid, p.create_time);
            next.insert(key);
            if self.seen.contains(&key) || rules.is_empty() {
                continue;
            }
            if let Some(rule) = rules.get(&p.name) {
                // 受保护的进程、以管理员身份运行的程序（Pulse 自己没有提权时）会失败，忽略。
                let _ = rule.apply(p.pid);
            }
        }
        self.seen = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let text = "Chrome.exe=priority:below-normal,affinity:f0,efficiency\n\
                    game.exe=priority:high\n\
                    empty.exe=\n\
                    bad.exe=priority:fast,affinity:0\n";
        let rules = Rules::parse(text);
        let chrome = rules.get("chrome.EXE").unwrap();
        assert_eq!(chrome.priority, Some(Priority::BelowNormal));
        assert_eq!(chrome.affinity, Some(0xf0));
        assert!(chrome.efficiency);
        assert_eq!(
            rules.get("game.exe").unwrap().priority,
            Some(Priority::High)
        );
        // 空规则与无法识别的值都被丢掉。
        assert!(rules.get("empty.exe").is_none());
        assert!(rules.get("bad.exe").is_none());
        assert_eq!(
            Rules::parse(&rules.serialize()).serialize(),
            rules.serialize()
        );
        assert_eq!(
            rules.serialize(),
            "chrome.exe=priority:below-normal,affinity:f0,efficiency\ngame.exe=priority:high\n"
        );
    }
}
