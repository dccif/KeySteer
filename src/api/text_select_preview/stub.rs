//! Compile away provisional OCR handling on platforms without macOS Vision.
use super::PreviewFrame;
use crate::api::{
    Rect, UiTarget,
    command::{UiScanResult, UiScanStatus},
    hint::CompactHint,
    text_select::Action,
};
use smallvec::SmallVec;
use std::{ops::Range, sync::Arc};

#[derive(Clone, Default)]
pub(crate) struct Regions {
    _private: (),
}

impl Regions {
    #[inline(always)]
    pub fn from_groups(_: &[(Rect, SmallVec<[&UiTarget; 2]>)]) -> Self {
        Self::default()
    }

    #[inline(always)]
    pub fn waiting(&self, _: &[bool]) -> bool {
        false
    }

    #[inline(always)]
    pub fn incomplete(&self, _: &[bool]) -> bool {
        false
    }

    #[inline(always)]
    pub fn reconcile(
        &mut self,
        _: &mut Arc<str>,
        _: &mut Vec<Range<usize>>,
        _: &UiScanResult,
    ) -> bool {
        false
    }

    #[inline(always)]
    pub fn write_deduplicated(
        &self,
        _: &[bool],
        _: &[Range<usize>],
        _: &Arc<str>,
        _: &mut String,
    ) -> bool {
        false
    }
}

#[derive(Default)]
pub(crate) struct Actions {
    _private: (),
}

// Keep the shared mode and frame layouts free of platform preview state.
const _: () = {
    assert!(std::mem::size_of::<Regions>() == 0);
    assert!(std::mem::size_of::<Actions>() == 0);
};

impl Actions {
    #[inline(always)]
    pub fn clear(&mut self) {}

    #[inline(always)]
    pub fn wait(&mut self, _: Action, _: bool, _: bool, _: &mut bool) -> bool {
        false
    }

    #[inline(always)]
    pub fn resolve(&mut self, _: &UiScanStatus, _: bool, _: bool) -> Option<Action> {
        None
    }
}

#[inline(always)]
pub(crate) fn is_fast_preview(_: &UiTarget) -> bool {
    false
}

#[inline(always)]
pub(crate) fn holds_labels(
    _: bool,
    _: bool,
    _: &Regions,
    _: &str,
    _: &[CompactHint<usize>],
) -> bool {
    false
}

#[inline(always)]
pub(crate) fn reconcile<T: PreviewFrame>(_: &mut T, _: &mut [T], _: &UiScanResult) -> bool {
    false
}
