//! 程序分类：游戏、开发、办公、创作、沟通、浏览、影音、系统工具。
//!
//! 依次按：用户手动指定（`%APPDATA%\Pulse\categories.ini`）→ Windows 游戏栏的已知游戏 →
//! 内置的常见程序表 → Steam / Epic / GOG 的游戏目录 → exe 旁边的游戏引擎文件。
//! 都不中的归为"未分类"。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::Color32;
use pulse_probe::games::{GameSource, KnownGames, engine_files};
use pulse_probe::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Game,
    Dev,
    Work,
    Creative,
    Chat,
    Web,
    Media,
    System,
    Other,
}

impl Category {
    pub const ALL: [Category; 9] = [
        Category::Game,
        Category::Dev,
        Category::Work,
        Category::Creative,
        Category::Chat,
        Category::Web,
        Category::Media,
        Category::System,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Game => t!("游戏", "Games", "ゲーム"),
            Category::Dev => t!("开发", "Development", "開発"),
            Category::Work => t!("办公", "Productivity", "仕事効率化"),
            Category::Creative => t!("创作", "Creative", "クリエイティブ"),
            Category::Chat => t!("沟通", "Communication", "コミュニケーション"),
            Category::Web => t!("浏览", "Browsing", "ブラウズ"),
            Category::Media => t!("影音", "Media", "メディア"),
            Category::System => t!("系统工具", "System & tools", "システム・ツール"),
            Category::Other => t!("未分类", "Uncategorized", "未分類"),
        }
    }

    pub fn color(self) -> Color32 {
        match self {
            Category::Game => Color32::from_rgb(0xf4, 0x3f, 0x5e),
            Category::Dev => Color32::from_rgb(0x63, 0x66, 0xf1),
            Category::Work => Color32::from_rgb(0x14, 0xb8, 0xa6),
            Category::Creative => Color32::from_rgb(0xf5, 0x9e, 0x0b),
            Category::Chat => Color32::from_rgb(0x84, 0xcc, 0x16),
            Category::Web => Color32::from_rgb(0x0e, 0xa5, 0xe9),
            Category::Media => Color32::from_rgb(0xd9, 0x46, 0xef),
            Category::System => Color32::from_rgb(0x94, 0xa3, 0xb8),
            Category::Other => Color32::from_rgb(0x8a, 0x8a, 0x8a),
        }
    }

    /// 开发、办公、创作合称"生产力"。
    pub fn productive(self) -> bool {
        matches!(self, Category::Dev | Category::Work | Category::Creative)
    }

    fn key(self) -> &'static str {
        match self {
            Category::Game => "game",
            Category::Dev => "dev",
            Category::Work => "work",
            Category::Creative => "creative",
            Category::Chat => "chat",
            Category::Web => "web",
            Category::Media => "media",
            Category::System => "system",
            Category::Other => "other",
        }
    }

    fn from_key(k: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == k)
    }
}

/// 分类的依据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    User,
    WindowsGames,
    GameLibrary,
    Engine,
    BuiltIn,
    Unknown,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Reason::User => t!("手动指定", "Set by you", "手動で指定"),
            Reason::WindowsGames => t!(
                "Windows 游戏列表",
                "Windows game list",
                "Windows のゲーム一覧"
            ),
            Reason::GameLibrary => t!(
                "Steam / Epic / GOG 游戏目录",
                "Steam / Epic / GOG library",
                "Steam / Epic / GOG のライブラリ"
            ),
            Reason::Engine => t!(
                "游戏引擎文件",
                "Game engine files",
                "ゲーム エンジンのファイル"
            ),
            Reason::BuiltIn => t!("常见程序", "Well-known app", "よく使われるアプリ"),
            Reason::Unknown => t!("未识别", "Not recognized", "識別できません"),
        }
    }
}

