//! 用户设置：保存在 %APPDATA%\Pulse\settings.ini（简单的 key=value）。

use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Speed {
    High,
    Normal,
    Low,
    Paused,
}

impl Speed {
    pub const ALL: [Speed; 4] = [Speed::High, Speed::Normal, Speed::Low, Speed::Paused];

    pub fn label(self) -> &'static str {
        match self {
            Speed::High => "高（0.5 秒）",
            Speed::Normal => "正常（1 秒）",
            Speed::Low => "低（4 秒）",
            Speed::Paused => "暂停",
        }
    }

    /// 采集间隔（毫秒），0 表示暂停。
    pub fn millis(self) -> u64 {
        match self {
            Speed::High => 500,
            Speed::Normal => 1000,
            Speed::Low => 4000,
            Speed::Paused => 0,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Speed::High => "high",
            Speed::Normal => "normal",
            Speed::Low => "low",
            Speed::Paused => "paused",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "跟随系统",
            ThemeChoice::Light => "浅色",
            ThemeChoice::Dark => "深色",
        }
    }

    pub fn preference(self) -> eframe::egui::ThemePreference {
        match self {
            ThemeChoice::System => eframe::egui::ThemePreference::System,
            ThemeChoice::Light => eframe::egui::ThemePreference::Light,
            ThemeChoice::Dark => eframe::egui::ThemePreference::Dark,
        }
    }

    fn key(self) -> &'static str {
        match self {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub speed: Speed,
    pub always_on_top: bool,
    pub theme: ThemeChoice,
    pub grouped: bool,
    /// 隐藏的列（列标签）。
    pub hidden_columns: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            speed: Speed::Normal,
            always_on_top: false,
            theme: ThemeChoice::System,
            grouped: true,
            hidden_columns: Vec::new(),
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("APPDATA")?)
            .join("Pulse")
            .join("settings.ini"),
    )
}

impl Settings {
    pub fn load() -> Self {
        let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return Self::default();
        };
        Self::parse(&text)
    }

    fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim();
            match k.trim() {
                "speed" => {
                    s.speed = Speed::ALL
                        .into_iter()
                        .find(|x| x.key() == v)
                        .unwrap_or(s.speed)
                }
                "always_on_top" => s.always_on_top = v == "true",
                "theme" => {
                    s.theme = ThemeChoice::ALL
                        .into_iter()
                        .find(|x| x.key() == v)
                        .unwrap_or(s.theme)
                }
                "grouped" => s.grouped = v != "false",
                "hidden_columns" => {
                    s.hidden_columns = v
                        .split(',')
                        .map(str::trim)
                        .filter(|c| !c.is_empty())
                        .map(String::from)
                        .collect()
                }
                _ => {}
            }
        }
        s
    }

    fn serialize(&self) -> String {
        format!(
            "speed={}\nalways_on_top={}\ntheme={}\ngrouped={}\nhidden_columns={}\n",
            self.speed.key(),
            self.always_on_top,
            self.theme.key(),
            self.grouped,
            self.hidden_columns.join(",")
        )
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, self.serialize());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = Settings {
            speed: Speed::Low,
            always_on_top: true,
            theme: ThemeChoice::Dark,
            grouped: false,
            hidden_columns: vec!["PID".into(), "CPU 时间".into()],
        };
        assert_eq!(Settings::parse(&s.serialize()), s);
        assert_eq!(Settings::parse("garbage\nspeed=nope"), Settings::default());
    }
}
