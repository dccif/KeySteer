//! Native text entry with ordinary configurable bindings and temporary layers.

use crate::api::{Command, CommandBatch, HostContext, Mode, ModeEvent, ModeId};

#[derive(Default)]
pub struct TextInputMode;

impl TextInputMode {
    pub fn new() -> Self {
        Self
    }
}

impl Mode for TextInputMode {
    fn id(&self) -> ModeId {
        ModeId::text_input()
    }
    fn display_name(&self) -> String {
        "Text Input".into()
    }
    fn captures_keyboard(&self) -> bool {
        false
    }
    fn wants_pointer_events(&self) -> bool {
        false
    }
    fn handle(&mut self, event: &ModeEvent, _ctx: &HostContext<'_>) -> CommandBatch {
        match event {
            ModeEvent::Activated { .. } => Command::HideOverlay.into(),
            _ => CommandBatch::new(),
        }
    }
}
