//! 统计页的数据：从使用记录库里按日、周、月、年、最近 N 天、自选区间汇总各程序的使用时长。
//!
//! 只读连接，查询都在毫秒级（按天的汇总表很小，单日明细有分钟索引），在界面线程里直接查，结果缓存。

use std::collections::HashMap;

use rusqlite::{Connection, OpenFlags};

/// 日期：yyyymmdd 整数与 (年, 月, 日) 互转，以及按天加减（公历，与时区无关）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub y: i32,
    pub m: u32,
    pub d: u32,
}

impl Date {
    pub fn today() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        Self::from_key(pulse_probe::os::local_day(now))
    }

    pub fn from_key(k: i64) -> Self {
        Self {
            y: (k / 10_000) as i32,
            m: (k / 100 % 100) as u32,
            d: (k % 100) as u32,
        }
    }

    /// yyyymmdd，与 `usage_day.day` 相同。
    pub fn key(self) -> i64 {
        self.y as i64 * 10_000 + self.m as i64 * 100 + self.d as i64
    }

    /// 自 1970-01-01 起的天数（Howard Hinnant 的 days_from_civil）。
    fn days(self) -> i64 {
        let y = self.y as i64 - (self.m <= 2) as i64;
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let m = self.m as i64;
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + self.d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    fn from_days(z: i64) -> Self {
        let z = z + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let y = (yoe + era * 400 + (m <= 2) as i64) as i32;
        Self { y, m, d }
    }

    pub fn add_days(self, n: i64) -> Self {
        Self::from_days(self.days() + n)
    }

    /// 星期几：0 = 周一 … 6 = 周日。
    pub fn weekday(self) -> u32 {
        // 1970-01-01 是周四。
        (self.days() + 3).rem_euclid(7) as u32
    }

    pub fn month_start(self) -> Self {
        Self { d: 1, ..self }
    }

    pub fn add_months(self, n: i32) -> Self {
        let total = self.y * 12 + self.m as i32 - 1 + n;
        Self {
            y: total.div_euclid(12),
            m: (total.rem_euclid(12) + 1) as u32,
            d: 1,
        }
    }

    pub fn days_until(self, other: Self) -> i64 {
        other.days() - self.days()
    }

    /// 本地零点对应的分钟数（Unix 时间 / 60）。
    pub fn minute(self) -> i64 {
        pulse_probe::os::local_midnight(self.y, self.m, self.d).unwrap_or(0) / 60
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    /// 包含锚点的自然日、周、月、年。
    Day,
    Week,
    Month,
    Year,
    /// 截止到现在的最近 24 小时、7 天、30 天、365 天（含今天）。
    Last24h,
    Last7,
    Last30,
    Last365,
    All,
    /// 日历上选的区间 [起, 止)。
    Custom(Date, Date),
}

/// 图表里一列代表多长时间。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grain {
    FiveMin,
    Day,
    Week,
    Month,
}

/// 区间实际覆盖的日期 [start, end)；最近 24 小时还有精确到分钟的 [起, 止)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: Date,
    pub end: Date,
    pub minutes: Option<(i64, i64)>,
}

impl Span {
    fn days(start: Date, end: Date) -> Self {
        Self {
            start,
            end,
            minutes: None,
        }
    }

    pub fn len_days(&self) -> i64 {
        self.start.days_until(self.end)
    }

    /// 按分钟的 [起, 止)。
    pub fn minute_bounds(&self) -> (i64, i64) {
        self.minutes
            .unwrap_or((self.start.minute(), self.end.minute()))
    }
}

impl Range {
    /// 自然区间（可前后翻页）。
    pub const PERIODS: [Range; 5] = [
        Range::Day,
        Range::Week,
        Range::Month,
        Range::Year,
        Range::All,
    ];
    /// 截止到现在的滚动区间。
    pub const ROLLING: [Range; 4] = [Range::Last24h, Range::Last7, Range::Last30, Range::Last365];

