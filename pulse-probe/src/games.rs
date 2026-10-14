//! 本机已知的游戏：Windows 游戏栏识别过的 exe，以及 Steam、Epic、GOG 的游戏安装目录。
//!
//! - `HKCU\System\GameConfigStore\Children\*` 的 `MatchedExeFullPath`：Windows 按微软维护的
//!   已知游戏列表（Known Game List）认出、并在本机运行过的游戏，也包括不经商店安装的游戏；
//! - Steam：各个库的 `steamapps\common`（`libraryfolders.vdf`）；
//! - Epic：`Manifests\*.item` 里类别含 games 的 `InstallLocation`；
//! - GOG：`HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\*` 的 `path`。
//!
//! 另外，[`engine_files`] 按 exe 旁边的文件认出用游戏引擎、游戏平台 SDK 做的程序。
//!
//! 路径都转成小写、以 `\` 分隔。

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RegCloseKey, RegEnumKeyExW,
};
use windows::core::PWSTR;

use crate::startup::{open_key, reg_values};

#[derive(Clone, Debug, Default)]
pub struct KnownGames {
    /// 游戏 exe 的完整路径。
    pub exes: Vec<String>,
    /// 游戏库目录（以 `\` 结尾），其中的程序都算游戏。
    pub dirs: Vec<String>,
}

impl KnownGames {
    pub fn scan() -> Self {
        let mut exes: Vec<String> = sub_keys(HKEY_CURRENT_USER, GAME_CONFIG)
            .into_iter()
            .filter_map(|k| {
                reg_values(HKEY_CURRENT_USER, &format!("{GAME_CONFIG}\\{k}"))
                    .into_iter()
                    .find(|(n, _)| n == "MatchedExeFullPath")
                    .map(|(_, v)| normalize(&v))
            })
            .collect();
        exes.sort();
        exes.dedup();
        let mut dirs = steam_dirs();
        dirs.extend(epic_dirs());
        dirs.extend(gog_dirs());
        dirs.retain(|d| d.len() > 3);
        for d in &mut dirs {
            if !d.ends_with('\\') {
                d.push('\\');
            }
        }
        dirs.sort();
        dirs.dedup();
        Self { exes, dirs }
    }

    /// 某个 exe 是否是已知的游戏，以及依据。
    pub fn source(&self, path: &str) -> Option<GameSource> {
        let p = normalize(path);
        if self.exes.binary_search(&p).is_ok() {
            return Some(GameSource::Windows);
        }
        self.dirs
            .iter()
            .any(|d| p.starts_with(d.as_str()))
            .then_some(GameSource::Library)
    }

    /// 只有 exe 名、没有路径时：Windows 游戏列表里有同名 exe（不含扩展名，小写）。
    pub fn has_stem(&self, stem: &str) -> bool {
        self.exes.iter().any(|e| {
            let name = e.rsplit('\\').next().unwrap_or(e);
            name.strip_suffix(".exe").unwrap_or(name) == stem
        })
    }
}

