//! Worker-owned persistent groups and transactional native placement.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use super::window_session::{Snapshot, WindowAccess};
use super::window_tab_model::Groups;
use crate::api::window::{WindowId, WindowInfo};
use crate::api::window_tabs::{
    TabBar, TabDrop, TabNativeEvent, TabOperation, TabState, WindowTarget,
};
use crate::api::{Point, Rect, Screen};

#[derive(Clone)]
struct Checkpoint {
    groups: Groups,
    windows: Vec<Snapshot>,
}

pub(crate) struct Grouped<A> {
    numbers_dirty: bool,
    scope: Option<crate::api::window::WindowScope>,
    native: A,
    groups: Groups,
    history: VecDeque<Checkpoint>,
    redo: VecDeque<Checkpoint>,
    observed: BTreeMap<WindowId, Snapshot>,
    bars: Vec<TabBar>,
    watched: Vec<WindowId>,
    retired: Vec<WindowId>,
    hidden: BTreeSet<WindowId>,
    screens: Vec<Screen>,
}

fn same_frame(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() <= 2.0
        && (a.y - b.y).abs() <= 2.0
        && (a.width - b.width).abs() <= 2.0
        && (a.height - b.height).abs() <= 2.0
}
#[cfg(test)]
fn same_state(a: &Snapshot, b: &Snapshot) -> bool {
    same_frame(a.info.bounds, b.info.bounds)
        && a.info.minimized == b.info.minimized
        && a.info.maximized == b.info.maximized
        && a.info.fullscreen == b.info.fullscreen
}

impl<A: WindowAccess> Grouped<A> {
    pub fn new(native: A) -> Self {
        Self {
            numbers_dirty: false,
            scope: None,
            native,
            groups: Groups::default(),
            history: VecDeque::new(),
            redo: VecDeque::new(),
            observed: BTreeMap::new(),
            bars: Vec::new(),
            watched: Vec::new(),
            retired: Vec::new(),
            hidden: BTreeSet::new(),
            screens: Vec::new(),
        }
    }
    pub fn persistent(&self) -> bool {
        !self.groups.state.groups.is_empty() || !self.hidden.is_empty()
    }