    pub fn span(self, anchor: Date, first: Option<Date>) -> Span {
        let today = Date::today();
        let tomorrow = today.add_days(1);
        match self {
            Range::Day => Span::days(anchor, anchor.add_days(1)),
            Range::Week => {
                let start = anchor.add_days(-(anchor.weekday() as i64));
                Span::days(start, start.add_days(7))
            }
            Range::Month => Span::days(anchor.month_start(), anchor.month_start().add_months(1)),
            Range::Year => {
                let start = Date {
                    y: anchor.y,
                    m: 1,
                    d: 1,
                };
                Span::days(start, start.add_months(12))
            }
            Range::Last24h => {
                // 截止到下一个 5 分钟整点，图表的每个点正好对齐 5 分钟。
                let end = (now_minute() + 4) / 5 * 5;
                let start = end - 24 * 60;
                Span {
                    start: Date::from_key(pulse_probe::os::local_day(start * 60)),
                    end: tomorrow,
                    minutes: Some((start, end)),
                }
            }
            Range::Last7 => Span::days(today.add_days(-6), tomorrow),
            Range::Last30 => Span::days(today.add_days(-29), tomorrow),
            Range::Last365 => Span::days(today.add_days(-364), tomorrow),
            Range::All => Span::days(first.unwrap_or(anchor).month_start(), tomorrow),
            Range::Custom(a, b) => Span::days(a, b),
        }
    }

    /// 图表一列的粒度：列数大致在 7–60 之间。
    pub fn grain(self, span: &Span) -> Grain {
        match self {
            Range::Day | Range::Last24h => Grain::FiveMin,
            Range::Week | Range::Month | Range::Last7 | Range::Last30 => Grain::Day,
            Range::Last365 => Grain::Week,
            Range::Year | Range::All => Grain::Month,
            Range::Custom(..) => match span.len_days() {
                ..=1 => Grain::FiveMin,
                2..=62 => Grain::Day,
                63..=420 => Grain::Week,
                _ => Grain::Month,
            },
        }
    }

    /// 能否前后翻页：截止到现在的区间与"全部"不能。
    pub fn steppable(self) -> bool {
        matches!(
            self,
            Range::Day | Range::Week | Range::Month | Range::Year | Range::Custom(..)
        )
    }

    /// 上一个 / 下一个区间：返回新的区间与锚点。
    pub fn step(self, anchor: Date, dir: i32) -> (Range, Date) {
        match self {
            Range::Day => (self, anchor.add_days(dir as i64)),
            Range::Week => (self, anchor.add_days(7 * dir as i64)),
            Range::Month => (self, anchor.month_start().add_months(dir)),
            Range::Year => (self, anchor.month_start().add_months(12 * dir)),
            Range::Custom(a, b) => {
                let n = a.days_until(b) * dir as i64;
                (Range::Custom(a.add_days(n), b.add_days(n)), anchor)
            }
            _ => (self, anchor),
        }
    }
}

fn now_minute() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64 / 60)
}

#[derive(Clone, Debug)]
pub struct App {
    /// 小写 exe 文件名，与正在运行的进程对照。
    pub key: String,
    pub name: String,
    pub path: Option<std::sync::Arc<str>>,
}

/// 某程序在区间里的合计。
#[derive(Clone, Debug)]
pub struct Total {
    pub app: i64,
    pub active: i64,
    pub typing: i64,
}

/// 某程序在区间里收、发的字节数。
#[derive(Clone, Debug)]
pub struct Traffic {
    pub app: i64,
    pub rx: i64,
    pub tx: i64,
}

impl Traffic {
    pub fn total(&self) -> i64 {
        self.rx + self.tx
    }
}

/// 图表的一列：按程序分的秒数（日视图里一列是 5 分钟，其他视图一列是一天或一个月）。
#[derive(Clone, Debug, Default)]
pub struct Column {
    pub parts: Vec<(i64, i64)>,
}

impl Column {
    pub fn total(&self) -> i64 {
        self.parts.iter().map(|p| p.1).sum()
    }

    /// 秒数最多的程序。
    pub fn top(&self) -> Option<i64> {
        self.parts.iter().max_by_key(|p| p.1).map(|p| p.0)
    }
}

