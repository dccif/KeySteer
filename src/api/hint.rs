//! Public hint algorithm options.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelDirection {
    #[default]
    Normal,
    Reverse,
}