/// 常见程序（exe 名，小写、不含扩展名）。
const BUILT_IN: &[(Category, &[&str])] = &[
    (
        Category::Game,
        &[
            "steam",
            "steamwebhelper",
            "epicgameslauncher",
            "battle.net",
            "riotclientux",
            "riotclientservices",
            "leagueclientux",
            "eadesktop",
            "origin",
            "upc",
            "ubisoftconnect",
            "galaxyclient",
            "goggalaxy",
            "xboxpcapp",
            "playnite.desktopapp",
            "playnite.fullscreenapp",
            "heroic",
            "minecraftlauncher",
            "wegame",
            "hyp",
            "retroarch",
            "dolphin",
            "pcsx2-qt",
            "rpcs3",
            "yuzu",
            "ryujinx",
            "citra-qt",
            "ppssppwindows64",
            "cemu",
            "duckstation-qt-x64-releaseltcg",
            "siglusengine",
            "reallive",
            "bgi",
            "malie",
            "cmvs32",
            "cmvs64",
        ],
    ),
    (
        Category::Dev,
        &[
            "code",
            "code - insiders",
            "cursor",
            "windsurf",
            "zed",
            "devenv",
            "idea",
            "idea64",
            "pycharm",
            "pycharm64",
            "webstorm64",
            "rider64",
            "clion64",
            "goland64",
            "rustrover64",
            "phpstorm64",
            "rubymine64",
            "datagrip64",
            "dataspell64",
            "studio64",
            "fleet",
            "sublime_text",
            "sublime_merge",
            "notepad++",
            "windowsterminal",
            "wt",
            "openconsole",
            "powershell",
            "powershell_ise",
            "pwsh",
            "cmd",
            "conhost",
            "mintty",
            "alacritty",
            "wezterm-gui",
            "kitty",
            "warp",
            "tabby",
            "hyper",
            "nvim-qt",
            "neovide",
            "gvim",
            "emacs",
            "runemacs",
            "postman",
            "insomnia",
            "bruno",
            "hoppscotch",
            "dbeaver",
            "heidisql",
            "ssms",
            "tableplus",
            "navicat",
            "githubdesktop",
            "gitkraken",
            "fork",
            "sourcetree",
            "smartgit",
            "tortoisegitproc",
            "docker desktop",
            "unity",
            "unity hub",
            "unrealeditor",
            "ue4editor",
            "wireshark",
            "fiddler",
            "x64dbg",
            "x32dbg",
            "ida",
            "ida64",
            "termius",
            "mobaxterm",
            "xshell",
            "putty",
            "winscp",
            "filezilla",
            "vmware",
            "virtualboxvm",
            "virtualbox",
            "vmconnect",
            "android studio",
        ],
    ),
    (
        Category::Work,
        &[
            "winword",
            "excel",
            "powerpnt",
            "onenote",
            "onenoteim",
            "msaccess",
            "mspub",
            "visio",
            "winproj",
            "wps",
            "et",
            "wpp",
            "wpspdf",
            "soffice",
            "soffice.bin",
            "notion",
            "obsidian",
            "logseq",
            "typora",
            "marktext",
            "joplin",
            "evernote",
            "xmind",
            "mindmanager",
            "acrobat",
            "acrord32",
            "foxitpdfreader",
            "foxitphantompdf",
            "foxitpdfeditor",
            "sumatrapdf",
            "pdfxedit",
            "notepad",
            "anki",
            "zotero",
            "calibre",
            "kindle",
            "todoist",
            "ticktick",
            "chatgpt",
            "claude",
            "deepl",
            "youdaodict",
            "eudic",
            "goldendict",
            "mstsc",
            "anydesk",
            "teamviewer",
            "linear",
            "clickup",
        ],
    ),
    (
        Category::Creative,
        &[
            "photoshop",
            "illustrator",
            "indesign",
            "afterfx",
            "adobe premiere pro",
            "adobe media encoder",
            "audition",
            "lightroom",
            "animate",
            "adobe substance 3d painter",
            "figma",
            "blender",
            "krita",
            "clipstudiopaint",
            "aseprite",
            "inkscape",
            "paintdotnet",
            "mspaint",
            "sai",
            "sai2",
            "resolve",
            "obs64",
            "obs32",
            "fl64",
            "fl",
            "reaper",
            "studio one",
            "audacity",
            "musescore4",
            "musescore3",
            "capcut",
            "jianyingpro",
            "sketchup",
            "3dsmax",
            "maya",
            "cinema 4d",
            "zbrush",
            "houdinifx",
            "houdini",
            "pureref",
            "eagle",
            "handbrake",
            "shotcut",
            "kdenlive",
            "spine",
            "live2d cubism editor",
        ],
    ),
    (
        Category::Chat,
        &[
            "slack",
            "teams",
            "ms-teams",
            "discord",
            "telegram",
            "wechat",
            "weixin",
            "wxwork",
            "qq",
            "tim",
            "dingtalk",
            "feishu",
            "lark",
            "zoom",
            "wemeetapp",
            "line",
            "whatsapp",
            "signal",
            "skype",
            "outlook",
            "olk",
            "thunderbird",
            "mailbird",
            "kakaotalk",
            "element",
            "mattermost",
            "chatwork",
        ],
    ),
    (
        Category::Web,
        &[
            "chrome",
            "msedge",
            "firefox",
            "brave",
            "opera",
            "vivaldi",
            "arc",
            "zen",
            "librewolf",
            "waterfox",
            "floorp",
            "thorium",
            "chromium",
            "360se",
            "360chrome",
            "qqbrowser",
            "sogouexplorer",
            "iexplore",
        ],
    ),
    (
        Category::Media,
        &[
            "spotify",
            "vlc",
            "potplayer",
            "potplayermini",
            "potplayermini64",
            "mpv",
            "mpc-hc",
            "mpc-hc64",
            "mpc-be64",
            "cloudmusic",
            "qqmusic",
            "kugou",
            "kwmusic",
            "foobar2000",
            "aimp",
            "musicbee",
            "itunes",
            "applemusic",
            "music.ui",
            "video.ui",
            "microsoft.media.player",
            "kodi",
            "plex",
            "plex htpc",
            "netflix",
            "哔哩哔哩",
            "bilibili",
            "qqlive",
            "iqiyi",
            "qiyiclient",
            "youku",
            "douyin",
            "honeyview",
            "imageglass",
            "microsoft.photos",
            "picview",
        ],
    ),
    (
        Category::System,
        &[
            "explorer",
            "taskmgr",
            "pulse",
            "sweep",
            "systemsettings",
            "mmc",
            "regedit",
            "control",
            "applicationframehost",
            "searchhost",
            "searchapp",
            "startmenuexperiencehost",
            "shellexperiencehost",
            "lockapp",
            "textinputhost",
            "snippingtool",
            "screenclippinghost",
            "everything",
            "everything64",
            "powertoys",
            "powertoys.settings",
            "powertoys.powerlauncher",
            "7zfm",
            "winrar",
            "bandizip",
            "sharex",
            "snipaste",
            "wallpaper32",
            "wallpaper64",
            "perfmon",
            "resmon",
            "procexp",
            "procexp64",
            "processhacker",
            "systeminformer",
            "cpuz",
            "hwinfo64",
            "crystaldiskinfo",
            "crystaldiskmark",
            "windirstat",
            "wiztree",
            "wiztree64",
            "msiexec",
            "rundll32",
            "dllhost",
            "winstore.app",
            "msedgewebview2",
        ],
    ),
];

