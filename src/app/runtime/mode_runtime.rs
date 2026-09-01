//! Runtime responsibility extracted from the Engine composition root.

use super::*;

impl Engine {
    pub(super) fn dispatch(
        &mut self,
        event: ModeEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let owner = self.registry.active.clone();
        self.dispatch_to(&owner, event, backend)
    }

    pub(super) fn mode_index_for_dispatch(&mut self, owner: &ModeId) -> Option<usize> {
        if owner == &self.registry.active {
            if let Some(index) = self.registry.active_slot
                && self.registry.id_at(index) == Some(owner)
            {
                return Some(index);
            }
            let index = self.registry.index_of(owner)?;
            self.registry.active_slot = Some(index);
            return Some(index);
        }
        self.registry.index_of(owner)
    }

    /// Deliver an event to a specific registered mode. This is used for normal
    /// pointer controls borrowed by an active label mode.
    pub(super) fn dispatch_to(
        &mut self,
        owner: &ModeId,
        event: ModeEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let Some(mode_index) = self.mode_index_for_dispatch(owner) else {
            return Ok(());
        };
        let context = HostContext {
            screens: &self.screens,
            cursor: self.cursor,
            focused_app: self.focused_app.as_ref(),
            palette: &self.palette,
        };
        let Some(mode) = self.registry.get_index_mut(mode_index) else {
            return Ok(());
        };
        let commands = mode.handle(&event, &context);
        crate::support::perf_probe::mark("mode_handled");
        crate::support::perf_probe::mark("commands_ready");
        self.execute_for(owner, commands, backend)
    }

    /// Deliver the one event family whose platform-owned buffers are worth
    /// consuming. Keeping this separate leaves frame, pointer and key dispatch
    /// on the original single-vtable-call hot path.
    pub(super) fn dispatch_owned_to(
        &mut self,
        owner: &ModeId,
        event: ModeEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let Some(mode_index) = self.mode_index_for_dispatch(owner) else {
            return Ok(());
        };
        let context = HostContext {
            screens: &self.screens,
            cursor: self.cursor,
            focused_app: self.focused_app.as_ref(),
            palette: &self.palette,
        };
        let Some(mode) = self.registry.get_index_mut(mode_index) else {
            return Ok(());
        };
        let commands = mode.handle_owned(event, &context);
        crate::support::perf_probe::mark("mode_handled");
        crate::support::perf_probe::mark("commands_ready");
        self.execute_for(owner, commands, backend)
    }

    pub(super) fn push_mode(
        &mut self,
        target: ModeId,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if target == self.registry.active || !self.registry.contains_key(&target) {
            return Ok(());
        }
        if self.registry.active == ModeId::normal()
            && target != ModeId::normal()
            && !Self::releases_toggle_session_on_entry(&target)
        {
            let _ = self.release_drag_auto_release(backend)?;
        }
        if Self::releases_toggle_session_on_entry(&target) {
            self.release_toggle_session_for_safe_mode(backend)?;
        }
        let previous = self.registry.active.clone();
        self.dispatch(ModeEvent::Suspended, backend)?;
        self.registry.modal_stack.push(previous.clone());
        self.set_active(target);
        self.dispatch(ModeEvent::Pushed { previous }, backend)
    }

    pub(super) fn pop_mode(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let Some(previous) = self.registry.modal_stack.last().cloned() else {
            return Ok(());
        };
        if self.registry.active == ModeId::normal()
            && previous != ModeId::normal()
            && !Self::releases_toggle_session_on_entry(&previous)
        {
            let _ = self.release_drag_auto_release(backend)?;
        }
        if Self::releases_toggle_session_on_entry(&previous) {
            self.release_toggle_session_for_safe_mode(backend)?;
        }
        self.registry.modal_stack.pop();
        let current = self.registry.active.clone();
        self.dispatch(ModeEvent::Deactivated, backend)?;
        self.cancel_scans_for_owner(&current, backend)?;
        self.scheduler.cancel_timers_for_owner(&current);
        self.set_active(previous);
        self.dispatch(ModeEvent::Resumed, backend)
    }

    pub(super) fn restart_active(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let active = self.registry.active.clone();
        self.scheduler.cancel_owner(&active);
        self.dispatch(ModeEvent::Restarted, backend)
    }
}
