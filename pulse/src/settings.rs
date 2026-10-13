//! 用户设置：保存在 %APPDATA%\Pulse\settings.ini（简单的 key=value）。

use std::path::PathBuf;

use pulse_probe::i18n::Lang;
use pulse_probe::t;

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
            Speed::High => t!("高（0.5 秒）", "High (0.5 s)", "高 (0.5 秒)"),
            Speed::Normal => t!("正常（1 秒）", "Normal (1 s)", "標準 (1 秒)"),
            Speed::Low => t!("低（4 秒）", "Low (4 s)", "低 (4 秒)"),
            Speed::Paused => t!("暂停", "Paused", "一時停止"),
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
            SearchEngine::Baidu => t!("百度", "Baidu", "Baidu"),
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
            ThemeChoice::System => t!("跟随系统", "System", "システム設定を使用"),
            ThemeChoice::Light => t!("浅色", "Light", "ライト"),
            ThemeChoice::Dark => t!("深色", "Dark", "ダーク"),
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

/// 界面语言。默认跟随 Windows 的显示语言。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Language {
    System,
    Zh,
    En,
    Ja,
}

impl Language {
    pub const ALL: [Language; 4] = [Language::System, Language::Zh, Language::En, Language::Ja];

    /// 具体语言的选项用它自己的语言写，切换前后都认得出来。
    pub fn label(self) -> &'static str {
        match self {
            Language::System => t!("跟随系统", "System", "システム設定を使用"),
            Language::Zh => "中文",
            Language::En => "English",
            Language::Ja => "日本語",
        }
    }

    pub fn resolve(self) -> Lang {
        match self {
            Language::System => pulse_probe::i18n::system_lang(),
            Language::Zh => Lang::Zh,
            Language::En => Lang::En,
            Language::Ja => Lang::Ja,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Language::System => "system",
            Language::Zh => "zh",
            Language::En => "en",
            Language::Ja => "ja",
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
    pub language: Language,
    /// 启动后与每 24 小时自动检查更新。
    pub auto_update: bool,
    /// 用户选择跳过的版本（不带 v），不再为它提示。
    pub skipped_version: String,
    /// 关闭窗口时缩到通知区域，继续在后台运行。
    pub close_to_tray: bool,
    /// 已经提示过"关闭后仍在后台运行"。
    pub tray_hinted: bool,
    /// 记录各程序的使用时长与性能历史。
    pub record_usage: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            speed: Speed::Normal,
            always_on_top: false,
            theme: ThemeChoice::System,
            grouped: true,
            search: SearchEngine::Google,
            language: Language::System,
            auto_update: true,
            skipped_version: String::new(),
            close_to_tray: true,
            tray_hinted: false,
            record_usage: true,
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
                "language" => {
                    s.language = Language::ALL
                        .into_iter()
                        .find(|x| x.key() == v)
                        .unwrap_or(s.language)
                }
                "auto_update" => s.auto_update = v != "false",
                "skip_version" => s.skipped_version = v.to_string(),
                "close_to_tray" => s.close_to_tray = v != "false",
                "tray_hinted" => s.tray_hinted = v == "true",
                "record_usage" => s.record_usage = v != "false",
                _ => {}
            }
        }
        s
    }

    fn serialize(&self) -> String {
        format!(
            "speed={}\nalways_on_top={}\ntheme={}\ngrouped={}\nsearch={}\nlanguage={}\nauto_update={}\nskip_version={}\nclose_to_tray={}\ntray_hinted={}\nrecord_usage={}\n",
            self.speed.key(),
            self.always_on_top,
            self.theme.key(),
            self.grouped,
            self.search.key(),
            self.language.key(),
            self.auto_update,
            self.skipped_version,
            self.close_to_tray,
            self.tray_hinted,
            self.record_usage,
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
            language: Language::Ja,
            auto_update: false,
            skipped_version: "0.3.2".into(),
            close_to_tray: false,
            tray_hinted: true,
            record_usage: false,
        };
        assert_eq!(Settings::parse(&s.serialize()), s);
        assert_eq!(Settings::parse("garbage\nspeed=nope"), Settings::default());
    }

    #[test]
    fn old_files_without_language_follow_system() {
        // 加入语言设置之前保存的文件没有 language 这一行。
        let old = "speed=low\nalways_on_top=true\ntheme=dark\ngrouped=false\nsearch=bing\n";
        let s = Settings::parse(old);
        assert_eq!(s.language, Language::System);
        assert_eq!(s.speed, Speed::Low);
        assert_eq!(s.search, SearchEngine::Bing);
        assert!(s.auto_update);
        assert_eq!(s.skipped_version, "");
        for lang in Language::ALL {
            let s = Settings {
                language: lang,
                ..Settings::default()
            };
            assert_eq!(Settings::parse(&s.serialize()).language, lang);
        }
    }

    #[test]
    fn parses_every_language_key() {
        // zh/en 是加入日语之前就有的取值，旧文件必须照常读出。
        for (v, lang) in [
            ("system", Language::System),
            ("zh", Language::Zh),
            ("en", Language::En),
            ("ja", Language::Ja),
        ] {
            let s = Settings::parse(&format!("speed=high\nlanguage={v}\n"));
            assert_eq!(s.language, lang);
            assert_eq!(s.speed, Speed::High);
        }
        // 不认识的取值回到跟随系统。
        assert_eq!(Settings::parse("language=fr\n").language, Language::System);
    }

    #[test]
    fn update_keys() {
        // 加入自动更新之前的文件：默认开启，没有跳过的版本。
        let old = "speed=high\nalways_on_top=false\ntheme=system\ngrouped=true\nsearch=google\nlanguage=zh\n";
        let s = Settings::parse(old);
        assert!(s.auto_update);
        assert_eq!(s.skipped_version, "");
        assert_eq!(s.language, Language::Zh);
        assert_eq!(s.speed, Speed::High);
        let s = Settings::parse("auto_update=false\nskip_version= 0.4.0 \n");
        assert!(!s.auto_update);
        assert_eq!(s.skipped_version, "0.4.0");
        for auto in [true, false] {
            let s = Settings {
                auto_update: auto,
                ..Settings::default()
            };
            assert_eq!(Settings::parse(&s.serialize()), s);
        }
    }
}
