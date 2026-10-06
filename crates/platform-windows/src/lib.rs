//! Win32/WinRT platform services behind core traits.

#[cfg(windows)]
pub mod clip;

pub mod desktop;

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
