//! Independently routed modes sharing one platform-free window session.
use super::*;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Move,
    Quick,
    Editor,
    Restore,
    Tab,
}

impl WindowKind {
    pub fn id(self) -> ModeId {
        match self {
            Self::Move => ModeId::window(),
            Self::Quick => ModeId::window_quick(),
            Self::Editor => ModeId::window_editor(),
            Self::Restore => ModeId::window_restore(),
            Self::Tab => ModeId::window_tab(),
        }
    }
    pub fn is_library(self) -> bool {
        matches!(self, Self::Restore)
    }
}

pub struct WindowMode {
    kind: WindowKind,
    settings: Settings,
    session: Arc<Mutex<WindowSession>>,
}
impl WindowMode {
    pub fn family(modes: impl IntoIterator<Item = (WindowKind, Settings)>) -> Vec<Self> {
        let mut session = None;
        modes
            .into_iter()
            .map(|(kind, settings)| {
                let shared = session.get_or_insert_with(|| {
                    Arc::new(Mutex::new(WindowSession::new(settings.clone())))
                });
                Self {
                    kind,
                    settings,
                    session: Arc::clone(shared),
                }
            })
            .collect()
    }
}
impl Mode for WindowMode {
    fn id(&self) -> ModeId {
        self.kind.id()
    }
    fn display_name(&self) -> String {
        self.kind.id().to_string()
    }
    fn session_group(&self) -> Option<&'static str> {
        Some("window")
    }
    fn wants_pointer_events(&self) -> bool {
        false
    }
    fn prepare_transition(
        &mut self,
        target: &ModeId,
        ctx: &HostContext<'_>,
    ) -> Option<CommandBatch> {
        let mut session = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        session.preserve_session = target.is_window();
        if session.kind == WindowKind::Tab
            && (session.tabs.in_flight.is_some() || !session.tabs.queue.is_empty())
        {
            session.pending_transition = Some(target.clone());
            session.pending_handoff = true;
            return Some(CommandBatch::new());
        }
        let keep_edit = target == &ModeId::window_restore()
            || (target == &ModeId::window_editor()
                && session
                    .edit
                    .as_ref()
                    .is_some_and(|e| matches!(e.model, EditModel::Tree(_))));
        if keep_edit
            && session
                .edit
                .as_ref()
                .is_some_and(|e| !e.ready || e.dirty || e.in_flight.is_some() || e.ending)
        {
            let mut out = CommandBatch::new();
            session.stop_movement(&mut out);
            session.temporary = false;
            session.pending_transition = Some(target.clone());
            session.pending_handoff = true;
            session.flush_edit(&mut out);
            return Some(out);
        }
        session.pending_handoff = false;
        if session.edit.is_some() && !keep_edit {
            let mut out = CommandBatch::new();
            session.stop_movement(&mut out);
            session.temporary = false;
            session.enter_pending = false;
            session.restore_pending = false;
            session.pending_transition = Some(target.clone());
            session.finish_edit(Finish::Transition, &mut out);
            out.push(ctx.present(session.view()));
            Some(out)
        } else {
            None
        }
    }
    fn handle_owned(&mut self, event: ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        let mut session = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut scope_changed = false;
        if matches!(
            event,
            ModeEvent::Activated { .. } | ModeEvent::Pushed { .. } | ModeEvent::Restarted
        ) {
            scope_changed = session.settings.all_screens != self.settings.all_screens
                || session.settings.include_minimized != self.settings.include_minimized;
            session.kind = self.kind;
            session.settings = self.settings.clone();
            if scope_changed {
                session.inventory_dirty = true;
                session.numbers.clear();
                session.next_number = 1;
                session.rebuild_numbers();
            }
        } else if session.kind != self.kind {
            return CommandBatch::new();
        }
        let mut out = CommandBatch::new();
        if scope_changed && session.session != 0 {
            session.refresh_pending = None;
            session.refresh(&mut out);
        }
        out.extend(session.handle_owned(event, ctx));
        if session.pending_handoff
            && session.tabs.in_flight.is_none()
            && session.tabs.queue.is_empty()
            && session
                .edit
                .as_ref()
                .is_none_or(|e| e.ready && !e.dirty && e.in_flight.is_none() && !e.ending)
        {
            session.pending_handoff = false;
            if let Some(target) = session.pending_transition.take() {
                out.push(Command::SwitchMode(target));
            }
        }
        out
    }
    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        self.handle_owned(event.clone(), ctx)
    }
    fn claims_key(&self, key: &Key) -> bool {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .claims_key(key)
    }
    fn available_keys(&self) -> Vec<(String, String)> {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .available_keys()
    }
    fn window_action_supported(&self, action: &W) -> bool {
        self.kind.supports_action(action)
    }
    fn window_action_available(&self, action: &W) -> bool {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .window_action_available(action)
    }
    fn indicator_detail(&self) -> Option<String> {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .indicator_detail()
    }
    fn help_anchor(&self) -> Option<Rect> {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .help_anchor()
    }
    fn help_previews(&self) -> Vec<(String, String, Rect, bool)> {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .help_previews()
    }
}

impl WindowSession {
    pub(super) fn enter_kind(&mut self, ctx: &HostContext<'_>, out: &mut CommandBatch) {
        self.library_open = self.kind.is_library();
        self.cancel_number(out);
        self.swap_source = None;
        if self.kind.is_library() {
            self.open_library(out);
        }
        if self.target.is_none() && self.request > self.result {
            self.enter_pending = true;
            return;
        }
        self.enter_pending = false;
        match self.kind {
            WindowKind::Tab => self.enter_tabs(out),
            WindowKind::Quick if self.edit.is_none() => self.start_edit(false, ctx, out),
            WindowKind::Editor if self.edit.is_none() => self.start_edit(true, ctx, out),
            _ => {}
        }
    }
}
