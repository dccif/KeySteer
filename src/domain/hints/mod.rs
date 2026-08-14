//! Hint label assignment, matching, and grid algorithms.

mod grid;
mod labels;
mod matcher;

pub use grid::{fit_grid, grid_labels};
pub(crate) use labels::{CompactHint, assign_compact_into};
pub use labels::{Hint, assign, assign_into};
pub use matcher::{Match, match_input};