/// 名字开头相同即可（版本号写在 exe 名里的程序）。
const PREFIXES: &[(Category, &str)] = &[
    (Category::Game, "onscripter"),
    (Category::Game, "ai5win"),
    (Category::Game, "ai6win"),
    (Category::Dev, "godot"),
    (Category::Dev, "jetbrains"),
    (Category::Creative, "gimp"),
    (Category::Creative, "ableton live"),
    (Category::Creative, "cubase"),
    (Category::Creative, "vegas"),
];

/// 太常见的 exe 名：只有名字、没有路径时不据此认作游戏。
const GENERIC: &[&str] = &[
    "game", "launcher", "start", "starter", "setup", "main", "app", "client", "play", "run",
    "java", "javaw", "python", "pythonw", "node", "electron",
];

fn built_in(key: &str) -> Option<Category> {
    BUILT_IN
        .iter()
        .find(|(_, names)| names.contains(&key))
        .map(|(c, _)| *c)
        .or_else(|| {
            PREFIXES
                .iter()
                .find(|(_, p)| key.starts_with(p))
                .map(|(c, _)| *c)
        })
}

fn overrides_path() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("APPDATA")?)
            .join("Pulse")
            .join("categories.ini"),
    )
}

pub struct Classifier {
    games: KnownGames,
    scanned: Instant,
    overrides: HashMap<String, Category>,
    /// 各 exe 旁边有没有游戏引擎文件（要读目录，记下来）。
    engines: RefCell<HashMap<String, bool>>,
}

impl Classifier {
    pub fn load() -> Self {
        let text = overrides_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        Self {
            games: KnownGames::scan(),
            scanned: Instant::now(),
            overrides: parse(&text),
            engines: RefCell::default(),
        }
    }

    /// 装了新游戏、玩了新游戏之后 Windows 的列表会变，每 10 分钟重新读一次。返回是否重读了。
    pub fn rescan_if_stale(&mut self) -> bool {
        if self.scanned.elapsed() < Duration::from_secs(600) {
            return false;
        }
        self.games = KnownGames::scan();
        self.scanned = Instant::now();
        self.engines.borrow_mut().clear();
        true
    }

