//! Public hint algorithm options.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelDirection {
    #[default]
    Normal,
    Reverse,
}

/// Match groups used to order search-result previews, independent of label numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMatchKind {
    Label,
    Text,
    Pinyin,
}

pub const DEFAULT_SEARCH_MATCH_PRIORITY: [SearchMatchKind; 3] = [
    SearchMatchKind::Pinyin,
    SearchMatchKind::Text,
    SearchMatchKind::Label,
];

/// Precompiled ranks for label, text and initials match quality. Each quality
/// occupies two bits: whole word/code, prefix, substring, or absent. Quality
/// takes precedence; configured match groups break ties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledSearchPriority {
    ranks: [u8; 64],
}

impl CompiledSearchPriority {
    pub fn new(priority: [SearchMatchKind; 3]) -> Self {
        let mut ranks = [u8::MAX; 64];
        for (mask, result) in ranks.iter_mut().enumerate() {
            for (position, kind) in priority.iter().enumerate() {
                let shift = match kind {
                    SearchMatchKind::Label => 0,
                    SearchMatchKind::Text => 2,
                    SearchMatchKind::Pinyin => 4,
                };
                let quality = ((mask >> shift) & 3) as u8;
                if quality < 3 {
                    *result = (*result).min(quality * 3 + position as u8);
                }
            }
        }
        Self { ranks }
    }

    pub(crate) fn label_first(&self) -> bool {
        self.ranks[3 << 2 | 3 << 4] == 0
    }

    pub(crate) fn rank(&self, mask: usize) -> Option<u8> {
        let rank = self.ranks[mask];
        (rank != u8::MAX).then_some(rank)
    }
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

#[derive(Clone)]
pub struct CompactHint<T> {
    pub(crate) label: HintCode,
    /// Visual placement anchor. Hint may retain this within a refined control
    /// to avoid jumps; its authoritative click rectangle remains in UiTarget.
    pub(crate) bounds: Rect,
    pub(crate) value: T,
}
