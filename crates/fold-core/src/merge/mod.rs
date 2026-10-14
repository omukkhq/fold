mod diff3;
mod markers;

pub use diff3::{ConflictHunk, Merge3, Segment, merge3};
pub use markers::{contains_marker_line, render_markers};
