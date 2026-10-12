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

/// "在线搜索"使用的搜索引擎。浏览器自己的默认搜索引擎没有系统接口可读，所以在这里选。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchEngine {
    Google,
    Bing,
    DuckDuckGo,
    Baidu,
}

impl SearchEngine {
    pub const ALL: [SearchEngine; 4] = [
        SearchEngine::Google,
        SearchEngine::Bing,
        SearchEngine::DuckDuckGo,
        SearchEngine::Baidu,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SearchEngine::Google => "Google",
            SearchEngine::Bing => "Bing",
            SearchEngine::DuckDuckGo => "DuckDuckGo",
            SearchEngine::Baidu => "百度",
        }
    }

    /// 搜索地址前缀，后面直接接 URL 编码后的关键词。
    pub fn url_prefix(self) -> &'static str {
        match self {
            SearchEngine::Google => "https://www.google.com/search?q=",
            SearchEngine::Bing => "https://www.bing.com/search?q=",
            SearchEngine::DuckDuckGo => "https://duckduckgo.com/?q=",
            SearchEngine::Baidu => "https://www.baidu.com/s?wd=",
        }
    }

    fn key(self) -> &'static str {
        match self {
            SearchEngine::Google => "google",
            SearchEngine::Bing => "bing",
            SearchEngine::DuckDuckGo => "duckduckgo",
            SearchEngine::Baidu => "baidu",
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
    pub search: SearchEngine,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            speed: Speed::Normal,
            always_on_top: false,
            theme: ThemeChoice::System,
            grouped: true,
            search: SearchEngine::Google,
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
                "search" => {
                    s.search = SearchEngine::ALL
                        .into_iter()
                        .find(|x| x.key() == v)
                        .unwrap_or(s.search)
                }
                _ => {}
            }
        }
        s
    }

    fn serialize(&self) -> String {
        format!(
            "speed={}\nalways_on_top={}\ntheme={}\ngrouped={}\nsearch={}\n",
            self.speed.key(),
            self.always_on_top,
            self.theme.key(),
            self.grouped,
            self.search.key(),
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
            search: SearchEngine::DuckDuckGo,
        };
        assert_eq!(Settings::parse(&s.serialize()), s);
        assert_eq!(Settings::parse("garbage\nspeed=nope"), Settings::default());
    }
}
