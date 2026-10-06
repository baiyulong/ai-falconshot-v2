//! The annotation model: a [`Document`] of [`Element`]s, the [`Command`]s that
//! change it, and the [`UndoStack`] that reverses them.
//!
//! 计划 §6.4 fixes the shapes, 技术方案 §8.2 fixes the flow: the core writes the
//! document and then files the command. Nothing here knows about pixels —
//! turning an `Element` into a `Frame` is the rasteriser's job, and drawing on a
//! screen is Qt's. Keeping the model free of both is what makes 5.7 editable on
//! a machine with no Qt installed.

pub mod command;
pub mod model;
pub mod undo;

pub use command::{
    clone_command, crop_command, merge_styles, move_command, remove_command, resize_command,
    resize_to_rect, style_command, Command, Dirty,
};
pub use model::{
    Align, ArrowHead, Brush, Dash, Document, Element, Geom, Kind, Style, Transform, KINDS,
};
pub use undo::UndoStack;
