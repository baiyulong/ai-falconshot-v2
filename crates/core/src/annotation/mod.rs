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
pub mod select;
pub mod undo;

pub use command::{
    alpha_command, clone_command, colour_command, corner_command, crop_command,
    delete_and_renumber_command, font_size_command, intensity_command, merge_styles, move_command,
    patch_style_command, remove_command, resize_command, resize_to_rect, style_command,
    text_command, width_command, Command, Dirty,
};
pub use model::{
    alpha_from_pct, drawn_corner, head_reach, outline_of, pct_from_alpha, Align, ArrowHead, Brush,
    Dash, Document, Element, Geom, Kind, Style, Transform, ALPHA_MIN_PCT, ALPHA_STEP_PCT,
    BLUR_RADIUS, CORNER_MAX, CORNER_MIN, EFFECT_MAX, EFFECT_MIN, FILL_ALPHA_PCT, FONT_MAX,
    FONT_MIN, KINDS, MARKER_ALPHA, MOSAIC_BLOCK, OUTLINE_WIDTH_PX, WIDTH_MAX, WIDTH_MIN,
};
pub use raster::{paint, render, Glyphs, Ink, NoGlyphs};
pub use select::{
    clamp_rotation, grip_at, grip_at_any, grip_point, radius_grip_command, resize_grip,
    resize_grip_command, rotate_at_command, rotate_command, rotation_at, segment_floor, Grip,
    Selection, GRIP_SLOP, MIN_SIZE, RADIUS_GRIP_INSET, ROTATE_LIMIT, ROTATE_OFFSET,
};
pub use undo::UndoStack;
