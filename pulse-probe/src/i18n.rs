//! 界面语言：简体中文 / 英文，运行时切换。
//!
//! 文案不放资源文件，调用处用 [`t!`](crate::t) / [`tf!`](crate::tf) 把两种语言写在一起，
//! 按当前语言取其一。当前语言是进程级全局状态，UI 线程和采集线程都能读。

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::Zh as u8);

/// 当前界面语言。
pub fn lang() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        0 => Lang::Zh,
        _ => Lang::En,
    }
}

pub fn set_lang(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

/// 系统界面语言：主语言是中文时为 `Zh`，其余一律 `En`。
pub fn system_lang() -> Lang {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;
    // LANGID 低 10 位是主语言，LANG_CHINESE = 0x04。
    const LANG_CHINESE: u16 = 0x04;
    if unsafe { GetUserDefaultUILanguage() } & 0x3ff == LANG_CHINESE {
        Lang::Zh
    } else {
        Lang::En
    }
}

/// 按当前语言取中文或英文的 `&'static str`：`t!("结束任务", "End task")`。
#[macro_export]
macro_rules! t {
    ($zh:literal, $en:literal $(,)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::Zh => $zh,
            $crate::i18n::Lang::En => $en,
        }
    };
}

/// 带格式化的 [`t!`]，两个分支各自 `format!`，所以可以直接内联捕获变量：
/// `tf!("结束 {label}", "End {label}")`。
#[macro_export]
macro_rules! tf {
    ($zh:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::Zh => format!($zh $(, $arg)*),
            $crate::i18n::Lang::En => format!($en $(, $arg)*),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_language() {
        let n = 3;
        set_lang(Lang::En);
        assert_eq!(t!("进程", "Processes"), "Processes");
        assert_eq!(tf!("{n} 个", "{n} items"), "3 items");
        set_lang(Lang::Zh);
        assert_eq!(t!("进程", "Processes"), "进程");
        assert_eq!(tf!("{} 个", "{} items", n), "3 个");
    }
}