    fn checkpoint(&self, extra: &[WindowId], screens: &[Screen]) -> Checkpoint {
        let mut ids: BTreeSet<_> = self
            .groups
            .state
            .groups
            .iter()
            .flat_map(|g| g.members.iter().copied())
            .collect();
        ids.extend(extra.iter().copied());
        Checkpoint {
            groups: self.groups.clone(),
            windows: ids
                .into_iter()
                .filter_map(|id| self.native.snapshot(id, screens).ok())
                .collect(),
        }
    }
    fn push(stack: &mut VecDeque<Checkpoint>, checkpoint: Checkpoint) {
        if stack.len() == 32 {
            stack.pop_front();
        }
        stack.push_back(checkpoint);
    }
    fn remember(&mut self, before: Checkpoint) {
        if !before.groups.same_membership(&self.groups) {
            Self::push(&mut self.history, before);
            self.redo.clear();
        }
    }
    fn rollback(&mut self, before: &Checkpoint, screens: &[Screen]) -> Result<(), String> {
        self.release_visibility(None)?;
        let deadline = Instant::now() + Duration::from_millis(1500);
        let mut errors = Vec::new();
        for snapshot in before.windows.iter().rev() {
            if Instant::now() >= deadline {
                errors.push("rollback timed out".to_string());
                break;
            }
            if let Err(error) = self
                .native
                .restore(snapshot, screens, &|| Instant::now() >= deadline)
            {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
    fn affected(&self, id: WindowId) -> Vec<WindowId> {
        self.groups
            .state
            .containing(id)
            .map_or_else(|| vec![id], |g| g.members.clone())
    }
    fn release_visibility(&mut self, affected: Option<&[WindowId]>) -> Result<(), String> {
        let release: Vec<_> = self
            .hidden
            .iter()
            .copied()
            .filter(|id| affected.is_none_or(|ids| ids.contains(id)))
            .collect();
        for id in release {
            self.native.tab_set_hidden(id, false)?;
            self.hidden.remove(&id);
        }
        let keep: Vec<_> = self
            .bars
            .iter()
            .filter(|bar| {
                affected.is_some_and(|ids| !bar.tabs.iter().any(|(id, _, _)| ids.contains(id)))
            })
            .cloned()
            .collect();
        self.release_header_space(&keep)?;
        self.native.tab_bars(&keep)?;
        self.bars = keep;
        Ok(())
    }
    fn cache_observed(&mut self, screens: &[Screen]) {
        self.observed.clear();
        for id in self.groups.state.groups.iter().flat_map(|g| &g.members) {
            if let Ok(snapshot) = self.native.snapshot(*id, screens) {
                self.observed.insert(*id, snapshot);
            }
        }
    }
    fn release_header_space(&mut self, keep: &[TabBar]) -> Result<(), String> {
        for bar in self
            .bars
            .iter()
            .filter(|bar| !keep.iter().any(|next| next.group == bar.group))
        {
            let Ok(snapshot) = self.native.snapshot(bar.active, &self.screens) else {
                continue;
            };
            if snapshot.info.maximized
                && !snapshot.info.minimized
                && let Some(screen) = self.screens.get(snapshot.info.screen)
                && self.native.tab_bar_height(screen) > 0.0
            {
                self.native
                    .tab_fit_frame(bar.active, screen.work_area, &self.screens, &|| false)?;
            }
        }
        Ok(())
    }
    fn publish(&mut self, screens: &[Screen]) -> Result<(), String> {
        if self.screens != screens {
            self.screens = screens.to_vec();
        }
        let desired: BTreeSet<_> = self
            .groups
            .state
            .groups
            .iter()
            .flat_map(|g| g.members.iter().copied().filter(|id| *id != g.active))
            .collect();
        for id in self
            .hidden
            .difference(&desired)
            .copied()
            .collect::<Vec<_>>()
        {
            self.native.tab_set_hidden(id, false)?;
            self.hidden.remove(&id);
        }
        for id in desired
            .difference(&self.hidden)
            .copied()
            .collect::<Vec<_>>()
        {
            self.native.tab_set_hidden(id, true)?;
            self.hidden.insert(id);
        }
        let watched: Vec<_> = self
            .groups
            .state
            .groups
            .iter()
            .flat_map(|g| g.members.iter().copied())
            .collect();
        if watched != self.watched {
            self.native.tab_watch(&watched)?;
            self.watched = watched;
        }
        let mut bars = Vec::new();
        for group in &self.groups.state.groups {
            let Ok(active) = self.native.snapshot(group.active, screens) else {
                continue;
            };
            let visible = !active.info.minimized
                && !active.info.fullscreen
                && self.native.tab_visible(group.active);
            let tabs = group
                .members
                .iter()
                .filter_map(|id| {
                    let window = self
                        .observed
                        .get(id)
                        .cloned()
                        .or_else(|| self.native.snapshot(*id, screens).ok())?;
                    let number = self
                        .groups
                        .state
                        .numbers
                        .iter()
                        .find(|(w, _)| w == id)
                        .map_or(0, |(_, number)| *number);
                    Some((*id, number, window.info.title))
                })
                .collect();
            bars.push(TabBar {
                visible,
                group: group.id,
                bounds: active.info.bounds,
                screen: active.info.screen,
                active: group.active,
                tabs,
            });
        }
        if bars != self.bars {
            self.release_header_space(&bars)?;
            self.native.tab_bars(&bars)?;
            self.bars = bars;
        }
        Ok(())
    }

    fn align(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let members = self.affected(id);
        let source = self.native.snapshot(id, screens)?;
        let mut rect = if source.info.minimized {
            source.restored
        } else {
            source.info.bounds
        };
        for member in &members {
            self.native.tab_eligible(*member, screens)?;
            let minimum = self.native.minimum_size(*member);
            rect.width = rect.width.max(minimum.x);
            rect.height = rect.height.max(minimum.y);
        }
        let display = screens
            .get(source.info.screen)
            .ok_or("Display is unavailable")?;
        let header = self.native.tab_bar_height(display);
        rect.height = rect.height.min(display.work_area.height - header);
        let minimum = members
            .iter()
            .map(|id| self.native.minimum_size(*id).y)
            .fold(0.0_f64, f64::max);
        if rect.width > display.work_area.width || rect.height < minimum {
            return Err("The members' minimum sizes do not fit this display".into());
        }
        rect.x = rect.x.clamp(
            display.work_area.x,
            display.work_area.x + display.work_area.width - rect.width,
        );
        rect.y = rect.y.clamp(
            display.work_area.y + header,
            display.work_area.y + display.work_area.height - rect.height,
        );
        // Only prepare the chosen member. Inactive members keep their geometry.
        let active = self.groups.state.containing(id).map_or(id, |g| g.active);
        self.native.tab_set_hidden(active, true)?;
        self.hidden.insert(active);
        let applied = self.native.set_frame(active, rect, screens, cancelled)?;
        if !same_frame(applied.bounds, rect) {
            return Err("A member refused the shared window size".into());
        }
        Ok(())
    }

    fn header_height(&self, screen: usize, screens: &[Screen]) -> f64 {
        screens
            .get(screen)
            .map_or(0.0, |screen| self.native.tab_bar_height(screen))
    }
    fn outer_frame(rect: Rect, header: f64) -> Rect {
        Rect::new(rect.x, rect.y - header, rect.width, rect.height + header)
    }
    fn content_frame(rect: Rect, header: f64) -> Result<Rect, String> {
        if rect.height <= header {
            return Err("The layout has no room below the tab strip".into());
        }
        Ok(Rect::new(
            rect.x,
            rect.y + header,
            rect.width,
            rect.height - header,
        ))
    }
    fn fit_header(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let snapshot = self.native.snapshot(id, screens)?;
        if snapshot.info.minimized || snapshot.info.fullscreen {
            return Ok(());
        }
        let screen = screens
            .get(snapshot.info.screen)
            .ok_or("Display is unavailable")?;
        let header = self.native.tab_bar_height(screen);
        if header <= 0.0 {
            return Ok(());
        }
        let mut bounds = snapshot.info.bounds;
        let available = screen.work_area.height - header;
        if self.native.minimum_size(id).y > available {
            return Err("The window and tab strip do not fit this display".into());
        }
        bounds.height = bounds.height.min(available);
        bounds.y = bounds.y.clamp(
            screen.work_area.y + header,
            screen.work_area.bottom() - bounds.height,
        );
        if !same_frame(bounds, snapshot.info.bounds) {
            self.native.tab_fit_frame(id, bounds, screens, cancelled)?;
        }
        Ok(())
    }

    fn minimize_group(
        &mut self,
        active: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let Some(group) = self
            .groups
            .state
            .containing(active)
            .filter(|g| g.active == active)
        else {
            return Ok(());
        };
        let members = group.members.clone();
        if !self.native.snapshot(active, screens)?.info.minimized {
            return Ok(());
        }
        for id in members {
            if cancelled() {
                return Err("Window grouping cancelled".into());
            }
            if !self.native.snapshot(id, screens)?.info.minimized {
                self.native.tab_minimize(id, screens, cancelled)?;
            }
        }
        Ok(())
    }

    fn activate(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        if cancelled() {
            return Err("Window grouping cancelled".into());
        }
        let bar = self
            .groups
            .state
            .containing(id)
            .and_then(|group| self.bars.iter().find(|bar| bar.group == group.id))
            .or_else(|| {
                self.hidden
                    .contains(&id)
                    .then(|| {
                        self.bars
                            .iter()
                            .find(|bar| bar.tabs.iter().any(|(member, _, _)| *member == id))
                    })
                    .flatten()
            })
            .filter(|bar| bar.active != id)
            .cloned();
        if let Some(bar) = bar {
            // Read the previous active window now, so a physical drag since
            // the last notification is included. Closed members use the last bar.
            let source = self.native.snapshot(bar.active, screens).ok();
            let rect = source
                .as_ref()
                .filter(|s| !s.info.minimized)
                .map_or(bar.bounds, |s| s.info.bounds);
            let before = self.native.snapshot(id, screens)?;
            self.native.tab_set_hidden(id, true)?;
            self.hidden.insert(id);
            if !same_frame(before.info.bounds, rect)
                || before.info.minimized
                || before.info.maximized
            {
                let applied = self.native.set_frame(id, rect, screens, cancelled)?;
                if !same_frame(applied.bounds, rect) {
                    return Err("A member refused the shared window size".into());
                }
            }
        }
        let mut snapshot = self.native.snapshot(id, screens)?;
        if snapshot.info.minimized && !self.hidden.contains(&id) {
            snapshot.info.minimized = false;
            self.native.restore(&snapshot, screens, cancelled)?;
        }
        self.native.tab_set_hidden(id, false)?;
        self.hidden.remove(&id);
        self.groups.activate(id);
        // Keep the old member visible until focus transfers to the revealed
        // member. Hiding a foreground window first makes the OS activate an
        // unrelated window and exposes intermediate desktop/app frames.
        Ok(())
    }

    fn activate_tab(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        self.activate(id, screens, cancelled)?;
        if let Err(error) = self.native.select(id) {
            // The chosen member is already visible. Focus refusal must not undo
            // membership, replay geometry, or expose the previous member again.
            crate::report_warning!("window-tabs", "{error}");
        }
        // Focus refusal still commits visibility; it must not roll back tabs.
        self.publish(screens)
    }

    fn apply_tab(
        &mut self,
        operation: TabOperation,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        match operation {
            TabOperation::EndGroup => self.groups.state.target = None,
            TabOperation::Activate(id) => self.activate_tab(id, screens, cancelled)?,
            TabOperation::Choose(target) => {
                let incoming = self.groups.members(target)?;
                for id in &incoming {
                    self.native.tab_eligible(*id, screens)?;
                }
                for id in &incoming {
                    self.groups.number(*id);
                }
                let anchor = self
                    .groups
                    .state
                    .target
                    .map(|target| self.groups.members(target))
                    .transpose()?
                    .and_then(|ids| ids.first().copied())
                    .map(|id| self.groups.state.containing(id).map_or(id, |g| g.active));
                // The first chosen window may still be ungrouped, so the
                // group's existing membership does not yet include the anchor.
                let mut affected = incoming.clone();
                affected.extend(anchor);
                let before = self.checkpoint(&affected, screens);
                let mut candidate = self.groups.clone();
                candidate.choose(target)?;
                let changed = !candidate.same_membership(&self.groups);
                self.groups = candidate;
                let outcome = (|| {
                    if changed {
                        self.align(anchor.ok_or("Missing group anchor")?, screens, cancelled)?;
                        self.publish(screens)?;
                    }
                    if let Some(active) = self.groups.state.active {
                        self.activate_tab(active, screens, cancelled)?;
                    }
                    self.publish(screens)
                })();
                if let Err(error) = outcome {
                    let recovery = if changed {
                        self.rollback(&before, screens)
                    } else {
                        Ok(())
                    };
                    self.groups.restore(before.groups);
                    if let Err(recovery) = recovery {
                        return Err(format!("{error}; recovery: {recovery}"));
                    }
                    return Err(error);
                }
                if changed {
                    self.remember(before);
                }
            }
            TabOperation::Dissolve => {
                let before = self.checkpoint(&[], screens);
                self.groups.dissolve()?;
                // Dissolution restores visibility independently of foreground permission.
                if let Err(error) = self.publish(screens) {
                    // Keep recovery active until all hidden windows are restored.
                    self.groups.restore(before.groups);
                    return Err(error);
                }
                self.remember(before);
            }
            TabOperation::RemoveActive | TabOperation::Reorder { .. } => {
                let before = self.checkpoint(&[], screens);
                match operation {
                    TabOperation::RemoveActive => self.groups.remove_active()?,
                    TabOperation::Reorder { backwards } => self.groups.cycle(backwards, true)?,
                    _ => unreachable!(),
                }
                if let Some(id) = self.groups.state.active
                    && let Err(error) = self.activate_tab(id, screens, cancelled)
                {
                    self.groups.restore(before.groups);
                    return Err(error);
                }
                self.remember(before);
            }
            TabOperation::Cycle { backwards } => {
                let before = self.groups.clone();
                self.groups.cycle(backwards, false)?;
                if let Some(id) = self.groups.state.active
                    && let Err(error) = self.activate_tab(id, screens, cancelled)
                {
                    self.groups = before;
                    return Err(error);
                }
            }
            TabOperation::Enter { screen } => {
                let preferred = self.groups.state.active.filter(|id| {
                    self.snapshot(*id, screens).is_ok_and(|snapshot| {
                        self.scope
                            .is_none_or(|scope| scope.contains(&snapshot.info))
                    })
                });
                self.groups.state.target = None;
                let windows = self.enumerate(screens, cancelled)?;
                let mut apps: BTreeMap<String, Vec<WindowId>> = BTreeMap::new();
                for window in crate::api::window::application_number_order(
                    &windows,
                    self.groups.state.numbers.iter().copied(),
                ) {
                    self.groups.number(window.id);
                    if (self.scope.is_none() && window.screen != screen)
                        || self.groups.state.containing(window.id).is_some()
                        || self.native.tab_eligible(window.id, screens).is_err()
                    {
                        continue;
                    }
                    let app = self.native.tab_application(window.id, screens)?;
                    if !app.is_empty() {
                        // Keep automatic groups on their original display.
                        apps.entry(format!("{}:{app}", window.screen))
                            .or_default()
                            .push(window.id);
                    }
                }
                let ids: Vec<_> = apps
                    .values()
                    .filter(|ids| ids.len() >= 2)
                    .flatten()
                    .copied()
                    .collect();
                let before = self.checkpoint(&ids, screens);
                let outcome = (|| {
                    for members in apps.values().filter(|ids| ids.len() >= 2) {
                        self.groups.state.target = None;
                        for id in members {
                            self.groups.choose(WindowTarget::Window(*id))?;
                        }
                        self.align(members[0], screens, cancelled)?;
                    }
                    self.groups.state.target = None;
                    if let Some(id) = preferred.or(self.groups.state.active) {
                        self.activate_tab(id, screens, cancelled)?;
                    }
                    self.publish(screens)
                })();
                if let Err(error) = outcome {
                    let recovery = self.rollback(&before, screens);
                    self.groups.restore(before.groups);
                    self.groups.state.target = None;
                    return Err(recovery
                        .err()
                        .map_or(error.clone(), |r| format!("{error}; recovery: {r}")));
                }
                self.remember(before);
            }
            TabOperation::Undo | TabOperation::Redo => {
                let redo = matches!(operation, TabOperation::Redo);
                let desired = if redo {
                    self.redo.back()
                } else {
                    self.history.back()
                }
                .cloned();
                let Some(desired) = desired else {
                    return Err(if redo {
                        "Nothing to redo"
                    } else {
                        "Nothing to undo"
                    }
                    .into());
                };
                let ids: Vec<_> = desired.windows.iter().map(|s| s.info.id).collect();
                let inverse = self.checkpoint(&ids, screens);
                self.release_visibility(None)?;
                for snapshot in &desired.windows {
                    if cancelled() {
                        self.rollback(&inverse, screens)?;
                        return Err("Window grouping cancelled".into());
                    }
                    if let Err(error) = self.native.restore(snapshot, screens, cancelled) {
                        self.rollback(&inverse, screens)?;
                        return Err(error);
                    }
                }
                self.groups.restore(desired.groups);
                self.groups.state.target = None;
                for id in self
                    .groups
                    .state
                    .groups
                    .iter()
                    .map(|g| g.active)
                    .collect::<Vec<_>>()
                {
                    if let Err(error) = self.fit_header(id, screens, cancelled) {
                        let recovery = self.rollback(&inverse, screens);
                        self.groups.restore(inverse.groups);
                        return Err(recovery.err().map_or(error.clone(), |recovery| {
                            format!("{error}; recovery: {recovery}")
                        }));
                    }
                }
                if let Some(id) = self.groups.state.active
                    && let Err(error) = self.activate_tab(id, screens, cancelled)
                {
                    self.rollback(&inverse, screens)?;
                    self.groups.restore(inverse.groups);
                    return Err(error);
                }
                if redo {
                    self.redo.pop_back();
                    Self::push(&mut self.history, inverse);
                } else {
                    self.history.pop_back();
                    Self::push(&mut self.redo, inverse);
                }
            }
            TabOperation::Restore {
                members,
                region,
                screen,
                active,
            } => {
                crate::api::window_presets::TabTemplate { region, active }
                    .validate(members.len())?;
                if members.len() < 2
                    || active >= members.len()
                    || members.iter().collect::<BTreeSet<_>>().len() != members.len()
                {
                    return Err("Select distinct windows for every template tab".into());
                }
                for id in &members {
                    self.native.tab_eligible(*id, screens)?;
                }
                let display = screens.get(screen).ok_or("Display is unavailable")?;
                let rect = Rect::new(
                    display.work_area.x + region.x * display.work_area.width,
                    display.work_area.y + region.y * display.work_area.height,
                    region.width * display.work_area.width,
                    region.height * display.work_area.height,
                );
                let rect = Self::content_frame(rect, self.native.tab_bar_height(display))?;
                let before = self.checkpoint(&members, screens);
                let outcome = (|| {
                    self.release_visibility(Some(&members))?;
                    for id in &members {
                        self.groups.detach(*id);
                    }
                    self.groups.state.target = None;
                    for id in &members {
                        self.groups.choose(WindowTarget::Window(*id))?;
                    }
                    self.groups.activate(members[active]);
                    self.native.tab_set_hidden(members[active], true)?;
                    self.hidden.insert(members[active]);
                    self.native
                        .set_frame(members[active], rect, screens, cancelled)?;
                    self.align(members[active], screens, cancelled)?;
                    self.publish(screens)?;
                    self.activate_tab(members[active], screens, cancelled)?;
                    self.groups.state.target = None;
                    self.publish(screens)
                })();
                if let Err(error) = outcome {
                    let recovery = self.rollback(&before, screens);
                    self.groups.restore(before.groups);
                    return Err(recovery
                        .err()
                        .map_or(error.clone(), |r| format!("{error}; recovery: {r}")));
                }
                self.remember(before);
            }
        }
        self.cache_observed(screens);
        self.publish(screens)
    }

    fn drop_tabs(
        &mut self,
        drop: TabDrop,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let incoming = self.groups.members(drop.source)?;
        let anchor = self
            .groups
            .state
            .group(drop.target)
            .ok_or("Drop target expired")?
            .active;
        for id in &incoming {
            self.native.tab_eligible(*id, screens)?;
        }
        let before = self.checkpoint(&incoming, screens);
        let mut candidate = self.groups.clone();
        candidate.drop_tabs(drop)?;
        if candidate.same_membership(&self.groups) {
            return Ok(());
        }
        let active = candidate
            .state
            .group(drop.target)
            .ok_or("Drop target expired")?
            .active;
        self.groups = candidate;
        let result = (|| {
            // Moving the old active tab may leave a hidden adjacent member (or
            // one ungrouped survivor). Restore it at its own group's last frame.
            for old in &before.groups.state.groups {
                if old.id != drop.target && incoming.contains(&old.active) {
                    let next = self
                        .groups
                        .state
                        .group(old.id)
                        .map(|g| g.active)
                        .or_else(|| {
                            old.members
                                .iter()
                                .copied()
                                .find(|id| !incoming.contains(id))
                        });
                    if let Some(next) = next {
                        self.activate(next, screens, cancelled)?;
                    }
                }
            }
            if before
                .groups
                .state
                .containing(active)
                .is_some_and(|g| g.id == drop.target)
            {
                self.activate_tab(active, screens, cancelled)?;
            } else {
                self.align(anchor, screens, cancelled)?;
                self.native.tab_set_hidden(active, false)?;
                self.hidden.remove(&active);
                self.groups.activate(active);
                if let Err(error) = self.native.select(active) {
                    crate::report_warning!("window-tabs", "{error}");
                }
                self.publish(screens)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            let recovery = self.rollback(&before, screens);
            self.groups.restore(before.groups);
            let visibility = self.publish(screens);
            return Err([Some(error), recovery.err(), visibility.err()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("; "));
        }
        self.remember(before);
        self.cache_observed(screens);
        self.publish(screens)
    }

    /// Native callbacks only enqueue/coalesce identities. All snapshots and
    /// writes happen here, serialized with keyboard-driven operations.
    pub fn pump(&mut self, screens: &[Screen], cancelled: &dyn Fn() -> bool) -> Result<(), String> {
        if cancelled() {
            return Ok(());
        }
        let events = self.native.tab_events();
        if events.is_empty() {
            // A failed unhide after closure/dissolution must remain retryable
            // even when no live group is left to produce native notifications.
            if self.hidden.iter().any(|id| {
                self.groups
                    .state
                    .containing(*id)
                    .is_none_or(|g| g.active == *id)
            }) {
                return self.publish(screens);
            }
            return Ok(());
        }
        let mut changed = BTreeSet::new();
        let mut focus = None;
        for event in events {
            if cancelled() {
                return Ok(());
            }
            match event {
                TabNativeEvent::Changed(id) => {
                    changed.insert(id);
                }
                TabNativeEvent::Focused(id) => {
                    if self.native.tab_selected(id) {
                        focus = Some(id);
                    }
                }
                TabNativeEvent::Activate(id) => {
                    // A click supersedes foreground notifications queued before
                    // it; otherwise that stale focus could select the old tab.
                    focus = None;
                    self.activate_tab(id, screens, cancelled)?;
                }
                TabNativeEvent::Dissolve(group) => {
                    if self.groups.state.group(group).is_some() {
                        self.groups.state.target = Some(WindowTarget::Group(group));
                        self.apply_tab(TabOperation::Dissolve, screens, cancelled)?;
                    }
                }
                TabNativeEvent::Drop(drop) => {
                    focus = None;
                    self.drop_tabs(drop, screens, cancelled)?;
                }
                TabNativeEvent::Closed(id) => self.close_member(id, screens, cancelled)?,
                TabNativeEvent::VisibilityChanged => {}
            }
        }
        for id in changed {
            if cancelled() {
                return Ok(());
            }
            if self.groups.state.containing(id).is_none() {
                continue;
            }
            let Ok(now) = self.native.snapshot(id, screens) else {
                continue;
            };
            if now.info.fullscreen {
                self.groups.detach(id);
                continue;
            }
            if self
                .groups
                .state
                .containing(id)
                .is_some_and(|g| g.active == id)
            {
                if now.info.minimized {
                    self.minimize_group(id, screens, cancelled)?;
                    // Minimize notifications can be accompanied by old focus
                    // notifications. Do not let them reopen a different member.
                    if focus.is_some_and(|focused| {
                        self.groups
                            .state
                            .containing(id)
                            .is_some_and(|g| g.members.contains(&focused))
                    }) {
                        focus = None;
                    }
                }
                self.fit_header(id, screens, cancelled)?;
            }
            self.observed.insert(id, now);
            // Physical geometry changes never select or move another member.
        }
        if let Some(id) = focus
            && self.groups.state.containing(id).is_some()
            && self.native.tab_selected(id)
            && self
                .groups
                .state
                .containing(id)
                .is_some_and(|g| g.active != id)
        {
            self.activate_tab(id, screens, cancelled)?;
        }
        self.publish(screens)
    }
    fn close_member(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let minimized = self
            .observed
            .get(&id)
            .is_some_and(|snapshot| snapshot.info.minimized);
        let adjacent = self
            .groups
            .state
            .containing(id)
            .filter(|g| g.active == id)
            .and_then(|g| {
                let index = g.members.iter().position(|member| *member == id)?;
                g.members
                    .get(index + 1)
                    .or_else(|| index.checked_sub(1).and_then(|i| g.members.get(i)))
                    .copied()
            });
        self.forget(id);
        if !minimized
            && let Some(next) = adjacent
            && let Err(error) = self.activate_tab(next, screens, cancelled)
        {
            // A refused resize cannot strand the surviving application.
            // Reveal it at its accepted geometry, then report the error.
            self.publish(screens)?;
            return Err(error);
        }
        Ok(())
    }
    fn forget(&mut self, id: WindowId) {
        self.groups.forget(id);
        self.observed.remove(&id);
        self.hidden.remove(&id);
        for checkpoint in self.history.iter_mut().chain(self.redo.iter_mut()) {
            checkpoint.groups.forget(id);
            checkpoint.windows.retain(|s| s.info.id != id);
        }
        if !self.retired.contains(&id) {
            self.retired.push(id);
        }
    }
    pub fn shutdown(&mut self) {
        if let Err(error) = self.release_visibility(None) {
            crate::report_error!("window-tabs", "{error}");
        }
        let _ = self.native.tab_bars(&[]);
        let _ = self.native.tab_watch(&[]);
        self.native.reset();
    }
}

impl<A: WindowAccess> WindowAccess for Grouped<A> {
    fn set_scope(&mut self, scope: Option<crate::api::window::WindowScope>, reset: bool) {
        self.native.set_scope(scope, reset);
        if reset || self.scope != scope {
            self.groups.clear_numbers();
            self.numbers_dirty = true;
        }
        self.scope = scope;
    }
    fn wait_for_events(&self, timeout: Option<Duration>) -> bool {
        let recovery = self.hidden.iter().any(|id| {
            self.groups
                .state
                .containing(*id)
                .is_none_or(|g| g.active == *id)
        });
        self.native.wait_for_events(if recovery {
            Some(Duration::from_millis(250))
        } else {
            timeout
        })
    }
    fn activate_window(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let result = self
            .activate(id, screens, cancelled)
            .and_then(|()| self.native.select(id));
        if result.is_ok()
            || self
                .groups
                .state
                .containing(id)
                .is_some_and(|g| g.active == id)
        {
            self.cache_observed(screens);
            self.publish(screens)?;
        }
        result
    }
    fn acquire(&mut self, point: Point, screens: &[Screen]) -> Result<Option<WindowInfo>, String> {
        let result = self
            .native
            .acquire(point, screens)?
            .filter(|window| self.scope.is_none_or(|scope| scope.contains(window)));
        self.groups.state.active = result.as_ref().map(|window| window.id);
        if let Some(window) = &result {
            self.groups.number(window.id);
            self.groups.state.active = Some(window.id);
        }
        result
            .map(|window| {
                self.snapshot(window.id, screens)
                    .map(|snapshot| snapshot.info)
            })
            .transpose()
    }
    fn enumerate(
        &mut self,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<WindowInfo>, String> {
        let windows = self.native.enumerate(screens, cancelled)?;
        // Enumeration may observe destruction before the queued native callback.
        // Run the same survivor-selection path before publishing the inventory.
        for id in self.native.take_closed() {
            self.close_member(id, screens, cancelled)?;
        }
        let windows: Vec<_> = windows
            .into_iter()
            .map(|w| self.snapshot(w.id, screens).map_or(w, |s| s.info))
            .filter(|w| self.scope.is_none_or(|scope| scope.contains(w)))
            .collect();
        // Scope filtering (especially minimizing during size_cycle) is not
        // destruction. Reserve each identity's number until close_member or
        // the next session/scope reset, so restoration cannot renumber it.
        if windows.iter().any(|window| {
            !self
                .groups
                .state
                .numbers
                .iter()
                .any(|(id, _)| *id == window.id)
        }) {
            self.numbers_dirty = true;
            for window in crate::api::window::application_number_order(
                &windows,
                self.groups.state.numbers.iter().copied(),
            ) {
                self.groups.number(window.id);
            }
        }
        if self.numbers_dirty {
            self.publish(screens)?;
            self.numbers_dirty = false;
        }
        Ok(windows)
    }
    fn snapshot(&self, id: WindowId, screens: &[Screen]) -> Result<Snapshot, String> {
        let mut snapshot = self.native.snapshot(id, screens)?;
        if let Some(group) = self.groups.state.containing(id) {
            let active = self.native.snapshot(group.active, screens)?;
            let header = self.header_height(active.info.screen, screens);
            snapshot.info.bounds = Self::outer_frame(active.info.bounds, header);
            snapshot.info.screen = active.info.screen;
            snapshot.info.minimized = active.info.minimized;
            snapshot.info.maximized = active.info.maximized;
            snapshot.restored = Self::outer_frame(active.restored, header);
        }
        Ok(snapshot)
    }
    fn set_frame(
        &mut self,
        id: WindowId,
        rect: Rect,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        let active = self.groups.state.containing(id).map_or(id, |g| g.active);
        let rect = if active != id || self.groups.state.containing(id).is_some() {
            let screen = super::window_geometry::screen_index(screens, rect)
                .ok_or("Display is unavailable")?;
            Self::content_frame(rect, self.header_height(screen, screens))?
        } else {
            rect
        };
        self.native.set_frame(active, rect, screens, cancelled)?;
        let result = self.snapshot(id, screens)?.info;
        self.cache_observed(screens);
        self.publish(screens)?;
        Ok(result)
    }
    fn restore(
        &mut self,
        snapshot: &Snapshot,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        let mut desired = snapshot.clone();
        desired.info.id = self
            .groups
            .state
            .containing(snapshot.info.id)
            .map_or(snapshot.info.id, |g| g.active);
        if self.groups.state.containing(snapshot.info.id).is_some() {
            let header = self.header_height(snapshot.info.screen, screens);
            desired.info.bounds = Self::content_frame(desired.info.bounds, header)?;
            desired.restored = Self::content_frame(desired.restored, header)?;
        }
        self.native.restore(&desired, screens, cancelled)?;
        if self.groups.state.containing(snapshot.info.id).is_some() {
            self.fit_header(desired.info.id, screens, cancelled)?;
        }
        let result = self.snapshot(snapshot.info.id, screens)?.info;
        self.cache_observed(screens);
        self.publish(screens)?;
        Ok(result)
    }
    fn cycle_state(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        let active = self.groups.state.containing(id).map_or(id, |g| g.active);
        self.native.cycle_state(active, screens, cancelled)?;
        if self.groups.state.containing(id).is_some() {
            self.minimize_group(active, screens, cancelled)?;
            self.fit_header(active, screens, cancelled)?;
        }
        let result = self.snapshot(id, screens)?.info;
        self.cache_observed(screens);
        self.publish(screens)?;
        Ok(result)
    }
    fn select(&self, id: WindowId) -> Result<(), String> {
        self.native.select(id)
    }
    fn maintain_audio(&self) -> bool {
        self.native.maintain_audio()
    }
    fn system_audio(&self, change: crate::api::audio::AudioAction) -> Result<String, String> {
        self.native.system_audio(change)
    }
    fn volume(
        &self,
        id: WindowId,
        change: crate::api::audio::AudioAction,
    ) -> Result<String, String> {
        let active = self
            .groups
            .state
            .containing(id)
            .map_or(id, |group| group.active);
        self.native.volume(active, change)
    }
    fn close(&self, id: WindowId) -> Result<(), String> {
        let active = self
            .groups
            .state
            .containing(id)
            .map_or(id, |group| group.active);
        self.native.close(active)
    }
    fn pointer(&self) -> Result<Point, String> {
        self.native.pointer()
    }
    fn minimum_size(&self, id: WindowId) -> Point {
        let mut minimum = self
            .affected(id)
            .iter()
            .map(|id| self.native.minimum_size(*id))
            .fold(Point::new(0.0, 0.0), |a, b| {
                Point::new(a.x.max(b.x), a.y.max(b.y))
            });
        if let Some(group) = self.groups.state.containing(id)
            && let Some(bar) = self.bars.iter().find(|bar| bar.group == group.id)
        {
            minimum.y += self.header_height(bar.screen, &self.screens);
        }
        minimum
    }
    fn logical_scale(&self, screen: &Screen) -> f64 {
        self.native.logical_scale(screen)
    }
    fn move_fullscreen(
        &mut self,
        id: WindowId,
        destination: crate::api::command::WindowScreenTarget,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(WindowInfo, Option<Point>), String> {
        self.activate_tab(id, screens, cancelled)?;
        self.groups.detach(id);
        self.publish(screens)?;
        self.native
            .move_fullscreen(id, destination, screens, cancelled)
    }
    fn tab_operation(
        &mut self,
        operation: TabOperation,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let result = self.apply_tab(operation, screens, cancelled);
        if let Err(error) = result {
            self.cache_observed(screens);
            return Err(self
                .publish(screens)
                .err()
                .map_or(error.clone(), |recovery| {
                    format!("{error}; recovery: {recovery}")
                }));
        }
        Ok(())
    }
    fn tab_state(&self) -> Option<TabState> {
        Some(self.groups.state.clone())
    }
    fn layout_representative(&self, id: WindowId) -> WindowId {
        self.groups.state.representative(id)
    }
    fn take_closed(&mut self) -> Vec<WindowId> {
        std::mem::take(&mut self.retired)
    }
    fn reset(&mut self) {
        // Mode sessions own editing/history, not native group identity leases.
        self.groups.state.target = None;
    }
}

#[cfg(test)]
mod tests;
