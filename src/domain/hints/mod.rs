//! Hint label assignment, matching, and grid algorithms.

mod grid;
mod labels;
mod matcher;

pub use grid::{fit_grid, grid_labels};
pub use labels::{Hint, assign};
pub use matcher::{Match, match_input};
