//! 把快照整理成表格行：过滤、按名称分组、排序。

use std::collections::{HashMap, HashSet};

use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use pulse_probe::{Process, Snapshot};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Pid,
    Cpu,
    CpuTime,
    Memory,
    Gpu,
    GpuMemory,
    Disk,
    Network,
}

impl SortKey {
    /// 数值列默认降序，文字列默认升序。
    pub fn default_descending(self) -> bool {
        !matches!(self, SortKey::Name | SortKey::Pid)
    }
}

#[derive(Default, Clone, Copy)]
pub struct Metrics {
    pub cpu: f32,
    /// 累计 CPU 时间，100ns 为单位。
    pub cpu_time: u64,
    pub memory: u64,
    /// 专用显存。
    pub vram: u64,
    pub gpu: f32,
    pub disk: u64,
    pub net_rx: u64,
    pub net_tx: u64,
}

impl Metrics {
    pub fn of(p: &Process) -> Self {
        Self {
            cpu: p.cpu,
            cpu_time: p.user_time + p.kernel_time,
            memory: p.private_ws,
            vram: p.gpu_dedicated,
            gpu: p.gpu,
            disk: p.io_rate,
            net_rx: p.net_rx,
            net_tx: p.net_tx,
        }
    }

    fn add(&mut self, o: &Metrics) {
        self.cpu += o.cpu;
        self.cpu_time += o.cpu_time;
        self.memory += o.memory;
        self.vram += o.vram;
        self.gpu = (self.gpu + o.gpu).min(100.0);
        self.disk += o.disk;
        self.net_rx += o.net_rx;
        self.net_tx += o.net_tx;
    }

    pub fn net(&self) -> u64 {
        self.net_rx + self.net_tx
    }
}

pub struct Group {
    pub name: String,
    /// 分组键（小写名称），用于记录展开状态。
    pub key: String,
    pub members: Vec<usize>,
    pub metrics: Metrics,
}

#[derive(Clone, Copy)]
pub enum Row {
    /// 有多个成员的分组标题行。
    Group(usize),
    /// 展开分组中的成员，缩进显示。
    Member(usize),
    /// 独立进程（未分组模式，或分组只有一个成员）。
    Single(usize),
}

#[derive(Default)]
pub struct Table {
    pub groups: Vec<Group>,
    pub rows: Vec<Row>,
}

pub struct Query<'a> {
    pub filter: &'a str,
    pub grouped: bool,
    pub sort: SortKey,
    pub descending: bool,
    pub expanded: &'a HashSet<String>,
    /// 只看有 TCP 监听端口的进程。
    pub listening_only: bool,
}

/// 搜索用的一个关键词。
enum Term {
    /// `:端口`，只精确匹配监听端口。
    Port(u16),
    /// 普通词（含 fzf 语法：`'精确`、`^前缀`、`后缀$`、`!排除`）。纯数字时额外匹配 PID 与端口。
    Text {
        atom: Atom,
        number: Option<u32>,
        plain: Option<Vec<char>>,
    },
}

/// 端口、PID 精确命中的得分，压过任何模糊匹配。
const EXACT: u32 = 10_000;
/// 拼写容错命中名称/描述时每个字符的得分：略高于散落的模糊匹配，
/// 让 `steem` 先找到 steam，而不是路径里凑巧拼出 s-t-e-e-m 的进程。
const TYPO_PER_CHAR: u32 = 22;
/// 模糊匹配的质量下限（每个字符的平均原始得分）。完全散落的匹配每字符只有约 16 分基础分，
/// 连续或落在词首会有加分；低于这个值的视为凑巧（`vsc` 散落命中 "Node.js JavaScript"）。
const MIN_PER_CHAR: u32 = 20;
/// 得分低于最佳结果这个比例的匹配视为噪音丢掉（例如 `vite` 散落命中 "Visual Studio Code"）。
const KEEP_RATIO: f32 = 0.35;

struct Searcher {
    terms: Vec<Term>,
    matcher: Matcher,
    buf: Vec<char>,
}