#[derive(Clone, Debug)]
pub struct Data {
    pub span: Span,
    pub grain: Grain,
    pub totals: Vec<Total>,
    /// 每 5 分钟一列（共 288 列，从 `span.minute_bounds().0` 起），或每天、每周、每月一列。
    pub columns: Vec<Column>,
    /// 按天以上的粒度时，每列的起始日期（第一列可能早于 `span.start`，如月粒度时是那个月的 1 日）。
    pub buckets: Vec<Date>,
    /// 有记录的天数（算日均用）。
    pub days_with_data: i64,
    /// 热力图：最近 53 周每天各程序的活跃秒数 (日期键, 程序, 秒)。
    pub heat: Vec<(i64, i64, i64)>,
    /// 各程序的网络流量，多的在前。
    pub traffic: Vec<Traffic>,
    pub first: Option<Date>,
}

impl Data {
    pub fn active(&self) -> i64 {
        self.totals.iter().map(|t| t.active).sum()
    }

    pub fn typing(&self) -> i64 {
        self.totals.iter().map(|t| t.typing).sum()
    }

    /// 只留 `keep` 为真的程序。
    pub fn filtered(&self, keep: impl Fn(i64) -> bool) -> Data {
        let mut d = self.clone();
        d.totals.retain(|t| keep(t.app));
        for c in &mut d.columns {
            c.parts.retain(|p| keep(p.0));
        }
        d.heat.retain(|h| keep(h.1));
        d.traffic.retain(|t| keep(t.app));
        d
    }

    /// 热力图用：每天的合计秒数。
    pub fn heat_by_day(&self) -> HashMap<i64, i64> {
        let mut m = HashMap::new();
        for &(day, _, secs) in &self.heat {
            *m.entry(day).or_default() += secs;
        }
        m
    }
}

pub struct Store {
    conn: Option<Connection>,
    pub apps: HashMap<i64, App>,
}

impl Store {
    pub fn open() -> Self {
        let conn = crate::history::path().and_then(|p| {
            Connection::open_with_flags(
                p,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .ok()
        });
        Self {
            conn,
            apps: HashMap::new(),
        }
    }

    pub fn available(&self) -> bool {
        self.conn.is_some()
    }

    /// 程序名单：显示名优先用 exe 的文件描述（Code → Visual Studio Code）。
    fn load_apps(&mut self, conn: &Connection) -> rusqlite::Result<()> {
        let mut stmt = conn.prepare_cached("SELECT id, key, name, path FROM apps")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (id, key, name, path) = row?;
            let known = self
                .apps
                .get(&id)
                .is_some_and(|a| a.path.as_deref() == path.as_deref());
            if known {
                continue;
            }
            let pretty = path
                .as_deref()
                .and_then(|p| display_name(&name, p))
                .unwrap_or(name);
            self.apps.insert(
                id,
                App {
                    key,
                    name: pretty,
                    path: path.map(std::sync::Arc::from),
                },
            );
        }
        Ok(())
    }

    /// 有些旧记录只有程序名、没有路径；同名程序正在运行时借用它的路径，就有了图标和友好的名称。
    pub fn fill_paths(&mut self, procs: &[pulse_probe::Process]) {
        let mut running: HashMap<String, &std::sync::Arc<str>> = HashMap::new();
        for p in procs {
            if let Some(path) = &p.path {
                let stem = p.name.strip_suffix(".exe").unwrap_or(&p.name);
                running.entry(stem.to_lowercase()).or_insert(path);
            }
        }
        for app in self.apps.values_mut().filter(|a| a.path.is_none()) {
            if let Some(path) = running.get(&app.key) {
                if let Some(d) = display_name(&app.name, path) {
                    app.name = d;
                }
                app.path = Some((*path).clone());
            }
        }
    }

    pub fn query(&mut self, range: Range, anchor: Date) -> rusqlite::Result<Option<Data>> {
        let Some(conn) = self.conn.take() else {
            return Ok(None);
        };
        let result = self.query_with(&conn, range, anchor);
        self.conn = Some(conn);
        result.map(Some)
    }

