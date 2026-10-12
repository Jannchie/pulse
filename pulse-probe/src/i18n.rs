//! 界面语言：简体中文 / 英文 / 日文，运行时切换。
//!
//! 文案不放资源文件，调用处用 [`t!`](crate::t) / [`tf!`](crate::tf) 把三种语言写在一起，
//! 按当前语言取其一。当前语言是进程级全局状态，UI 线程和采集线程都能读。
//!
//! 三种语言缺一不可，少写一个是编译错误，不会悄悄回退到别的语言：
//!
//! ```compile_fail
//! let _ = pulse_probe::t!("进程", "Processes");
//! ```

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Zh,
    En,
    Ja,
}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::Zh as u8);

/// 当前界面语言。
pub fn lang() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        0 => Lang::Zh,
        1 => Lang::En,
        _ => Lang::Ja,
    }
}

pub fn set_lang(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

/// 系统界面语言：主语言是中文时为 `Zh`，日文时为 `Ja`，其余一律 `En`。
pub fn system_lang() -> Lang {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;
    from_langid(unsafe { GetUserDefaultUILanguage() })
}

/// LANGID 低 10 位是主语言：LANG_CHINESE = 0x04，LANG_JAPANESE = 0x11。
fn from_langid(id: u16) -> Lang {
    match id & 0x3ff {
        0x04 => Lang::Zh,
        0x11 => Lang::Ja,
        _ => Lang::En,
    }
}

/// 按当前语言取中文、英文或日文的 `&'static str`：
/// `t!("结束任务", "End task", "タスクを終了する")`。
#[macro_export]
macro_rules! t {
    ($zh:literal, $en:literal, $ja:literal $(,)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::Zh => $zh,
            $crate::i18n::Lang::En => $en,
            $crate::i18n::Lang::Ja => $ja,
        }
    };
}

/// 带格式化的 [`t!`]，每个分支各自 `format!`，所以可以直接内联捕获变量：
/// `tf!("结束 {label}", "End {label}", "{label} を終了")`。
#[macro_export]
macro_rules! tf {
    ($zh:literal, $en:literal, $ja:literal $(, $arg:expr)* $(,)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::Zh => format!($zh $(, $arg)*),
            $crate::i18n::Lang::En => format!($en $(, $arg)*),
            $crate::i18n::Lang::Ja => format!($ja $(, $arg)*),
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
        assert_eq!(t!("进程", "Processes", "プロセス"), "Processes");
        assert_eq!(tf!("{n} 个", "{n} items", "{n} 個"), "3 items");
        set_lang(Lang::Ja);
        assert_eq!(lang(), Lang::Ja);
        assert_eq!(t!("进程", "Processes", "プロセス"), "プロセス");
        assert_eq!(tf!("{} 个", "{} items", "{} 個", n), "3 個");
        set_lang(Lang::Zh);
        assert_eq!(t!("进程", "Processes", "プロセス"), "进程");
        assert_eq!(tf!("{} 个", "{} items", "{} 個", n), "3 个");
    }

    #[test]
    fn maps_system_language() {
        assert_eq!(from_langid(0x0804), Lang::Zh); // zh-CN
        assert_eq!(from_langid(0x0404), Lang::Zh); // zh-TW
        assert_eq!(from_langid(0x0411), Lang::Ja); // ja-JP
        assert_eq!(from_langid(0x0409), Lang::En); // en-US
        assert_eq!(from_langid(0x0407), Lang::En); // de-DE
    }
}
