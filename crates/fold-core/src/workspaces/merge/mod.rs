mod diff3;
mod markers;
mod origin;
mod report;
mod tree;

pub use diff3::{ConflictHunk, Merge3, Segment, merge3};
pub use markers::{contains_marker_line, render_markers, sync_labels};
pub use origin::{OriginRun, line_origins};
pub use report::{
    ConflictReport, HunkSide, ReportEntry, ReportEntryKind, ReportFile, ReportHunk, ReportKind,
    ReportTrees,
};
pub use tree::{BlobLoader, Conflict, ConflictKind, TreeMerge, merge_trees};
