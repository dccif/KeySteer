//! Hint label assignment, matching, and grid algorithms.

mod grid;
mod labels;
mod matcher;
mod visual_layers;

pub use grid::{fit_grid, grid_labels};
pub(crate) use labels::{CompactHint, assign_compact_into};
pub use labels::{Hint, assign, assign_into};
pub use matcher::{Match, match_input};
pub(crate) use visual_layers::{VisualLayerPlan, build_visual_layer_plan};
