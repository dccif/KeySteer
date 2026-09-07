//! Window movement is a native capability; this plugin only interprets verbs.
use crate::api::binding::Binding;
use crate::api::command::{
    Command, CommandBatch, HostContext, Mode, ModeEvent, WindowScreenTarget,
};
use crate::api::input::{KeyChord, ModeId};
use crate::api::plugin::{Manifest, Plugin};
use std::collections::BTreeMap;

const VERB: &str = "move_window";

pub struct WindowMover {
    id: ModeId,
    manifest: Manifest,
}

impl WindowMover {
    pub fn new() -> Result<Self, String> {
        Self::with_aliases(&BTreeMap::new())
    }

    pub(crate) fn with_aliases(aliases: &BTreeMap<String, String>) -> Result<Self, String> {
        let manifest = Manifest::new("com.keysteer.window-mover", "Window Mover")
            .with_description("Move the window under the pointer to another display")
            .with_verb(VERB)
            .with_default_binding(
                KeyChord::parse_with_aliases("primary+d", aliases)?,
                Binding::Invoke {
                    verb: VERB.into(),
                    args: vec!["next".into()],
                },
            );
        Ok(Self {
            id: ModeId::new("plugin:window-mover")?,
            manifest,
        })
    }
}

impl Mode for WindowMover {
    fn id(&self) -> ModeId {
        self.id.clone()
    }

    fn captures_keyboard(&self) -> bool {
        false
    }

    fn wants_pointer_events(&self) -> bool {
        false
    }

    fn handle(&mut self, event: &ModeEvent, _ctx: &HostContext<'_>) -> CommandBatch {
        let ModeEvent::Invoked { verb, args } = event else {
            return CommandBatch::new();
        };
        if verb != VERB || args.len() != 1 {
            return CommandBatch::new();
        }
        let target = match args[0].to_ascii_lowercase().as_str() {
            "previous" | "prev" => WindowScreenTarget::Previous,
            "next" => WindowScreenTarget::Next,
            number => match number.parse::<usize>().ok().and_then(|n| n.checked_sub(1)) {
                Some(index) => WindowScreenTarget::Index(index),
                None => return CommandBatch::new(),
            },
        };
        CommandBatch::one(Command::MoveWindowToScreen(target))
    }
}

impl Plugin for WindowMover {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
}
