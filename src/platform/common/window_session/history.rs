//! Compact restoration checkpoints and bounded undo/redo history.
use super::*;
/// Undo state contains no titles or application strings. Metadata stays in the
/// native inventory and is refreshed when a placement is restored.
#[derive(Clone, Copy, Debug)]
pub(super) struct PlacementSnapshot {
    pub(super) info: PlacementInfo,
    pub(super) restored: Rect,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct PlacementInfo {
    pub(super) id: WindowId,
    pub(super) bounds: Rect,
    pub(super) screen: usize,
    pub(super) resizable: bool,
    pub(super) maximized: bool,
    pub(super) minimized: bool,
    pub(super) fullscreen: bool,
}
impl From<&Snapshot> for PlacementSnapshot {
    fn from(snapshot: &Snapshot) -> Self {
        let info = &snapshot.info;
        Self {
            info: PlacementInfo {
                id: info.id,
                bounds: info.bounds,
                screen: info.screen,
                resizable: info.resizable,
                maximized: info.maximized,
                minimized: info.minimized,
                fullscreen: info.fullscreen,
            },
            restored: snapshot.restored,
        }
    }
}
impl From<Snapshot> for PlacementSnapshot {
    fn from(snapshot: Snapshot) -> Self {
        Self::from(&snapshot)
    }
}
impl From<&PlacementSnapshot> for PlacementSnapshot {
    fn from(snapshot: &PlacementSnapshot) -> Self {
        *snapshot
    }
}
impl PlacementSnapshot {
    pub(super) fn apply_to(&self, snapshot: &mut Snapshot) {
        snapshot.info.bounds = self.info.bounds;
        snapshot.info.screen = self.info.screen;
        snapshot.info.resizable = self.info.resizable;
        snapshot.info.maximized = self.info.maximized;
        snapshot.info.minimized = self.info.minimized;
        snapshot.info.fullscreen = self.info.fullscreen;
        snapshot.restored = self.restored;
    }
    pub(super) fn restore(
        &self,
        access: &mut impl WindowAccess,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        let mut snapshot = access.snapshot(self.info.id, screens)?;
        self.apply_to(&mut snapshot);
        access.restore(&snapshot, screens, cancelled)
    }
}

impl Session {
    pub(super) fn recover_edit(&mut self, access: &mut impl WindowAccess) {
        if let Some(edit) = self.edit.take() {
            // Only native partial failure uses bounded entry-layout recovery.
            let deadline = Instant::now() + Duration::from_millis(1500);
            for before in edit.before.iter().rev() {
                if Instant::now() >= deadline {
                    crate::report_error!("window-worker", "entry-layout recovery timed out");
                    break;
                }
                if let Err(error) =
                    before.restore(access, &self.screens, &|| Instant::now() >= deadline)
                {
                    crate::report_error!(
                        "window-worker",
                        "recover window {:?}: {error}",
                        before.info.id
                    );
                }
            }
        }
    }

    pub(super) fn remember(&mut self, group: u64, before: impl Into<PlacementSnapshot>) {
        let before = before.into();
        self.initial.entry(before.info.id).or_insert(before);
        self.changed_windows.insert(before.info.id);
        self.redo.clear();
        if self.history.back().is_none_or(|(g, _)| *g != group) {
            if self.history.len() == 32 {
                self.history.pop_front();
            }
            self.history.push_back((group, Vec::new()));
        }
        if let Some((_, snapshots)) = self.history.back_mut()
            && !snapshots.iter().any(|s| s.info.id == before.info.id)
        {
            snapshots.push(before);
        }
    }

    pub(super) fn capture_initial(
        &mut self,
        access: &impl WindowAccess,
        id: WindowId,
        screens: &[Screen],
    ) {
        if let std::collections::btree_map::Entry::Vacant(entry) = self.initial.entry(id)
            && let Ok(snapshot) = access.snapshot(id, screens)
        {
            entry.insert((&snapshot).into());
        }
    }

    pub(super) fn push_history(
        stack: &mut VecDeque<(u64, Vec<PlacementSnapshot>)>,
        group: u64,
        snapshots: Vec<PlacementSnapshot>,
    ) {
        if snapshots.is_empty() {
            return;
        }
        if stack.len() == 32 {
            stack.pop_front();
        }
        stack.push_back((group, snapshots));
    }

    // Return inverse snapshots for every actual native change, and retain
    // uncompleted work so cancellation or a refused write can be retried.
    pub(super) fn restore_snapshots(
        access: &mut impl WindowAccess,
        mut snapshots: Vec<PlacementSnapshot>,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
        result: &mut WindowResult,
    ) -> (Vec<PlacementSnapshot>, Vec<PlacementSnapshot>) {
        let mut inverse = Vec::new();
        let mut remaining = Vec::new();
        // One geometry operation per live group, preferring its stable anchor.
        snapshots.sort_by_key(|s| access.layout_representative(s.info.id) != s.info.id);
        let mut represented = std::collections::BTreeSet::new();
        snapshots.retain(|s| represented.insert(access.layout_representative(s.info.id)));
        while let Some(desired) = snapshots.pop() {
            if cancelled() {
                snapshots.push(desired);
                remaining.extend(snapshots);
                break;
            }
            let Ok(before) = access.snapshot(desired.info.id, screens) else {
                remaining.push(desired);
                result.skipped += 1;
                continue;
            };
            if same_placement(&before, desired) {
                continue;
            }
            let applied = desired.restore(access, screens, cancelled);
            let after = access.snapshot(desired.info.id, screens);
            if let Ok(after) = &after {
                if !same_placement(&before, after) {
                    inverse.push((&before).into());
                    result.changed += 1;
                }
                if same_placement(after, desired) {
                    continue;
                }
            } else {
                // The write may have succeeded before the read timed out.
                // Keep its inverse without claiming an observed state change.
                inverse.push((&before).into());
            }
            remaining.push(desired);
            result.skipped += 1;
            if let Err(error) = applied
                && !cancelled()
            {
                crate::report_error!("window-worker", "restore history: {error}");
            }
        }
        (inverse, remaining)
    }
}