impl Searcher {
    fn new(filter: &str) -> Self {
        let terms = filter
            .split_whitespace()
            .map(
                |raw| match raw.strip_prefix(':').and_then(|n| n.parse::<u16>().ok()) {
                    Some(port) => Term::Port(port),
                    None => {
                        let atom = Atom::parse(raw, CaseMatching::Ignore, Normalization::Smart);
                        let fuzzy = atom.kind == AtomKind::Fuzzy && !atom.negative;
                        Term::Text {
                            number: raw.parse().ok(),
                            // 拼写容错只对普通的模糊词、且足够长时启用，否则误命中太多。
                            plain: (fuzzy && raw.chars().count() >= 5)
                                .then(|| raw.to_lowercase().chars().collect()),
                            atom,
                        }
                    }
                },
            )
            .collect();
        Self {
            terms,
            matcher: Matcher::new(Config::DEFAULT),
            buf: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// 所有词都要命中（可以落在不同字段）；返回各词最佳得分之和，越大越相关。
    /// 字段权重：名称 > 描述 > 命令行 > 路径。
    fn score(&mut self, p: &Process) -> Option<u32> {
        // pulse 自己的命令行里可能就带着搜索词（`pulse :5173`），不参与匹配。
        let cmd = if p.pid == std::process::id() {
            ""
        } else {
            p.command_line.as_deref().unwrap_or("")
        };
        let fields: [(&str, u32); 4] = [
            (&p.name, 100),
            (p.description.as_deref().unwrap_or(""), 90),
            (cmd, 60),
            (p.path.as_deref().unwrap_or(""), 40),
        ];
        let has_port = |n: u32| n <= u16::MAX as u32 && p.ports.iter().any(|x| x.port == n as u16);
        let mut total = 0;
        for term in &self.terms {
            total += match term {
                Term::Port(port) => has_port(*port as u32).then_some(EXACT)?,
                Term::Text {
                    number: Some(n), ..
                } if *n == p.pid || has_port(*n) => EXACT,
                Term::Text { atom, plain, .. } => {
                    if atom.negative {
                        // 排除词：任一字段命中就排除。
                        for (field, _) in fields {
                            atom.score(Utf32Str::new(field, &mut self.buf), &mut self.matcher)?;
                        }
                        0
                    } else {
                        let floor = if atom.kind == AtomKind::Fuzzy {
                            MIN_PER_CHAR * atom.needle_text().len() as u32
                        } else {
                            0
                        };
                        let best = fields
                            .iter()
                            .filter_map(|(field, weight)| {
                                let raw = atom
                                    .score(Utf32Str::new(field, &mut self.buf), &mut self.matcher)?
                                    as u32;
                                (raw >= floor).then_some(raw * weight / 100)
                            })
                            .max();
                        let typo = plain
                            .as_ref()
                            .filter(|needle| fields[..2].iter().any(|(f, _)| near_word(needle, f)))
                            .map(|needle| TYPO_PER_CHAR * needle.len() as u32);
                        best.max(typo)?
                    }
                }
            };
        }
        Some(total)
    }
}

/// 拼写容错：needle 与 hay 中某个单词（或其等长前缀）的编辑距离不超过 1，
/// 相邻字母颠倒也算一次（`chorme` → chrome）。
fn near_word(needle: &[char], hay: &str) -> bool {
    let hay = hay.to_lowercase();
    hay.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .any(|w| {
            let word: Vec<char> = w.chars().collect();
            let prefix = &word[..word.len().min(needle.len())];
            osa_distance(needle, &word) <= 1 || osa_distance(needle, prefix) <= 1
        })
}

/// Optimal string alignment 距离（Levenshtein + 相邻换位）。
fn osa_distance(a: &[char], b: &[char]) -> usize {
    if a.len().abs_diff(b.len()) > 1 {
        return 2;
    }
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

pub fn build(snap: &Snapshot, q: &Query) -> Table {
    let procs = &snap.processes;
    let mut searcher = Searcher::new(q.filter);
    let searching = !searcher.is_empty();
    let mut scores = vec![0u32; procs.len()];
    let mut visible: Vec<usize> = (0..procs.len())
        .filter(|&i| {
            let p = &procs[i];
            if q.listening_only
                && !p
                    .ports
                    .iter()
                    .any(|x| x.proto == pulse_probe::ports::Proto::Tcp)
            {
                return false;
            }
            match searcher.score(p) {
                Some(s) => {
                    scores[i] = s;
                    true
                }
                None => false,
            }
        })
        .collect();
    if searching {
        let best = visible.iter().map(|&i| scores[i]).max().unwrap_or(0);
        visible.retain(|&i| scores[i] as f32 >= best as f32 * KEEP_RATIO);
    }
    // 排序用的档位：得分粗略分桶后取反（越小越靠前），同档内按当前排序列，避免分数微小差异打乱列排序。
    let rank: Vec<u32> = scores.iter().map(|&s| u32::MAX - s / 16).collect();

    let cmp = |a: (&str, u32, &Metrics), b: (&str, u32, &Metrics)| {
        let ord = match q.sort {
            SortKey::Name => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
            SortKey::Pid => a.1.cmp(&b.1),
            SortKey::Cpu => a.2.cpu.total_cmp(&b.2.cpu),
            SortKey::Memory => a.2.memory.cmp(&b.2.memory),
            SortKey::Gpu => a.2.gpu.total_cmp(&b.2.gpu),
            SortKey::Disk => a.2.disk.cmp(&b.2.disk),
            SortKey::Network => a.2.net().cmp(&b.2.net()),
            SortKey::CpuTime => a.2.cpu_time.cmp(&b.2.cpu_time),
            SortKey::GpuMemory => a.2.vram.cmp(&b.2.vram),
        };
        let ord = if q.descending { ord.reverse() } else { ord };
        // PID 兜底，相同值的行不会每秒乱跳。
        ord.then(a.1.cmp(&b.1))
    };
    let proc_key = |i: usize| (procs[i].name.as_str(), procs[i].pid, Metrics::of(&procs[i]));
    // 搜索时先按相关度分档，档内再按当前排序列。
    let sort_procs = |v: &mut Vec<usize>| {
        v.sort_by(|&a, &b| {
            let (ka, kb) = (proc_key(a), proc_key(b));
            rank[a]
                .cmp(&rank[b])
                .then_with(|| cmp((ka.0, ka.1, &ka.2), (kb.0, kb.1, &kb.2)))
        });
    };

    if !q.grouped {
        let mut v = visible;
        sort_procs(&mut v);
        return Table {
            groups: Vec::new(),
            rows: v.into_iter().map(Row::Single).collect(),
        };
    }

    let mut by_key: HashMap<String, usize> = HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    for i in visible {
        let key = procs[i].name.to_lowercase();
        let gi = *by_key.entry(key.clone()).or_insert_with(|| {
            groups.push(Group {
                name: procs[i].name.clone(),
                key,
                members: Vec::new(),
                metrics: Metrics::default(),
            });
            groups.len() - 1
        });
        groups[gi].metrics.add(&Metrics::of(&procs[i]));
        groups[gi].members.push(i);
    }
    for g in &mut groups {
        sort_procs(&mut g.members);
    }

    let group_pid = |g: &Group| procs[g.members[0]].pid;
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by(|&a, &b| {
        let (ga, gb) = (&groups[a], &groups[b]);
        // 单成员分组按进程自身的值比较，与多成员分组混排。
        let ka = if ga.members.len() == 1 {
            proc_key(ga.members[0])
        } else {
            (ga.name.as_str(), group_pid(ga), ga.metrics)
        };
        let kb = if gb.members.len() == 1 {
            proc_key(gb.members[0])
        } else {
            (gb.name.as_str(), group_pid(gb), gb.metrics)
        };
        // 成员已按相关度排好，第一个就是该组最好的一档。
        let (ta, tb) = (rank[ga.members[0]], rank[gb.members[0]]);
        ta.cmp(&tb)
            .then_with(|| cmp((ka.0, ka.1, &ka.2), (kb.0, kb.1, &kb.2)))
    });

    let mut rows = Vec::with_capacity(procs.len());
    for gi in order {
        let g = &groups[gi];
        if g.members.len() == 1 {
            rows.push(Row::Single(g.members[0]));
            continue;
        }
        rows.push(Row::Group(gi));
        // 搜索时自动展开，方便直接看到匹配的进程。
        if searching || q.expanded.contains(&g.key) {
            rows.extend(g.members.iter().map(|&i| Row::Member(i)));
        }
    }
    Table { groups, rows }
}

/// 进程及其所有后代的 PID（用创建时间排除 PID 复用造成的假父子关系）。
pub fn descendants(snap: &Snapshot, root: u32) -> Vec<u32> {
    let procs = &snap.processes;
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let parent = out[i];
        let parent_ct = procs
            .iter()
            .find(|p| p.pid == parent)
            .map_or(0, |p| p.create_time);
        for p in procs {
            if p.parent_pid == parent
                && p.pid != parent
                && p.create_time >= parent_ct
                && !out.contains(&p.pid)
            {
                out.push(p.pid);
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_probe::ports::{Port, Proto};
    use std::sync::Arc;

    fn proc(pid: u32, name: &str, desc: Option<&str>, cmd: Option<&str>, ports: &[u16]) -> Process {
        Process {
            pid,
            parent_pid: 0,
            name: name.into(),
            path: Some(Arc::from(format!(r"C:\Apps\{name}"))),
            description: desc.map(Arc::from),
            command_line: cmd.map(Arc::from),
            ports: ports
                .iter()
                .map(|&port| Port {
                    proto: Proto::Tcp,
                    port,
                })
                .collect(),
            cpu: 0.0,
            private_ws: 0,
            working_set: 0,
            io_rate: 0,
            gpu: 0.0,
            gpu_dedicated: 0,
            gpu_shared: 0,
            user_time: 0,
            kernel_time: 0,
            net_rx: 0,
            net_tx: 0,
            threads: 1,
            handles: 1,
            session_id: 1,
            create_time: 0,
        }
    }

    fn search(procs: Vec<Process>, filter: &str) -> Vec<u32> {
        let snap = Snapshot {
            processes: procs,
            ..Default::default()
        };
        let expanded = HashSet::new();
        let q = Query {
            filter,
            grouped: false,
            sort: SortKey::Pid,
            descending: false,
            expanded: &expanded,
            listening_only: false,
        };
        build(&snap, &q)
            .rows
            .iter()
            .map(|r| match r {
                Row::Single(i) | Row::Member(i) => snap.processes[*i].pid,
                Row::Group(_) => 0,
            })
            .collect()
    }

    fn sample() -> Vec<Process> {
        vec![
            proc(100, "Code.exe", Some("Visual Studio Code"), None, &[]),
            proc(
                200,
                "node.exe",
                Some("Node.js JavaScript Runtime"),
                Some("node ./node_modules/vite/bin/vite.js"),
                &[5173],
            ),
            proc(300, "Weixin.exe", Some("微信"), None, &[]),
            proc(5173, "svchost.exe", None, None, &[135]),
            proc(400, "chrome.exe", Some("Google Chrome"), None, &[]),
        ]
    }

    #[test]
    fn description_and_chinese() {
        assert_eq!(search(sample(), "visual"), vec![100]);
        assert_eq!(search(sample(), "微信"), vec![300]);
    }

    #[test]
    fn fuzzy() {
        assert_eq!(search(sample(), "vscode"), vec![100]);
        assert_eq!(search(sample(), "vsc"), vec![100]);
        assert_eq!(search(sample(), "chrm"), vec![400]);
        // 散落命中的弱匹配被相对阈值过滤，只留下命令行里连续命中的 vite。
        assert_eq!(search(sample(), "vite"), vec![200]);
    }

    #[test]
    fn typo_tolerance() {
        assert_eq!(search(sample(), "chorme"), vec![400]);
        assert_eq!(search(sample(), "wexin"), vec![300]);
    }

    #[test]
    fn command_line_and_multi_term() {
        assert_eq!(search(sample(), "node vite"), vec![200]);
        assert!(search(sample(), "node weixin").is_empty());
    }

    #[test]
    fn fzf_syntax() {
        assert!(!search(sample(), "!svchost").contains(&5173));
        assert_eq!(search(sample(), "^code"), vec![100]);
    }

    #[test]
    fn ports() {
        // 冒号前缀只匹配端口；纯数字同时匹配 PID 与端口，精确命中都排在前面。
        assert_eq!(search(sample(), ":5173"), vec![200]);
        assert_eq!(search(sample(), "5173"), vec![200, 5173]);
        assert_eq!(search(sample(), ":135"), vec![5173]);
    }
}
