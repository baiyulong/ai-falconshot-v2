//! The annotation model: a [`Document`] of [`Element`]s, the [`Command`]s that
//! change it, and the [`UndoStack`] that reverses them.
//!
//! 计划 §6.4 fixes the shapes, 技术方案 §8.2 fixes the flow: the core writes the
//! document and then files the command. The model, the commands and the stack
//! know nothing about pixels; [`raster`] is the one place that turns an
//! `Element` into a `Frame`, and drawing on a screen is still Qt's job. Keeping
//! the first three free of both is what makes 5.7 editable on a machine with no
//! Qt installed.

pub mod command;
pub mod model;
pub mod raster;
pub mod undo;

pub use command::{
    clone_command, crop_command, merge_styles, move_command, remove_command, resize_command,
    resize_to_rect, style_command, Command, Dirty,
};
pub use model::{
    head_reach, Align, ArrowHead, Brush, Dash, Document, Element, Geom, Kind, Style, Transform,
    KINDS,
};
pub use raster::{paint, render, Glyphs, Ink, NoGlyphs};
pub use undo::UndoStack;
