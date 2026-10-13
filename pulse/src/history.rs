//! 使用记录与性能历史：本地 SQLite 数据库（`%LOCALAPPDATA%\Pulse\history.db`）。
//!
//! - `usage`：每分钟、每个程序、每个窗口标题在前台的活跃秒数，其中有键盘输入的秒数；
//! - `usage_day`：按本地日期汇总的同样数据，统计页按天、周、月查询时不必扫描明细；
//! - `perf` / `perf_top`：每分钟的整体占用，以及 CPU、内存、磁盘、网络、GPU 各自占用最高的进程。
//!
//! 数据只保存在本机。写入由记录线程独占一个连接；界面查询另开只读连接（WAL 模式下互不阻塞）。

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

/// 性能明细保留的天数；使用记录不清理（每天只有几百行）。
const PERF_DAYS: i64 = 30;
const PERF_TOP_DAYS: i64 = 7;

pub fn path() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
            .join("Pulse")
            .join("history.db"),
    )
}

/// 程序的标识：按 exe 文件名（不含扩展名、不区分大小写）归并，与 Active Time 的记录一致。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AppRef {
    /// 小写的 exe 文件名。
    pub key: String,
    /// 显示用的 exe 文件名。
    pub name: String,
    pub path: Option<String>,
}

impl AppRef {
    pub fn new(name: &str, path: Option<&str>) -> Self {
        Self {
            key: name.to_lowercase(),
            name: name.to_string(),
            path: path.map(str::to_string),
        }
    }
}

/// 一分钟内的累计：某程序某标题的活跃秒数与输入秒数。
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seconds {
    pub active: u32,
    pub typing: u32,
}

