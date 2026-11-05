//! Win32/WinRT platform services behind core traits.

#[cfg(windows)]
pub mod clip;

/// §5.1.2 开机启动那一枚开关在机器一侧的手。像 [`theme`] 一样无条件声明：判据（该写、该删、
/// 不必动；命令串；什么算一致）在一台没有注册表的机器上也要能被问一遍，只有 [`Registry`]
/// 那一格是 Windows 的。
///
/// [`Registry`]: autostart::Registry
pub mod autostart;

pub mod cursor;

#[cfg(windows)]
pub mod glyphs;

pub mod clock;
pub mod desktop;
/// §5.21.1 第 3 步「系统获取版本信息」里的那一只手：把一句 http(s) 地址取回成一段字。像
/// [`pick`] 一样无条件声明 —— 「这一台没有取回地址的那只手」也是一个答（[`fetch::FetchError::Unsupported`]），
/// 而不是一句没有声明；判「取回来的那串该怎么算版本」始终在 `falcon_core::update`。
pub mod fetch;
pub mod hotkey;
pub mod ocr;
pub mod open;
/// §5.8.3 第 1–2 步那一扇门：一扇真的 Windows 文件对话框，交回的是路径而不是像素。
/// 像 [`ocr`] 一样无条件声明 —— 「这一台没有文件对话框」也是一个答，而不是一句没有声明。
pub mod pick;
pub mod print;
pub mod proc;
pub mod reveal;
pub mod theme;
pub mod win;

/// The font here is DirectWrite's, and the MVP is Windows-only. Same arrangement as
/// [`clip`]: a machine without Windows still compiles the workspace and still runs
/// `cargo test`, and answers "no glyphs" - which is what [`NoGlyphs`] answers too,
/// so the text keeps its box and loses its letters rather than inventing either.
///
/// [`NoGlyphs`]: falcon_core::annotation::raster::NoGlyphs
#[cfg(not(windows))]
pub mod glyphs {
    use falcon_core::annotation::model::Style;
    use falcon_core::annotation::raster::{Glyphs, Ink};

    #[derive(Debug, Default)]
    pub struct DirectWrite;

    impl Glyphs for DirectWrite {
        fn ink(&self, _line: &str, _style: &Style) -> Option<Ink> {
            None
        }
    }
}

/// The clipboard here is the Win32 one, and the MVP is Windows-only. This
/// alternative exists so a machine without Windows still compiles the workspace
/// and still runs `cargo test` - every call answers "not on this platform"
/// rather than pretending to have done the work.
#[cfg(not(windows))]
pub mod clip {
    use falcon_core::clip::ClipFacts;
    use falcon_core::frame::Frame;
    use std::path::PathBuf;

    #[derive(Clone, Debug, Default)]
    pub struct Contents {
        pub image_bytes: Option<Vec<u8>>,
        pub html: Option<String>,
        pub text: Option<String>,
        pub files: Vec<PathBuf>,
    }

    impl Contents {
        pub fn facts(&self) -> ClipFacts<'_> {
            ClipFacts {
                image_bytes: self.image_bytes.as_deref(),
                html: self.html.as_deref(),
                text: self.text.as_deref(),
                files: &self.files,
            }
        }

        pub fn is_empty(&self) -> bool {
            self.image_bytes.is_none()
                && self.html.is_none()
                && self.text.is_none()
                && self.files.is_empty()
        }
    }

    #[derive(Clone, Debug)]
    pub struct ClipError(String);

    impl std::fmt::Display for ClipError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for ClipError {}

    fn absent() -> ClipError {
        ClipError("this platform has no clipboard implementation here".into())
    }

    pub fn read() -> Result<Contents, ClipError> {
        Err(absent())
    }

    pub fn write_image(_frame: &Frame) -> Result<(), ClipError> {
        Err(absent())
    }

    pub fn write_text(_text: &str) -> Result<(), ClipError> {
        Err(absent())
    }

    pub fn formats() -> Result<Vec<String>, ClipError> {
        Err(absent())
    }

    pub fn usable() -> Result<(), ClipError> {
        Err(absent())
    }
}
