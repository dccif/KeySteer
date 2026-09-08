//! Public hint algorithm options.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelDirection {
    #[default]
    Normal,
    Reverse,
}

use crate::api::geometry::Rect;
use smallvec::SmallVec;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HintCode(pub(crate) SmallVec<[u8; 8]>);

impl HintCode {
    pub(crate) fn as_str(&self) -> &str {
        // `push` is the only mutation path and appends complete encoded chars.
        // Keep the conversion safe even if that invariant changes later.
        std::str::from_utf8(&self.0).unwrap_or_default()
    }
}

pub struct CompactHint<T> {
    pub(crate) label: HintCode,
    pub(crate) bounds: Rect,
    pub(crate) value: T,
}