/// 一分钟的整体性能平均值。
#[derive(Default, Clone, Debug)]
pub struct PerfMinute {
    pub cpu: f32,
    pub mem: f32,
    pub gpu: f32,
    /// 字节 / 秒。
    pub disk: u64,
    pub net_rx: u64,
    pub net_tx: u64,
    /// (类别, 名次, 程序名, PID, 数值)。
    pub top: Vec<(TopKind, u8, String, u32, f64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopKind {
    Cpu = 0,
    Memory = 1,
    Disk = 2,
    Network = 3,
    Gpu = 4,
}

pub struct Db {
    conn: Connection,
    apps: HashMap<String, (i64, Option<String>)>,
    titles: HashMap<String, i64>,
}

impl Db {
    /// 打开（必要时创建）数据库。
    pub fn open() -> rusqlite::Result<Self> {
        let p = path().ok_or(rusqlite::Error::InvalidPath(PathBuf::new()))?;
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        Self::open_at(&p)
    }

    pub fn open_at(p: &std::path::Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(p)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS apps(
                 id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE, name TEXT NOT NULL, path TEXT);
             CREATE TABLE IF NOT EXISTS titles(id INTEGER PRIMARY KEY, text TEXT NOT NULL UNIQUE);
             CREATE TABLE IF NOT EXISTS usage(
                 minute INTEGER NOT NULL, app INTEGER NOT NULL, title INTEGER NOT NULL,
                 active INTEGER NOT NULL, typing INTEGER NOT NULL,
                 PRIMARY KEY(minute, app, title)) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS usage_day(
                 day INTEGER NOT NULL, app INTEGER NOT NULL,
                 active INTEGER NOT NULL, typing INTEGER NOT NULL,
                 PRIMARY KEY(day, app)) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS perf(
                 minute INTEGER PRIMARY KEY, cpu REAL NOT NULL, mem REAL NOT NULL, gpu REAL NOT NULL,
                 disk INTEGER NOT NULL, net_rx INTEGER NOT NULL, net_tx INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS perf_top(
                 minute INTEGER NOT NULL, kind INTEGER NOT NULL, rank INTEGER NOT NULL,
                 app INTEGER NOT NULL, pid INTEGER NOT NULL, value REAL NOT NULL,
                 PRIMARY KEY(minute, kind, rank)) WITHOUT ROWID;",
        )?;
        Ok(Self {
            conn,
            apps: HashMap::new(),
            titles: HashMap::new(),
        })
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    fn app_id(&mut self, app: &AppRef) -> rusqlite::Result<i64> {
        if let Some((id, path)) = self.apps.get(&app.key) {
            // 路径变了（程序更新、换了安装位置）时更新，统计页按它取图标。
            if app.path.is_some() && *path != app.path {
                self.conn.execute(
                    "UPDATE apps SET path = ?1 WHERE id = ?2",
                    params![app.path, id],
                )?;
                let id = *id;
                self.apps.insert(app.key.clone(), (id, app.path.clone()));
                return Ok(id);
            }
            return Ok(*id);
        }
        let row: Option<(i64, Option<String>)> = self
            .conn
            .query_row(
                "SELECT id, path FROM apps WHERE key = ?1",
                [&app.key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (id, path) = match row {
            Some((id, path)) => {
                if app.path.is_some() && path != app.path {
                    self.conn.execute(
                        "UPDATE apps SET path = ?1 WHERE id = ?2",
                        params![app.path, id],
                    )?;
                }
                (id, app.path.clone().or(path))
            }
            None => {
                self.conn.execute(
                    "INSERT INTO apps(key, name, path) VALUES (?1, ?2, ?3)",
                    params![app.key, app.name, app.path],
                )?;
                (self.conn.last_insert_rowid(), app.path.clone())
            }
        };
        self.apps.insert(app.key.clone(), (id, path));
        Ok(id)
    }

    /// 标题的编号；空标题是 0。
    fn title_id(&mut self, text: &str) -> rusqlite::Result<i64> {
        if text.is_empty() {
            return Ok(0);
        }
        if let Some(&id) = self.titles.get(text) {
            return Ok(id);
        }
        self.conn.execute(
            "INSERT INTO titles(text) VALUES (?1) ON CONFLICT(text) DO NOTHING",
            [text],
        )?;
        let id: i64 =
            self.conn
                .query_row("SELECT id FROM titles WHERE text = ?1", [text], |r| {
                    r.get(0)
                })?;
        // 标题种类可能很多（网页、文档名），缓存只留最近用到的一部分。
        if self.titles.len() > 4096 {
            self.titles.clear();
        }
        self.titles.insert(text.to_string(), id);
        Ok(id)
    }

    /// 写入一分钟的记录（一个事务）。`minute` 是 Unix 时间 / 60。
    pub fn write_minute(
        &mut self,
        minute: i64,
        usage: &HashMap<(AppRef, String), Seconds>,
        perf: Option<&PerfMinute>,
    ) -> rusqlite::Result<()> {
        let day = pulse_probe::os::local_day(minute * 60);
        // 先在事务外解析编号（会写 apps / titles），再一次性写明细。
        let mut rows = Vec::with_capacity(usage.len());
        for ((app, title), s) in usage {
            rows.push((self.app_id(app)?, self.title_id(title)?, *s));
        }
        let mut tops = Vec::new();
        if let Some(p) = perf {
            for (kind, rank, name, pid, value) in &p.top {
                let id = self.app_id(&AppRef::new(name, None))?;
                tops.push((*kind as i64, *rank, id, *pid, *value));
            }
        }
        // 保存点：单独调用时就是一个事务，导入时嵌套在外层事务里也能用。
        let tx = self.conn.savepoint()?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT INTO usage(minute, app, title, active, typing) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(minute, app, title) DO UPDATE SET
                     active = active + excluded.active, typing = typing + excluded.typing",
            )?;
            let mut day_ins = tx.prepare_cached(
                "INSERT INTO usage_day(day, app, active, typing) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(day, app) DO UPDATE SET
                     active = active + excluded.active, typing = typing + excluded.typing",
            )?;
            for (app, title, s) in &rows {
                ins.execute(params![minute, app, title, s.active, s.typing])?;
                day_ins.execute(params![day, app, s.active, s.typing])?;
            }
            if let Some(p) = perf {
                tx.execute(
                    "INSERT OR REPLACE INTO perf(minute, cpu, mem, gpu, disk, net_rx, net_tx)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        minute,
                        p.cpu,
                        p.mem,
                        p.gpu,
                        p.disk as i64,
                        p.net_rx as i64,
                        p.net_tx as i64
                    ],
                )?;
                let mut top = tx.prepare_cached(
                    "INSERT OR REPLACE INTO perf_top(minute, kind, rank, app, pid, value)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?;
                for (kind, rank, app, pid, value) in &tops {
                    top.execute(params![minute, kind, rank, app, pid, value])?;
                }
            }
        }
        tx.commit()
    }

    /// 删除过期的性能明细。
    pub fn prune(&self, now_minute: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM perf WHERE minute < ?1",
            [now_minute - PERF_DAYS * 1440],
        )?;
        self.conn.execute(
            "DELETE FROM perf_top WHERE minute < ?1",
            [now_minute - PERF_TOP_DAYS * 1440],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 从 Active Time 导入

/// Active Time 的数据库位置（存在时）。
pub fn active_time_db() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("APPDATA")?)
        .join("Active Time")
        .join("data.db");
    p.exists().then_some(p)
}

#[derive(Clone, Debug, Default)]
pub struct ImportReport {
    pub minutes: i64,
    pub seconds: i64,
    pub programs: usize,
}

/// Active Time 记录的程序名：去掉 `.exe`；被当成 GBK 解码的日文（"偍偟偐偗" 这类）还原成 Shift-JIS 的原文。
pub fn clean_program_name(raw: &str) -> String {
    let name = raw
        .len()
        .checked_sub(4)
        .filter(|&n| raw.is_char_boundary(n) && raw[n..].eq_ignore_ascii_case(".exe"))
        .map_or(raw, |n| &raw[..n]);
    let kana = |s: &str| s.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c));
    let cjk = name.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
    if cjk
        && !kana(name)
        && let Some(fixed) = pulse_probe::foreground::recode(name, 936, 932)
        && kana(&fixed)
    {
        return fixed;
    }
    name.to_string()
}

impl Db {
    /// 导入 Active Time 的记录：只导入 `before`（Pulse 开始记录的那一分钟）之前的部分，
    /// 两边不会重复计时。`progress` 收到 0–1 的进度。
    pub fn import_active_time(
        &mut self,
        src: &std::path::Path,
        before: i64,
        progress: &dyn Fn(f32),
    ) -> rusqlite::Result<ImportReport> {
        let from = self
            .meta("import.active_time.until")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(i64::MIN);
        let srcdb = Connection::open_with_flags(
            src,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let mut report = ImportReport::default();
        let mut programs = std::collections::HashSet::new();
        // (分钟, 程序, 标题) → 秒数；按分钟顺序读，换分钟时写入上一分钟。
        let mut cur_minute = i64::MIN;
        let mut cur: HashMap<(AppRef, String), Seconds> = HashMap::new();
        let mut names: HashMap<String, AppRef> = HashMap::new();
        let flush = |db: &mut Db,
                     minute: i64,
                     cur: &mut HashMap<(AppRef, String), Seconds>,
                     report: &mut ImportReport|
         -> rusqlite::Result<()> {
            if cur.is_empty() {
                return Ok(());
            }
            report.minutes += 1;
            report.seconds += cur.values().map(|s| s.active as i64).sum::<i64>();
            db.write_minute(minute, cur, None)?;
            cur.clear();
            Ok(())
        };

        // 2022-06-23 起最早的几天带窗口标题（表名 ls、is），时间是 "2022-06-23 02:55:00.000 +00:00"。
        // 之后的 minute_records 只有程序名，时间是毫秒。
        let mut sources: Vec<(String, bool)> = Vec::new();
        for t in ["ls", "is"] {
            let exists: bool = srcdb
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [t],
                    |r| r.get::<_, i64>(0),
                )
                .map(|n| n > 0)?;
            if exists {
                sources.push((t.to_string(), true));
            }
        }
        sources.push(("minute_records".into(), false));

        let (lo, hi): (i64, i64) = srcdb.query_row(
            "SELECT COALESCE(MIN(timestamp), 0), COALESCE(MAX(timestamp), 0) FROM minute_records",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let span = ((hi - lo) / 60_000).max(1) as f32;

        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> rusqlite::Result<()> {
            for (table, text_time) in &sources {
                let sql = if *text_time {
                    format!(
                        "SELECT CAST(strftime('%s', substr(timestamp, 1, 19)) AS INTEGER) / 60,
                                program, COALESCE(title, ''), event, seconds
                         FROM \"{table}\" ORDER BY timestamp"
                    )
                } else {
                    "SELECT timestamp / 60000, program, '', event, seconds
                     FROM minute_records ORDER BY timestamp"
                        .to_string()
                };
                let mut stmt = srcdb.prepare(&sql)?;
                let mut rows = stmt.query([])?;
                let mut n = 0u64;
                while let Some(r) = rows.next()? {
                    // Active Time 的库里偶有字段为空的行，跳过。
                    let Some(minute) = r.get::<_, Option<i64>>(0)? else {
                        continue;
                    };
                    if minute <= from || minute >= before {
                        continue;
                    }
                    let program: Option<String> = r.get(1)?;
                    let Some(program) = program.filter(|p| !p.is_empty()) else {
                        continue;
                    };
                    let title: String = r.get::<_, Option<String>>(2)?.unwrap_or_default();
                    let event: Option<String> = r.get(3)?;
                    let seconds: i64 = r.get::<_, Option<i64>>(4)?.unwrap_or(0);
                    if minute != cur_minute {
                        // 写入时已在外层事务里；write_minute 自己的事务改用保存点。
                        flush(self, cur_minute, &mut cur, &mut report)?;
                        cur_minute = minute;
                        n += 1;
                        if n.is_multiple_of(2000) && !*text_time {
                            progress(((minute * 60_000 - lo) / 60_000) as f32 / span);
                        }
                    }
                    let app = names
                        .entry(program.clone())
                        .or_insert_with(|| AppRef::new(&clean_program_name(&program), None))
                        .clone();
                    programs.insert(app.key.clone());
                    let s = cur.entry((app, title)).or_default();
                    let secs = seconds.clamp(0, 60) as u32;
                    s.active = (s.active + secs).min(60);
                    if event.as_deref() == Some("type") {
                        s.typing = (s.typing + secs).min(60);
                    }
                }
            }
            flush(self, cur_minute, &mut cur, &mut report)
        })();
        match result {
            Ok(()) => {
                let until = before.min(hi / 60_000);
                self.set_meta("import.active_time.until", &until.to_string())?;
                self.conn.execute_batch("COMMIT")?;
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                self.apps.clear();
                self.titles.clear();
                return Err(e);
            }
        }
        progress(1.0);
        report.programs = programs.len();
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> (PathBuf, Db) {
        let dir = std::env::temp_dir().join(format!("pulse-history-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("history.db");
        let db = Db::open_at(&p).unwrap();
        (dir, db)
    }

    #[test]
    fn cleans_program_names() {
        assert_eq!(clean_program_name("Code.exe"), "Code");
        assert_eq!(clean_program_name("msedge"), "msedge");
        assert_eq!(
            clean_program_name("偍偟偐偗墱條偲丄僉儈偺塕"),
            "おしかけ奥様と、キミの嘘"
        );
        // 本来就是中文、日文的名字不动。
        assert_eq!(clean_program_name("觅长生"), "觅长生");
        assert_eq!(clean_program_name("オフィスお尻"), "オフィスお尻");
        assert_eq!(clean_program_name("信息:"), "信息:");
    }

    #[test]
    fn writes_and_accumulates_minutes() {
        let (dir, mut db) = temp_db("write");
        let code = AppRef::new("Code", Some(r"C:\a\Code.exe"));
        let mut m = HashMap::new();
        m.insert(
            (code.clone(), "main.rs".to_string()),
            Seconds {
                active: 30,
                typing: 10,
            },
        );
        db.write_minute(29_000_000, &m, None).unwrap();
        db.write_minute(29_000_000, &m, None).unwrap();
        let (active, typing): (i64, i64) = db
            .conn
            .query_row("SELECT active, typing FROM usage", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((active, typing), (60, 20));
        let day: i64 = db
            .conn
            .query_row("SELECT SUM(active) FROM usage_day", [], |r| r.get(0))
            .unwrap();
        assert_eq!(day, 60);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn total(db: &Db, key: &str) -> i64 {
        db.conn
            .query_row(
                "SELECT COALESCE(SUM(u.active), 0) FROM usage u JOIN apps a ON a.id = u.app WHERE a.key = ?1",
                [key],
                |r| r.get(0),
            )
            .unwrap()
    }

    /// 造一个 Active Time 格式的小库，确认导入的换算与截止时间。
    #[test]
    fn imports_active_time() {
        let (dir, mut db) = temp_db("import");
        let src = dir.join("data.db");
        let s = Connection::open(&src).unwrap();
        s.execute_batch(
            "CREATE TABLE `is` (id INTEGER PRIMARY KEY, program TEXT, title TEXT, event TEXT, timestamp DATETIME, seconds INTEGER, createdAt DATETIME);
             INSERT INTO `is`(program, title, event, timestamp, seconds, createdAt) VALUES
               ('Code.exe', 'a.rs', 'type', '2022-06-23 02:55:00.000 +00:00', 5, ''),
               ('Code.exe', 'a.rs', 'read', '2022-06-23 02:55:00.000 +00:00', 5, '');
             CREATE TABLE minute_records (id INTEGER PRIMARY KEY, program TEXT, title TEXT, event TEXT, timestamp DATETIME, seconds INTEGER, createdAt DATETIME);
             INSERT INTO minute_records(program, event, timestamp, seconds, createdAt) VALUES
               ('msedge', 'read', 1656354420000, 40, ''),
               ('msedge', 'type', 1656354420000, 30, ''),
               ('偍偟偐偗', 'read', 1656354480000, 5, ''),
               ('msedge', 'read', 1700000000000, 5, '');",
        )
        .unwrap();
        drop(s);
        // Pulse 从 1700000000000 那一分钟开始记录，之后的不导入。
        let before = 1_700_000_000_000 / 60_000;
        let r = db.import_active_time(&src, before, &|_| {}).unwrap();
        assert_eq!(r.minutes, 3);
        assert_eq!(r.programs, 3);
        assert_eq!(total(&db, "code"), 10);
        // 同一分钟 70 秒封顶为 60。
        assert_eq!(total(&db, "msedge"), 60);
        assert_eq!(total(&db, "おしかけ"), 5);
        let titled: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM titles WHERE text = 'a.rs'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(titled, 1);
        // 再导一次不会重复。
        let again = db.import_active_time(&src, before, &|_| {}).unwrap();
        assert_eq!(again.minutes, 0);
        assert_eq!(total(&db, "msedge"), 60);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod real_import {
    use super::*;

    /// 用本机真实的 Active Time 数据（只读）导入到临时库，看耗时与结果。
    /// `cargo test --release -p pulse -- --ignored --nocapture real_active_time`
    #[test]
    #[ignore]
    fn real_active_time() {
        let Some(src) = active_time_db() else { return };
        let dir = std::env::temp_dir().join("pulse-real-import");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut db = Db::open_at(&dir.join("history.db")).unwrap();
        let t = std::time::Instant::now();
        let r = db
            .import_active_time(&src, i64::MAX, &|p| eprint!("\r{:.0}%  ", p * 100.0))
            .unwrap();
        eprintln!("\n{r:?} in {:?}", t.elapsed());
        let mut stmt = db
            .conn
            .prepare(
                "SELECT a.name, SUM(d.active) / 3600 FROM usage_day d JOIN apps a ON a.id = d.app
                 GROUP BY d.app ORDER BY 2 DESC LIMIT 15",
            )
            .unwrap();
        let top: Vec<(String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        eprintln!("top: {top:?}");
        let size = std::fs::metadata(dir.join("history.db")).unwrap().len();
        eprintln!("db size {} MB", size / 1_000_000);
    }
}
