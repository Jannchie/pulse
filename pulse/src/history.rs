//! 使用记录与性能历史：本地 SQLite 数据库（`%LOCALAPPDATA%\Pulse\history.db`）。
//!
//! - `usage`：每分钟、每个程序、每个窗口标题在前台的活跃秒数，其中有键盘输入的秒数；
//! - `usage_day`：按本地日期汇总的同样数据，统计页按天、周、月查询时不必扫描明细；
//! - `perf` / `perf_top`：每分钟的整体占用，以及 CPU、内存、磁盘、网络、GPU 各自占用最高的进程；
//! - `net` / `net_day`：各程序每分钟、每天收发的字节数（逐进程网络统计需要管理员权限）。
//!
//! 数据只保存在本机。写入由记录线程独占一个连接；界面查询另开只读连接（WAL 模式下互不阻塞）。

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, params};

/// 性能明细保留的天数；使用记录不清理（每天只有几百行）。
const PERF_DAYS: i64 = 30;
const PERF_TOP_DAYS: i64 = 7;
/// 每分钟的流量明细只给"最近 24 小时"用，按天的汇总一直保留。
const NET_MINUTE_DAYS: i64 = 2;

pub fn path() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
            .join("Pulse")
            .join("history.db"),
    )
}

/// 程序的标识：按 exe 文件名（不含扩展名、不区分大小写）归并。
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
    /// 各程序这一分钟收、发的字节数。
    pub traffic: Vec<(AppRef, u64, u64)>,
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
        // 统计页等其他连接写入时，等它写完而不是直接失败。
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        // 两个连接同时建库时，切换 WAL 不走 busy_timeout，会直接报 "database is locked"，稍后重试。
        let mut tries = 0;
        while let Err(e) = conn.execute_batch(
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
                 PRIMARY KEY(minute, kind, rank)) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS net(
                 minute INTEGER NOT NULL, app INTEGER NOT NULL,
                 rx INTEGER NOT NULL, tx INTEGER NOT NULL,
                 PRIMARY KEY(minute, app)) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS net_day(
                 day INTEGER NOT NULL, app INTEGER NOT NULL,
                 rx INTEGER NOT NULL, tx INTEGER NOT NULL,
                 PRIMARY KEY(day, app)) WITHOUT ROWID;",
        ) {
            tries += 1;
            if tries >= 50 || e.sqlite_error_code() != Some(rusqlite::ErrorCode::DatabaseBusy) {
                return Err(e);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
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
        let mut traffic = Vec::new();
        if let Some(p) = perf {
            for (kind, rank, name, pid, value) in &p.top {
                let id = self.app_id(&AppRef::new(name, None))?;
                tops.push((*kind as i64, *rank, id, *pid, *value));
            }
            for (app, rx, tx) in &p.traffic {
                traffic.push((self.app_id(app)?, *rx as i64, *tx as i64));
            }
        }
        // 这一分钟的所有写入放在同一个事务里。
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
                let mut net = tx.prepare_cached(
                    "INSERT INTO net(minute, app, rx, tx) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(minute, app) DO UPDATE SET
                         rx = rx + excluded.rx, tx = tx + excluded.tx",
                )?;
                let mut net_day = tx.prepare_cached(
                    "INSERT INTO net_day(day, app, rx, tx) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(day, app) DO UPDATE SET
                         rx = rx + excluded.rx, tx = tx + excluded.tx",
                )?;
                for (app, rx, tx) in &traffic {
                    net.execute(params![minute, app, rx, tx])?;
                    net_day.execute(params![day, app, rx, tx])?;
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
        self.conn.execute(
            "DELETE FROM net WHERE minute < ?1",
            [now_minute - NET_MINUTE_DAYS * 1440],
        )?;
        Ok(())
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

    #[test]
    fn accumulates_traffic() {
        let (dir, mut db) = temp_db("net");
        let perf = PerfMinute {
            traffic: vec![(AppRef::new("steam", None), 1000, 10)],
            ..Default::default()
        };
        db.write_minute(29_000_000, &HashMap::new(), Some(&perf))
            .unwrap();
        db.write_minute(29_000_001, &HashMap::new(), Some(&perf))
            .unwrap();
        let (rx, tx): (i64, i64) = db
            .conn
            .query_row("SELECT SUM(rx), SUM(tx) FROM net_day", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((rx, tx), (2000, 20));
        let minutes: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM net", [], |r| r.get(0))
            .unwrap();
        assert_eq!(minutes, 2);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn opens_concurrently() {
        // 记录线程和统计页同时建库，两边都要打开成功。
        for i in 0..10 {
            let (dir, db) = temp_db(&format!("race{i}"));
            drop(db);
            // 每轮都是一个新文件，两个连接才会同时建库。
            let p = dir.join("race.db");
            let opens: Vec<_> = (0..2)
                .map(|_| {
                    let p = p.clone();
                    std::thread::spawn(move || Db::open_at(&p).map(drop))
                })
                .collect();
            for h in opens {
                h.join().unwrap().unwrap();
            }
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
