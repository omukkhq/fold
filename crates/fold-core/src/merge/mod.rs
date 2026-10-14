mod diff3;
mod markers;
mod tree;

pub use diff3::{ConflictHunk, Merge3, Segment, merge3};
pub use markers::{contains_marker_line, render_markers};
pub use tree::{BlobLoader, Conflict, ConflictKind, TreeMerge, merge_trees};