    /// `key` 是小写 exe 名（不含扩展名）。
    pub fn classify(&self, key: &str, path: Option<&str>) -> (Category, Reason) {
        if let Some(&c) = self.overrides.get(key) {
            return (c, Reason::User);
        }
        let source = path.and_then(|p| self.games.source(p));
        if source == Some(GameSource::Windows) {
            return (Category::Game, Reason::WindowsGames);
        }
        if let Some(c) = built_in(key) {
            return (c, Reason::BuiltIn);
        }
        if source == Some(GameSource::Library) {
            return (Category::Game, Reason::GameLibrary);
        }
        if let Some(p) = path
            && *self
                .engines
                .borrow_mut()
                .entry(p.to_string())
                .or_insert_with(|| engine_files(p))
        {
            return (Category::Game, Reason::Engine);
        }
        if path.is_none() && !GENERIC.contains(&key) && self.games.has_stem(key) {
            return (Category::Game, Reason::WindowsGames);
        }
        (Category::Other, Reason::Unknown)
    }

    /// 手动指定分类；`None` 恢复自动识别。
    pub fn set(&mut self, key: &str, category: Option<Category>) {
        match category {
            Some(c) => self.overrides.insert(key.to_string(), c),
            None => self.overrides.remove(key),
        };
        let Some(p) = overrides_path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, serialize(&self.overrides));
    }
}

/// 每行 `exe 名=分类`。
fn parse(text: &str) -> HashMap<String, Category> {
    text.lines()
        .filter_map(|line| {
            let (k, v) = line.rsplit_once('=')?;
            let k = k.trim().to_lowercase();
            (!k.is_empty()).then_some((k, Category::from_key(v.trim())?))
        })
        .collect()
}

fn serialize(map: &HashMap<String, Category>) -> String {
    let mut lines: Vec<String> = map
        .iter()
        .map(|(k, c)| format!("{k}={}\n", c.key()))
        .collect();
    lines.sort();
    lines.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classifier() -> Classifier {
        Classifier {
            games: KnownGames {
                exes: vec![r"e:\game\aokana\aokana.exe".into()],
                dirs: vec![r"e:\steamlibrary\steamapps\common\".into()],
            },
            scanned: Instant::now(),
            overrides: HashMap::new(),
            engines: RefCell::default(),
        }
    }

    #[test]
    fn classifies_in_order() {
        let mut c = classifier();
        assert_eq!(
            c.classify("aokana", Some(r"E:\GAME\Aokana\Aokana.exe")),
            (Category::Game, Reason::WindowsGames)
        );
        assert_eq!(
            c.classify(
                "balatro",
                Some(r"E:\SteamLibrary\steamapps\common\Balatro\Balatro.exe")
            ),
            (Category::Game, Reason::GameLibrary)
        );
        // 在 Steam 目录里、但内置表认得的程序（Steam 版 Blender）按内置表。
        assert_eq!(
            c.classify(
                "blender",
                Some(r"E:\SteamLibrary\steamapps\common\Blender\blender.exe")
            ),
            (Category::Creative, Reason::BuiltIn)
        );
        assert_eq!(c.classify("code", None), (Category::Dev, Reason::BuiltIn));
        assert_eq!(c.classify("godot_v4.3-stable_win64", None).0, Category::Dev);
        // 没有路径的旧记录：按 Windows 游戏列表里的 exe 名。
        assert_eq!(c.classify("aokana", None).0, Category::Game);
        assert_eq!(c.classify("onscripter-ru", None).0, Category::Game);
        assert_eq!(
            c.classify(
                "palworld-win64-shipping",
                Some(r"X:\Pal\Binaries\Win64\Palworld-Win64-Shipping.exe")
            ),
            (Category::Game, Reason::Engine)
        );
        assert_eq!(
            c.classify("mystery", Some(r"C:\Tools\mystery.exe")),
            (Category::Other, Reason::Unknown)
        );
        c.overrides.insert("code".into(), Category::Game);
        assert_eq!(c.classify("code", None), (Category::Game, Reason::User));
    }

    #[test]
    fn overrides_round_trip() {
        let mut m = HashMap::new();
        m.insert("code".to_string(), Category::Work);
        m.insert("my game".to_string(), Category::Game);
        assert_eq!(parse(&serialize(&m)), m);
        assert!(parse("junk\nx=nope\n=game\n").is_empty());
    }

    #[test]
    fn built_in_table_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for (_, names) in BUILT_IN {
            for n in *names {
                assert!(seen.insert(*n), "{n} listed twice");
                assert_eq!(*n, n.to_lowercase());
            }
        }
    }
}
