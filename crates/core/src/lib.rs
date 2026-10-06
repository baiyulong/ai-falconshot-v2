//! Product logic for AI Falconshot. Nothing in this crate may depend on Qt, so the
//! whole model layer is unit-testable on a machine with no Qt installed.

pub mod annotation;
pub mod capture;
pub mod clip;
pub mod colors;
pub mod config;
pub mod encode;
pub mod frame;
pub mod geometry;
pub mod history;
pub mod hotcorner;
pub mod hotkeys;
pub mod imageops;
pub mod naming;
pub mod pin;
pub mod tasks;

pub use capture::{CaptureError, CaptureService, FrameSource, MonitorInfo, WindowInfo};
pub use frame::Frame;
pub use geometry::{PhysPoint, PhysRect, PhysSize, Scale};

pub const APP_ID: &str = "dev.falconshot.app";