/// exe 名或它所在目录里有游戏才会带的文件：Steam / Epic / GOG 的 SDK、Unity 与虚幻引擎的
/// 发行版、吉里吉里（`.xp3`）、RPG Maker（`RGSS*.dll`）、Ren'Py。
pub fn engine_files(path: &str) -> bool {
    let p = normalize(path);
    let Some((dir, exe)) = p.rsplit_once('\\') else {
        return false;
    };
    if exe.ends_with("-win64-shipping.exe") || exe.ends_with("-wingdk-shipping.exe") {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(format!("{dir}\\")) else {
        return false;
    };
    entries.flatten().any(|e| {
        let name = e.file_name().to_string_lossy().to_lowercase();
        matches!(
            name.as_str(),
            "steam_api.dll"
                | "steam_api64.dll"
                | "unityplayer.dll"
                | "eossdk-win64-shipping.dll"
                | "galaxy.dll"
                | "galaxy64.dll"
                | "renpy"
        ) || name.ends_with(".xp3")
            || (name.starts_with("rgss") && name.ends_with(".dll"))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameSource {
    /// Windows 游戏栏的已知游戏。
    Windows,
    /// 在 Steam / Epic / GOG 的游戏目录里。
    Library,
}

const GAME_CONFIG: &str = r"System\GameConfigStore\Children";

pub fn normalize(path: &str) -> String {
    path.trim().replace('/', "\\").to_lowercase()
}

fn sub_keys(root: HKEY, path: &str) -> Vec<String> {
    let Some(key) = open_key(root, path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0.. {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        let r = unsafe {
            RegEnumKeyExW(
                key,
                i,
                Some(PWSTR(name.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if r != ERROR_SUCCESS {
            break;
        }
        out.push(String::from_utf16_lossy(&name[..len as usize]));
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    out
}

fn steam_dirs() -> Vec<String> {
    let Some(root) = reg_values(HKEY_CURRENT_USER, r"Software\Valve\Steam")
        .into_iter()
        .find(|(n, _)| n == "SteamPath")
        .map(|(_, v)| v)
    else {
        return Vec::new();
    };
    let vdf =
        std::fs::read_to_string(format!("{root}/steamapps/libraryfolders.vdf")).unwrap_or_default();
    let mut libs = vdf_paths(&vdf);
    libs.push(root);
    libs.iter()
        .map(|l| normalize(&format!("{l}\\steamapps\\common\\")))
        .collect()
}

/// `libraryfolders.vdf` 里各库的 `"path"`。
fn vdf_paths(vdf: &str) -> Vec<String> {
    vdf.lines()
        .filter_map(|line| {
            let mut parts = line.split('"').skip(1).step_by(2);
            (parts.next()? == "path").then(|| parts.next().map(|p| p.replace("\\\\", "\\")))?
        })
        .collect()
}

fn epic_dirs() -> Vec<String> {
    let dir = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
    let Ok(entries) = std::fs::read_dir(format!(r"{dir}\Epic\EpicGamesLauncher\Data\Manifests"))
    else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "item"))
        .filter_map(|e| epic_game_dir(&std::fs::read_to_string(e.path()).ok()?))
        .collect()
}

/// Epic 清单（JSON）里类别含 games、且不是 DLC 的安装目录。只取两个字段，不必完整解析。
fn epic_game_dir(json: &str) -> Option<String> {
    let cats = json.split("\"AppCategories\"").nth(1)?;
    let cats = &cats[..cats.find(']')?];
    if !cats.contains("\"games\"") || cats.contains("\"addons\"") {
        return None;
    }
    let loc = json.split("\"InstallLocation\"").nth(1)?;
    let value = loc.split('"').nth(1)?;
    Some(normalize(&value.replace("\\\\", "\\")))
}

fn gog_dirs() -> Vec<String> {
    const GOG: &str = r"SOFTWARE\WOW6432Node\GOG.com\Games";
    sub_keys(HKEY_LOCAL_MACHINE, GOG)
        .into_iter()
        .filter_map(|k| {
            reg_values(HKEY_LOCAL_MACHINE, &format!("{GOG}\\{k}"))
                .into_iter()
                .find(|(n, _)| n == "path")
                .map(|(_, v)| normalize(&v))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_steam_libraries() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"E:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(
            vdf_paths(vdf),
            ["C:\\Program Files (x86)\\Steam", "E:\\SteamLibrary"]
        );
    }

    #[test]
    fn parses_epic_manifests() {
        let game = r#"{ "DisplayName": "DEATH STRANDING", "InstallLocation": "E:\\DeathStrandingDC", "AppCategories": [ "games", "applications" ] }"#;
        assert_eq!(epic_game_dir(game).as_deref(), Some("e:\\deathstrandingdc"));
        let dlc = r#"{ "InstallLocation": "E:\\Civ", "AppCategories": [ "addons" ] }"#;
        assert_eq!(epic_game_dir(dlc), None);
    }

    #[test]
    fn matches_exes_and_dirs() {
        let k = KnownGames {
            exes: vec![normalize(r"E:\GAME\Aokana\Aokana.exe")],
            dirs: vec![r"e:\steamlibrary\steamapps\common\".into()],
        };
        assert_eq!(
            k.source(r"e:/game/aokana/AOKANA.EXE"),
            Some(GameSource::Windows)
        );
        assert_eq!(
            k.source(r"E:\SteamLibrary\steamapps\common\Balatro\Balatro.exe"),
            Some(GameSource::Library)
        );
        assert_eq!(k.source(r"C:\Windows\explorer.exe"), None);
        assert!(k.has_stem("aokana"));
        assert!(!k.has_stem("explorer"));
    }

    #[test]
    fn engine_files_in_folder() {
        let dir = std::env::temp_dir().join(format!("pulse-engine-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("Game.exe");
        std::fs::write(&exe, b"").unwrap();
        assert!(!engine_files(&exe.to_string_lossy()));
        std::fs::write(dir.join("steam_api64.dll"), b"").unwrap();
        assert!(engine_files(&exe.to_string_lossy()));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(engine_files(
            r"X:\Nope\Pal\Binaries\Win64\Palworld-Win64-Shipping.exe"
        ));
    }

    #[test]
    #[ignore = "reads this machine's game lists"]
    fn scans_this_machine() {
        let k = KnownGames::scan();
        println!("{} exes, dirs: {:#?}", k.exes.len(), k.dirs);
    }
}
