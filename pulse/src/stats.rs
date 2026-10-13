//! 统计页的数据：从使用记录库里按日、周、月、全部汇总各程序的使用时长。
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
    Day,
    Week,
    Month,
    All,
}

impl Range {
    pub const ALL: [Range; 4] = [Range::Day, Range::Week, Range::Month, Range::All];

    /// 包含 `anchor` 的区间 [起, 止)。
    pub fn bounds(self, anchor: Date, first: Option<Date>) -> (Date, Date) {
        match self {
            Range::Day => (anchor, anchor.add_days(1)),
            Range::Week => {
                let start = anchor.add_days(-(anchor.weekday() as i64));
                (start, start.add_days(7))
            }
            Range::Month => (anchor.month_start(), anchor.month_start().add_months(1)),
            Range::All => (
                first.unwrap_or(anchor).month_start(),
                Date::today().add_days(1),
            ),
        }
    }

    /// 上一个 / 下一个区间的锚点。
    pub fn step(self, anchor: Date, dir: i32) -> Date {
        match self {
            Range::Day => anchor.add_days(dir as i64),
            Range::Week => anchor.add_days(7 * dir as i64),
            Range::Month => anchor.month_start().add_months(dir),
            Range::All => anchor,
        }
    }
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
    pub range: Range,
    pub start: Date,
    pub totals: Vec<Total>,
    /// 日视图：288 个 5 分钟；周、月视图：每天一列；全部：每月一列。
    pub columns: Vec<Column>,
    /// 有记录的天数（算日均用）。
    pub days_with_data: i64,
    /// 热力图：最近 53 周每天的活跃秒数。
    pub heat: HashMap<i64, i64>,
    pub first: Option<Date>,
}

impl Data {
    pub fn active(&self) -> i64 {
        self.totals.iter().map(|t| t.active).sum()
    }

    pub fn typing(&self) -> i64 {
        self.totals.iter().map(|t| t.typing).sum()
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
        let (start, end) = range.bounds(anchor, first);

        let mut totals = Vec::new();
        {
            let mut stmt = conn.prepare_cached(
                "SELECT app, SUM(active), SUM(typing) FROM usage_day
                 WHERE day >= ?1 AND day < ?2 GROUP BY app ORDER BY 2 DESC",
            )?;
            let rows = stmt.query_map([start.key(), end.key()], |r| {
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

        let columns = match range {
            Range::Day => {
                let (a, b) = (start.minute(), end.minute());
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
            }
            Range::Week | Range::Month | Range::All => {
                let by_month = range == Range::All;
                let n = if by_month {
                    let months = (end.y - start.y) * 12 + end.m as i32 - start.m as i32 + 1;
                    months.max(1) as usize
                } else {
                    start.days_until(end).max(1) as usize
                };
                let mut cols = vec![HashMap::<i64, i64>::new(); n];
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
                    let i = if by_month {
                        ((d.y - start.y) * 12 + d.m as i32 - start.m as i32) as usize
                    } else {
                        start.days_until(d) as usize
                    };
                    if let Some(c) = cols.get_mut(i) {
                        *c.entry(app).or_default() += secs;
                    }
                }
                cols.into_iter().map(to_column).collect()
            }
        };

        // 热力图：截至 anchor 所在周的最近 53 周。
        let heat_end = Date::today().add_days(1);
        let heat_start = heat_end.add_days(-(53 * 7 + 7));
        let mut heat = HashMap::new();
        {
            let mut stmt = conn.prepare_cached(
                "SELECT day, SUM(active) FROM usage_day WHERE day >= ?1 AND day < ?2 GROUP BY day",
            )?;
            let rows = stmt.query_map([heat_start.key(), heat_end.key()], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (day, secs) = row?;
                heat.insert(day, secs);
            }
        }

        Ok(Data {
            range,
            start,
            totals,
            columns,
            days_with_data,
            heat,
            first,
        })
    }
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
    fn range_bounds() {
        let d = Date::from_key(20261013);
        let (a, b) = Range::Week.bounds(d, None);
        assert_eq!((a.key(), b.key()), (20261012, 20261019));
        let (a, b) = Range::Month.bounds(d, None);
        assert_eq!((a.key(), b.key()), (20261001, 20261101));
        assert_eq!(Range::Month.step(d, -1).key(), 20260901);
    }

    #[test]
    fn local_midnight_is_a_local_day_start() {
        let d = Date::from_key(20261013);
        let m = d.minute();
        assert_eq!(pulse_probe::os::local_day(m * 60), 20261013);
        assert_eq!(pulse_probe::os::local_day(m * 60 - 1), 20261012);
    }
}
