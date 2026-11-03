//! Shared modal entry and bounds resolution for region-limited scan modes.

use crate::api::{
    Command, CommandBatch, HostContext, LifecycleAction, ModeId, Rect, Screen, UiScanScope,
    command::UiScanResult,
};
use std::sync::atomic::{AtomicU64, Ordering};

// Scan results may already be queued when a mode is cancelled or reloaded.
// Allocate one generation space for both consumers so a late result cannot
// be accepted by a newly entered mode with the same local counter value.
static NEXT_SCAN_ID: AtomicU64 = AtomicU64::new(1);

pub(super) fn next_scan_id() -> u64 {
    // The high bit is reserved for UIHint's panel requests.
    NEXT_SCAN_ID.fetch_add(1, Ordering::Relaxed) & (u64::MAX >> 1)
}

#[derive(Default)]
pub(super) struct ScanRegionSession {
    // The mode already owns its current scan bounds; only record whether those
    // bounds were supplied by a caller and must survive a rescan.
    bounded: bool,
    modal: bool,
}

pub(super) struct ScanArea {
    pub scope: UiScanScope,
    pub bounds: Rect,
    pub screen_bounds: Vec<Rect>,
}

impl ScanRegionSession {
    pub fn reset(&mut self) {
        self.bounded = false;
        self.modal = false;
    }

    pub fn push(&mut self, bounds: Option<Rect>) {
        self.bounded = bounds.is_some();
        self.modal = true;
    }

    pub fn bounds(&self, current: Option<Rect>) -> Option<Rect> {
        if self.bounded { current } else { None }
    }

    pub fn is_modal(&self) -> bool {
        self.modal
    }

    pub fn is_bounded(&self) -> bool {
        self.bounded
    }

    pub fn label_bounds(
        &self,
        ctx: &HostContext<'_>,
        current: Option<Rect>,
    ) -> Option<crate::api::presentation::HintLabelBounds> {
        self.bounds(current).map(|area| {
            let screen = Screen::containing(ctx.screens, &area.center());
            crate::api::presentation::HintLabelBounds::new(
                screen.map_or_else(|| ctx.active_bounds(), |screen| screen.bounds),
                screen.map_or(1.0, |screen| screen.scale),
            )
        })
    }

    pub fn area(
        &self,
        ctx: &HostContext<'_>,
        configured_scope: UiScanScope,
        current: Option<Rect>,
    ) -> ScanArea {
        if let Some(bounds) = self.bounds(current) {
            return ScanArea {
                scope: UiScanScope::Screen,
                bounds,
                screen_bounds: Vec::new(),
            };
        }
        let screens = if configured_scope == UiScanScope::All {
            ctx.screens
        } else {
            ctx.active_screen()
                .map(std::slice::from_ref)
                .unwrap_or_default()
        };
        ScanArea {
            scope: configured_scope,
            bounds: Screen::virtual_bounds(screens),
            screen_bounds: if screens.len() > 1 {
                screens.iter().map(|screen| screen.bounds).collect()
            } else {
                Vec::new()
            },
        }
    }

    pub fn return_command(&self, return_mode: &ModeId) -> Command {
        if self.modal {
            Command::PopMode
        } else {
            Command::SwitchMode(return_mode.clone())
        }
    }

    pub fn lifecycle_commands(
        &self,
        action: &LifecycleAction,
        return_mode: &ModeId,
    ) -> CommandBatch {
        if self.modal
            && (matches!(action, LifecycleAction::Return)
                || matches!(action, LifecycleAction::Mode(mode) if mode == return_mode))
        {
            CommandBatch::one(Command::PopMode)
        } else {
            super::targeting::lifecycle_commands(action, return_mode)
        }
    }
}

#[cold]
#[inline(never)]
pub(super) fn retain_region_results(result: &mut UiScanResult, bounds: Rect) {
    result
        .targets
        .retain(|target| bounds.contains(&target.rect.center()));
    result
        .retired
        .retain(|rect| bounds.contains(&rect.center()));
}
