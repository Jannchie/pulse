//! 按家族名在已安装字体里挑字体。
//!
//! 字体不打包进 exe：从注册表的已安装字体清单里按家族名找文件，读字体自己的 name 表
//! 核对家族名、字重和 .ttc 里的字面序号。选中的文件只内存映射一次，切换语言时复用，
//! 文件页可与其他进程共享，不计入私有内存。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use skrifa::raw::{FileRef, TableProvider as _};
use skrifa::{MetadataProvider as _, Tag, string::StringId};

/// 选中的一个字面。
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub family: &'static str,
    pub path: PathBuf,
    /// .ttc 里的字面序号，单个字体为 0。
    pub index: u32,
    /// 可变字体的 wght 坐标；静态字体为 `None`。
    pub wght: Option<f32>,
    /// 静态字体的字重（OS/2 usWeightClass）；可变字体同 `wght`。
    pub weight: f32,
}

impl Face {
    /// 映射后的文件内容，同一个文件只映射一次。
    pub fn bytes(&self) -> Option<&'static [u8]> {
        map_once(&self.path)
    }

    /// 在 egui 字体表里的键，不同字面、不同可变坐标各不相同。
    pub fn key(&self) -> String {
        let stem = self.path.file_name().unwrap_or_default().to_string_lossy();
        match self.wght {
            Some(w) => format!("{stem}#{}@{w}", self.index),
            None => format!("{stem}#{}", self.index),
        }
    }
}

/// 按优先级找第一个装了的家族，取最接近 `weight` 的正体字面。
pub fn pick(families: &[&'static str], weight: f32) -> Option<Face> {
    families.iter().find_map(|&f| find(f, weight))
}

struct Candidate {
    path: PathBuf,
    index: u32,
    weight: f32,
    /// 可变字体 wght 轴的范围。
    range: Option<(f32, f32)>,
}

fn find(family: &'static str, weight: f32) -> Option<Face> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Vec<Candidate>>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    let candidates = cache.entry(family).or_insert_with(|| scan(family));
    let target = |c: &Candidate| match c.range {
        Some((lo, hi)) => weight.clamp(lo, hi),
        None => c.weight,
    };
    // 距离相同时：常规字重偏向更细的，加粗字重偏向更粗的，保证加粗不会比正文还细。
    let best = candidates.iter().min_by(|a, b| {
        let (da, db) = ((target(a) - weight).abs(), (target(b) - weight).abs());
        da.total_cmp(&db).then_with(|| {
            let (ta, tb) = (target(a), target(b));
            if weight > 450.0 {
                tb.total_cmp(&ta)
            } else {
                ta.total_cmp(&tb)
            }
        })
    })?;
    Some(Face {
        family,
        path: best.path.clone(),
        index: best.index,
        wght: best.range.map(|_| target(best)),
        weight: target(best),
    })
}

/// 注册表显示名里有这个家族名的文件，逐个读 name 表确认。
fn scan(family: &str) -> Vec<Candidate> {
    static INSTALLED: OnceLock<Vec<(String, PathBuf)>> = OnceLock::new();
    let installed = INSTALLED.get_or_init(pulse_probe::fonts::installed);
    let lower = family.to_ascii_lowercase();
    let mut seen = Vec::<&Path>::new();
    let mut out = Vec::new();
    for (name, path) in installed {
        let listed = name
            .to_ascii_lowercase()
            .split('&')
            .any(|part| part.trim().starts_with(&lower));
        if !listed || seen.contains(&path.as_path()) {
            continue;
        }
        seen.push(path);
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        // 临时映射，只读表头；选中后由 map_once 另行常驻。
        let Ok(map) = (unsafe { memmap2::Mmap::map(&file) }) else {
            continue;
        };
        let Ok(file) = FileRef::new(&map) else {
            continue;
        };
        for (index, font) in file.fonts().enumerate() {
            let Ok(font) = font else { continue };
            if !family_matches(&font, family) {
                continue;
            }
            let attrs = font.attributes();
            if attrs.style != skrifa::attribute::Style::Normal {
                continue;
            }
            let range = font
                .axes()
                .get_by_tag(Tag::new(b"wght"))
                .map(|a| (a.min_value(), a.max_value()));
            out.push(Candidate {
                path: path.clone(),
                index: index as u32,
                weight: attrs.weight.value(),
                range,
            });
        }
    }
    out
}

/// 字面的家族名（typographic family 优先，其次 family）与要找的一致。
/// 只认英文名：中日文字体的 name 表同时带本地化名，egui 用不到。
fn family_matches(font: &skrifa::FontRef, family: &str) -> bool {
    // 有 OS/2 表才是正常的字体文件。
    if font.os2().is_err() {
        return false;
    }
    [StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME]
        .into_iter()
        .any(|id| {
            font.localized_strings(id)
                .filter(|s| s.language().is_none_or(|l| l.starts_with("en")))
                .any(|s| s.chars().collect::<String>().eq_ignore_ascii_case(family))
        })
}

/// 同一个文件只映射一次，之后一直复用（字体在进程退出前都要用）。
fn map_once(path: &Path) -> Option<&'static [u8]> {
    static MAPS: OnceLock<Mutex<HashMap<PathBuf, &'static [u8]>>> = OnceLock::new();
    let mut maps = MAPS.get_or_init(Default::default).lock().ok()?;
    if let Some(&bytes) = maps.get(path) {
        return Some(bytes);
    }
    let file = std::fs::File::open(path).ok()?;
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let bytes: &'static [u8] = Box::leak(Box::new(map));
    maps.insert(path.to_path_buf(), bytes);
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_file_once() {
        let Some(face) = pick(&["Segoe UI"], 400.0) else {
            return;
        };
        let a = face.bytes().unwrap();
        let b = face.bytes().unwrap();
        assert_eq!(a.as_ptr(), b.as_ptr());
    }

    #[test]
    fn picks_weights_from_collections() {
        // Yu Gothic UI 的常规体在 YuGothM.ttc，Semibold 在 YuGothB.ttc（.ttc 里的第二个字面）。
        let (Some(regular), Some(semibold)) = (
            pick(&["Yu Gothic UI"], 400.0),
            pick(&["Yu Gothic UI"], 550.0),
        ) else {
            return;
        };
        assert_eq!(regular.weight, 400.0);
        assert_eq!(semibold.weight, 600.0);
        assert_ne!(regular.path, semibold.path);
    }

    #[test]
    fn missing_family_is_none() {
        assert_eq!(pick(&["No Such Font Family 1234"], 400.0), None);
    }
}
