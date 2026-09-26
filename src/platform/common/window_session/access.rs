//! Native capability boundary; implementations own handles on their worker.
use super::*;
pub(crate) trait WindowAccess {
    /// Queried on the window worker; never on the engine or hook thread.
    fn focused_bounds(&self, _process: u32) -> Result<Option<Rect>, String> {
        Ok(None)
    }
    /// Native confirmations advanced by the existing worker event loop.
    fn native_deadline(&self) -> Option<Instant> {
        None
    }
    fn poll_native(&self) -> Result<(), String> {
        Ok(())
    }
    /// Bound temporary native objects to a work batch, never to the idle wait.
    fn native_batch<R>(work: impl FnOnce() -> R) -> R {
        work()
    }

    fn set_scope(&mut self, _scope: Option<crate::api::window::WindowScope>, _reset: bool) {}
    /// Optional native message-loop wakeup, created on the worker thread.
    fn event_waker(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        None
    }
    /// Wait for native events or the mailbox wakeup. False uses the portable condvar.
    fn wait_for_events(&self, _timeout: Option<Duration>) -> bool {
        false
    }
    /// Hide a single application window without changing its parent.
    fn tab_set_hidden(&mut self, _id: WindowId, _hidden: bool) -> Result<(), String> {
        Ok(())
    }
    /// Validate queued focus notifications against the current foreground window.
    fn tab_selected(&self, _id: WindowId) -> bool {
        true
    }
    fn activate_window(
        &mut self,
        id: WindowId,
        _screens: &[Screen],
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        self.select(id)
    }
    fn tab_operation(
        &mut self,
        _operation: crate::api::window_tabs::TabOperation,
        _screens: &[Screen],
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        Err("Window tab groups are unavailable on this backend".into())
    }
    /// Update one existing bar without changing group membership. False asks
    /// the adapter to use the full-list compatibility path.
    fn tab_bar_update(&mut self, _bar: &crate::api::window_tabs::TabBar) -> Result<bool, String> {
        Ok(false)
    }
    fn tab_geometry(
        &self,
        id: WindowId,
        _previous: &Snapshot,
        screens: &[Screen],
    ) -> Result<Snapshot, String> {
        self.snapshot(id, screens)
    }
    fn tab_interacting(&self, _id: WindowId) -> bool {
        false
    }
    fn tab_state(&self) -> Option<crate::api::window_tabs::TabState> {
        None
    }
    fn tab_eligible(&self, id: WindowId, screens: &[Screen]) -> Result<(), String> {
        let window = self.snapshot(id, screens)?.info;
        if !window.resizable || window.fullscreen {
            return Err("Only ordinary resizable windows can join a tab group".into());
        }
        Ok(())
    }
    fn tab_events(&mut self) -> Vec<crate::api::window_tabs::TabNativeEvent> {
        Vec::new()
    }
    fn tab_watch(&mut self, _ids: &[WindowId]) -> Result<(), String> {
        Ok(())
    }
    fn tab_bars(&mut self, _bars: &[crate::api::window_tabs::TabBar]) -> Result<(), String> {
        Ok(())
    }
    fn tab_visible(&self, _id: WindowId) -> bool {
        true
    }
    /// Occupied header height in this backend's screen coordinate system.
    /// Headless adapters have no native strip.
    fn tab_bar_height(&self, _screen: &Screen) -> f64 {
        0.0
    }
    /// Minimize a member without restoring or moving an inactive window first.
    fn tab_minimize(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let mut snapshot = self.snapshot(id, screens)?;
        if !snapshot.info.minimized {
            snapshot.info.minimized = true;
            self.restore(&snapshot, screens, cancelled)?;
        }
        Ok(())
    }
    /// Reserve strip space after an external move/maximize, preserving state
    /// where the platform supports resizing a maximized window in place.
    fn tab_fit_frame(
        &mut self,
        id: WindowId,
        bounds: Rect,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        self.set_frame(id, bounds, screens, cancelled)
    }
    fn layout_representative(&self, id: WindowId) -> WindowId {
        id
    }
    fn acquire(&mut self, point: Point, screens: &[Screen]) -> Result<Option<WindowInfo>, String>;
    fn enumerate(
        &mut self,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<WindowInfo>, String>;
    fn snapshot(&self, id: WindowId, screens: &[Screen]) -> Result<Snapshot, String>;
    /// Submit ordinary geometry without waiting for application acknowledgement.
    /// False selects the transactional/unsupported synchronous adapter path.
    fn submit_frame(
        &mut self,
        _before: &Snapshot,
        _rect: Rect,
        _screens: &[Screen],
    ) -> Result<bool, String> {
        Ok(false)
    }
    /// Read placement in place; native implementations leave metadata storage untouched.
    fn refresh_geometry(&self, snapshot: &mut Snapshot, screens: &[Screen]) -> Result<(), String> {
        *snapshot = self.tab_geometry(snapshot.info.id, snapshot, screens)?;
        Ok(())
    }
    /// Ordinary in-flight geometry may omit state queries until final validation.
    fn refresh_frame(&self, snapshot: &mut Snapshot, screens: &[Screen]) -> Result<(), String> {
        self.refresh_geometry(snapshot, screens)
    }
    /// Validate state after a geometry-only poll. Backends that already read it do nothing.
    fn validate_frame(&self, _snapshot: &mut Snapshot, _screens: &[Screen]) -> Result<(), String> {
        Ok(())
    }
    /// Reject known transactional targets before doing any native preparation.
    fn can_submit_frame(&self, _id: WindowId) -> bool {
        false
    }
    fn confirmed_frame(&mut self, _id: WindowId) {}
    /// Publish an acknowledged placement, including group header geometry.
    fn confirmed_placement(
        &mut self,
        snapshot: &Snapshot,
        _screens: &[Screen],
    ) -> Result<(), String> {
        if !snapshot.info.maximized {
            self.confirmed_frame(snapshot.info.id);
        }
        Ok(())
    }
    /// Submit maximize/restore without waiting. Unsupported adapters decline
    /// before writing; state transitions are confirmed by the shared worker.
    fn submit_maximize(
        &mut self,
        _before: &Snapshot,
        _maximize: bool,
        _screens: &[Screen],
    ) -> Result<bool, String> {
        Ok(false)
    }
    /// Virtual-maximize backends may acknowledge a settled application constraint.
    /// Native maximize flags must never be fabricated by the shared state machine.
    fn accept_maximized_frame(&mut self, _observed: &mut Snapshot, _screens: &[Screen]) {}
    fn submit_maximized_frame(
        &mut self,
        _before: &Snapshot,
        _rect: Rect,
        _screens: &[Screen],
    ) -> Result<bool, String> {
        Ok(false)
    }
    fn set_frame(
        &mut self,
        id: WindowId,
        rect: Rect,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String>;
    fn restore(
        &mut self,
        snapshot: &Snapshot,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String>;
    fn cycle_state(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String>;
    fn toggle_state(
        &mut self,
        id: WindowId,
        minimize: bool,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        let before = self.snapshot(id, screens)?;
        if cancelled() {
            return Ok(before.info);
        }
        if before.info.minimized || (!minimize && before.info.maximized) {
            self.set_frame(id, before.restored, screens, cancelled)
        } else if minimize {
            self.tab_minimize(id, screens, cancelled)?;
            self.snapshot(id, screens).map(|s| s.info)
        } else {
            self.cycle_state(id, screens, cancelled)
        }
    }
    /// Submit selection. Asynchronous adapters retain confirmation state until
    /// `native_deadline` clears; grouped visibility is committed after that.
    fn select(&self, id: WindowId) -> Result<(), String>;
    /// Resolve native foreground identity after refreshing the inventory.
    fn focused_window(&self, _windows: &[WindowInfo]) -> Option<WindowId> {
        None
    }
    /// Hit-test without activating, assigning numbers or changing tab selection.
    fn pointer_window(&mut self, screens: &[Screen]) -> Result<Option<WindowId>, String> {
        let point = self.pointer()?;
        Ok(self.acquire(point, screens)?.map(|window| window.id))
    }
    /// Reap native audio routes; true requests a bounded maintenance wakeup.
    fn maintain_audio(&self) -> bool {
        false
    }
    fn audio_factory(&self) -> Option<crate::platform::common::audio_worker::AudioFactory> {
        None
    }
    fn audio_process(
        &self,
        target: crate::api::audio::AudioTarget,
    ) -> Result<Option<crate::platform::common::audio_worker::AudioProcess>, String> {
        match target {
            crate::api::audio::AudioTarget::System => Ok(None),
            _ => Err("Application audio identity is unavailable".into()),
        }
    }
    fn system_audio(&self, _change: crate::api::audio::AudioAction) -> Result<String, String> {
        Err("System audio control is unavailable on this backend".into())
    }
    fn volume(
        &self,
        _id: WindowId,
        _change: crate::api::audio::AudioAction,
    ) -> Result<String, String> {
        Err("Application volume control is unavailable on this backend".into())
    }
    fn close(&self, _id: WindowId) -> Result<(), String> {
        Err("Closing windows is unavailable on this backend".into())
    }
    fn pointer(&self) -> Result<Point, String>;
    fn minimum_size(&self, _id: WindowId) -> Point {
        Point::new(100.0, 80.0)
    }
    fn logical_scale(&self, _screen: &Screen) -> f64 {
        1.0
    }
    fn move_fullscreen(
        &mut self,
        _id: WindowId,
        _destination: crate::api::command::WindowScreenTarget,
        _screens: &[Screen],
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<(WindowInfo, Option<Point>), String> {
        Err("moving native fullscreen windows is not supported".into())
    }
    fn reset(&mut self);
    fn take_closed(&mut self) -> Vec<WindowId> {
        Vec::new()
    }
}