    fn query_with(
        &mut self,
        conn: &Connection,
        range: Range,
        anchor: Date,
    ) -> rusqlite::Result<Data> {
        self.load_apps(conn)?;
        let first: Option<i64> =
            conn.query_row("SELECT MIN(day) FROM usage_day", [], |r| r.get(0))?;
        let first = first.map(Date::from_key);
        let span = range.span(anchor, first);
        let grain = range.grain(&span);
        let (start, end) = (span.start, span.end);

        let mut totals = Vec::new();
        {
            // 最近 24 小时不是整天，按分钟明细汇总；其余按天的汇总表。
            let (sql, bounds) = match span.minutes {
                Some((a, b)) => (
                    "SELECT app, SUM(active), SUM(typing) FROM usage
                     WHERE minute >= ?1 AND minute < ?2 GROUP BY app ORDER BY 2 DESC",
                    [a, b],
                ),
                None => (
                    "SELECT app, SUM(active), SUM(typing) FROM usage_day
                     WHERE day >= ?1 AND day < ?2 GROUP BY app ORDER BY 2 DESC",
                    [start.key(), end.key()],
                ),
            };
            let mut stmt = conn.prepare_cached(sql)?;
            let rows = stmt.query_map(bounds, |r| {
                Ok(Total {
                    app: r.get(0)?,
                    active: r.get(1)?,
                    typing: r.get(2)?,
                })
            })?;
            for t in rows {
                totals.push(t?);
            }
        }
        let days_with_data: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT day) FROM usage_day WHERE day >= ?1 AND day < ?2",
            [start.key(), end.key()],
            |r| r.get(0),
        )?;

        let mut buckets = Vec::new();
        let columns = if grain == Grain::FiveMin {
            let (a, b) = span.minute_bounds();
            let slots = ((b - a) / 5).clamp(1, 300) as usize;
            let mut cols = vec![HashMap::<i64, i64>::new(); slots];
            let mut stmt = conn.prepare_cached(
                "SELECT minute, app, SUM(active) FROM usage
                 WHERE minute >= ?1 AND minute < ?2 GROUP BY minute, app",
            )?;
            let rows = stmt.query_map([a, b], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?;
            for row in rows {
                let (minute, app, secs) = row?;
                let i = ((minute - a) / 5) as usize;
                if let Some(c) = cols.get_mut(i) {
                    *c.entry(app).or_default() += secs;
                }
            }
            cols.into_iter().map(to_column).collect()
        } else {
            let mut b = match grain {
                Grain::Month => start.month_start(),
                _ => start,
            };
            while b < end {
                buckets.push(b);
                b = match grain {
                    Grain::Month => b.add_months(1),
                    Grain::Week => b.add_days(7),
                    _ => b.add_days(1),
                };
            }
            let mut cols = vec![HashMap::<i64, i64>::new(); buckets.len().max(1)];
            let mut stmt = conn.prepare_cached(
                "SELECT day, app, active FROM usage_day WHERE day >= ?1 AND day < ?2",
            )?;
            let rows = stmt.query_map([start.key(), end.key()], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?;
            for row in rows {
                let (day, app, secs) = row?;
                let d = Date::from_key(day);
                let i = buckets.partition_point(|&b| b <= d).saturating_sub(1);
                if let Some(c) = cols.get_mut(i) {
                    *c.entry(app).or_default() += secs;
                }
            }
            cols.into_iter().map(to_column).collect()
        };

        // 热力图：截至 anchor 所在周的最近 53 周。
        let heat_end = Date::today().add_days(1);
        let heat_start = heat_end.add_days(-(53 * 7 + 7));
        let mut heat = Vec::new();
        {
            let mut stmt = conn.prepare_cached(
                "SELECT day, app, active FROM usage_day WHERE day >= ?1 AND day < ?2",
            )?;
            let rows = stmt.query_map([heat_start.key(), heat_end.key()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            for row in rows {
                heat.push(row?);
            }
        }

        // 流量表是新加的：旧版本建的库在新版记录线程打开前还没有这张表，查不到就当没有。
        let traffic = traffic(conn, &span).unwrap_or_default();

        Ok(Data {
            span,
            grain,
            totals,
            columns,
            buckets,
            days_with_data,
            heat,
            traffic,
            first,
        })
    }
}

/// 区间内各程序的流量：最近 24 小时按分钟明细，其余按天的汇总。
fn traffic(conn: &Connection, span: &Span) -> rusqlite::Result<Vec<Traffic>> {
    let (sql, bounds) = match span.minutes {
        Some((a, b)) => (
            "SELECT app, SUM(rx), SUM(tx) FROM net
             WHERE minute >= ?1 AND minute < ?2 GROUP BY app ORDER BY SUM(rx) + SUM(tx) DESC",
            [a, b],
        ),
        None => (
            "SELECT app, SUM(rx), SUM(tx) FROM net_day
             WHERE day >= ?1 AND day < ?2 GROUP BY app ORDER BY SUM(rx) + SUM(tx) DESC",
            [span.start.key(), span.end.key()],
        ),
    };
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(bounds, |r| {
        Ok(Traffic {
            app: r.get(0)?,
            rx: r.get(1)?,
            tx: r.get(2)?,
        })
    })?;
    rows.collect()
}

/// 显示名：用 exe 的文件描述（Code.exe → "Visual Studio Code"）。
/// exe 名本身是中日文等非 ASCII 文字（游戏常这样命名）、或描述很长（多是引擎名，
/// 如 "TVP(KIRIKIRI) Z core / Scripting Platform for Win32"）时，保留 exe 名。
fn display_name(stem: &str, path: &str) -> Option<String> {
    if !stem.is_ascii() {
        return None;
    }
    pulse_probe::os::version_strings(path, &["FileDescription"])
        .into_iter()
        .next()
        .flatten()
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty() && d.chars().count() <= 40)
}

fn to_column(m: HashMap<i64, i64>) -> Column {
    let mut parts: Vec<(i64, i64)> = m.into_iter().filter(|p| p.1 > 0).collect();
    parts.sort_by_key(|p| std::cmp::Reverse(p.1));
    Column { parts }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_arithmetic() {
        let d = Date::from_key(20261013);
        assert_eq!(d.key(), 20261013);
        assert_eq!(d.weekday(), 1); // 2026-10-13 是周二
        assert_eq!(d.add_days(19).key(), 20261101);
        assert_eq!(d.add_days(-13).key(), 20260930);
        assert_eq!(Date::from_key(20240228).add_days(1).key(), 20240229);
        assert_eq!(d.add_months(3).key(), 20270101);
        assert_eq!(d.add_months(-10).key(), 20251201);
        assert_eq!(Date::from_key(20261001).days_until(d), 12);
    }

    #[test]
    fn range_spans() {
        let d = Date::from_key(20261013);
        let keys = |r: Range| {
            let s = r.span(d, None);
            (s.start.key(), s.end.key())
        };
        assert_eq!(keys(Range::Week), (20261012, 20261019));
        assert_eq!(keys(Range::Month), (20261001, 20261101));
        assert_eq!(keys(Range::Year), (20260101, 20270101));
        assert_eq!(Range::Month.step(d, -1).1.key(), 20260901);
        assert_eq!(Range::Year.step(d, 1).1.y, 2027);
        let custom = Range::Custom(Date::from_key(20260901), Date::from_key(20260911));
        assert_eq!(
            custom.step(d, -1).0,
            Range::Custom(Date::from_key(20260822), Date::from_key(20260901))
        );
        // 最近 N 天含今天。
        let s = Range::Last7.span(d, None);
        assert_eq!(s.len_days(), 7);
        assert_eq!(s.end, Date::today().add_days(1));
        let s = Range::Last24h.span(d, None);
        let (a, b) = s.minutes.unwrap();
        assert_eq!((b - a, b % 5), (1440, 0));
    }

    #[test]
    fn grains() {
        let d = Date::from_key(20261013);
        let custom = |a: i64, b: i64| Range::Custom(Date::from_key(a), Date::from_key(b));
        let g = |r: Range| r.grain(&r.span(d, None));
        assert_eq!(g(Range::Last24h), Grain::FiveMin);
        assert_eq!(g(Range::Last30), Grain::Day);
        assert_eq!(g(Range::Last365), Grain::Week);
        assert_eq!(g(Range::Year), Grain::Month);
        assert_eq!(g(custom(20261013, 20261014)), Grain::FiveMin);
        assert_eq!(g(custom(20260901, 20261014)), Grain::Day);
        assert_eq!(g(custom(20250101, 20261014)), Grain::Month);
    }

    #[test]
    fn local_midnight_is_a_local_day_start() {
        let d = Date::from_key(20261013);
        let m = d.minute();
        assert_eq!(pulse_probe::os::local_day(m * 60), 20261013);
        assert_eq!(pulse_probe::os::local_day(m * 60 - 1), 20261012);
    }
}
