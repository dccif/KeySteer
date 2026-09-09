//! Window interaction state. Native objects and checkpoints belong to the worker.
mod editing;
mod interaction;
mod inventory;
mod numbering;
mod presets;
#[cfg(test)]
mod tests;
mod view;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::api::style::LabelUi;
use crate::api::window::{
    WindowAction as W, WindowChange, WindowEditResult, WindowId, WindowInfo, WindowOperation,
    WindowRequest,
};
use crate::api::window_layout::{LayoutTree, QuickPlacement};
use crate::api::{
    Binding, Command, CommandBatch, HostContext, Key, KeyState, Mode, ModeEvent, ModeId, Point,
    Rect,
};
use numbering::{NumberIndex, NumberInput};

const NUMBER_TIMER: &str = "window_number";
const INVENTORY_TIMER: &str = "window_inventory";

#[derive(Clone, Debug)]
pub struct Settings {
    pub exit_mode: crate::api::lifecycle::LifecycleAction,
    pub number_timeout_ms: u64,
    pub move_step: f64,
    pub move_speed: f64,
    pub resize_step: f64,
    pub resize_speed: f64,
    pub gap: f64,
    pub border_width: f64,
    pub ui: LabelUi,
}

#[derive(Clone, Debug, PartialEq)]
enum EditModel {
    Quick(QuickPlacement),
    Tree(LayoutTree),
}

#[derive(Clone, Copy, Debug)]
enum Finish {
    Commit,
    Cancel,
    QuickReset,
    TreeReset,
    Tree,
    Select(WindowId),
    Cycle,
    Tile,
    Exit,
}

struct LiveEdit {
    transaction: u64,
    screen: usize,
    model: EditModel,
    accepted: EditModel,
    history: Vec<EditModel>,
    divider_gesture: bool,
    entry_layout: bool,
    minimums: BTreeMap<WindowId, Point>,
    gap_scale: f64,
    ready: bool,
    revision: u64,
    in_flight: Option<(u64, EditModel)>,
    dirty: bool,
    finishing: Option<Finish>,
    ending: bool,
    deferred: Vec<DeferredEdit>,
}

enum DeferredEdit {
    Direction(crate::api::Direction, bool, bool),
    Number(bool, u32),
}

pub struct WindowMode {
    settings: Settings,
    saved_layouts: Vec<crate::api::window_presets::SavedLayout>,
    library_open: bool,
    save_pending: bool,
    library_page: usize,
    library_index: NumberIndex,
    pending_template: Option<crate::api::window_presets::SavedLayout>,
    session: u64,
    request: u64,
    result: u64,
    group: u64,
    target: Option<WindowInfo>,
    screen: usize,
    inventory: BTreeMap<WindowId, WindowInfo>,
    visible: Vec<WindowId>,
    inventory_dirty: bool,
    numbered_screen: usize,
    numbered_slots: u32,
    numbers: BTreeMap<WindowId, u32>,
    next_number: u32,
    window_index: NumberIndex,
    slot_index: NumberIndex,
    number: NumberInput,
    swap_source: Option<WindowId>,
    refresh_pending: Option<u64>,
    edit: Option<LiveEdit>,
    trees: BTreeMap<usize, LayoutTree>,
    resume_quick: bool,
    size: bool,
    temporary: bool,
    held: BTreeMap<Key, W>,
    status: Option<String>,
    modal: bool,
    previous: ModeId,
}

impl WindowMode {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            previous: ModeId::normal(),
            saved_layouts: Vec::new(),
            library_open: false,
            save_pending: false,
            library_page: 0,
            library_index: NumberIndex::default(),
            pending_template: None,
            session: 0,
            request: 0,
            result: 0,
            group: 0,
            target: None,
            screen: 0,
            inventory: BTreeMap::new(),
            visible: Vec::new(),
            inventory_dirty: true,
            numbered_screen: 0,
            numbered_slots: 0,
            numbers: BTreeMap::new(),
            next_number: 1,
            window_index: NumberIndex::default(),
            slot_index: NumberIndex::default(),
            number: NumberInput::default(),
            swap_source: None,
            refresh_pending: None,
            edit: None,
            trees: BTreeMap::new(),
            resume_quick: false,
            size: false,
            temporary: false,
            held: BTreeMap::new(),
            status: None,
            modal: false,
        }
    }

    fn request(&mut self, operation: WindowOperation, out: &mut CommandBatch) {
        if operation.precedes_inventory() {
            self.refresh_pending = None;
        }
        self.request += 1;
        out.push(Command::WindowRequest(Box::new(WindowRequest {
            session: self.session,
            id: self.request,
            operation,
        })));
    }

    fn stop_movement(&mut self, out: &mut CommandBatch) {
        if let Some(edit) = &mut self.edit {
            edit.divider_gesture = false;
        }
        if !self.held.is_empty() {
            self.held.clear();
            out.push(Command::SetFrameClock(false));
        }
    }

    fn cancel_number(&mut self, out: &mut CommandBatch) -> bool {
        let pending = self.number.cancel();
        if pending {
            out.push(Command::CancelTimer {
                id: NUMBER_TIMER.into(),
            });
        }
        pending
    }

    fn cancel_pending(&mut self, out: &mut CommandBatch) {
        self.request(WindowOperation::CancelPending, out);
        self.result = self.request - 1;
        self.refresh_pending = None;
    }

    fn refresh(&mut self, out: &mut CommandBatch) {
        if self.refresh_pending.is_none()
            && !self.temporary
            && self
                .edit
                .as_ref()
                .is_none_or(|e| e.ready && e.in_flight.is_none() && !e.ending)
        {
            self.request(WindowOperation::Enumerate, out);
            self.refresh_pending = Some(self.request);
        }
    }

    fn visible_windows(&self) -> impl Iterator<Item = &WindowInfo> {
        self.visible.iter().filter_map(|id| self.inventory.get(id))
    }

    fn rebuild_numbers(&mut self) {
        if self.inventory_dirty || self.numbered_screen != self.screen {
            self.visible.clear();
            for window in self.inventory.values().filter(|w| w.screen == self.screen) {
                self.numbers.entry(window.id).or_insert_with(|| {
                    let n = self.next_number;
                    self.next_number += 1;
                    n
                });
                self.visible.push(window.id);
            }
            self.visible.sort_unstable_by_key(|id| self.numbers[id]);
            self.window_index = NumberIndex::new(self.visible.iter().map(|id| self.numbers[id]));
            self.inventory_dirty = false;
            self.numbered_screen = self.screen;
        }
        let slots = match self.edit.as_ref().map(|e| &e.model) {
            Some(EditModel::Tree(tree)) => tree.slot_count(),
            _ => 0,
        };
        if slots != self.numbered_slots {
            self.slot_index = NumberIndex::new(self.edit.as_ref().into_iter().flat_map(|edit| {
                if let EditModel::Tree(tree) = &edit.model {
                    tree.slots()
                        .into_iter()
                        .map(|slot| slot.id)
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                }
            }));
            self.numbered_slots = slots;
        }
    }

    fn adjust(&mut self, change: WindowChange, out: &mut CommandBatch) {
        if let Some(target) = &self.target {
            self.request(
                WindowOperation::Adjust {
                    target: target.id,
                    change,
                    group: self.group,
                },
                out,
            );
        }
    }

    fn motion(&mut self, seconds: Option<f64>, ctx: &HostContext<'_>, out: &mut CommandBatch) {
        if self.edit.is_some() {
            self.divider_motion(seconds, ctx, out);
            return;
        }
        if self.temporary || self.edit.is_some() || self.held.is_empty() {
            return;
        }
        let mut x: f64 = 0.0;
        let mut y: f64 = 0.0;
        for action in self.held.values() {
            match action {
                W::Left => x -= 1.0,
                W::Right => x += 1.0,
                W::Up => y -= 1.0,
                W::Down => y += 1.0,
                _ => {}
            }
        }
        x = x.clamp(-1.0, 1.0);
        y = y.clamp(-1.0, 1.0);
        if x == 0.0 && y == 0.0 {
            return;
        }
        let amount = if self.size {
            seconds.map_or(self.settings.resize_step, |s| {
                s * self.settings.resize_speed
            })
        } else {
            seconds.map_or(self.settings.move_step, |s| s * self.settings.move_speed)
        };
        self.adjust(
            if self.size {
                WindowChange::Resize {
                    dw: x * amount,
                    dh: -y * amount,
                }
            } else {
                WindowChange::Move {
                    dx: x * amount,
                    dy: y * amount,
                }
            },
            out,
        );
    }

    fn exit(&mut self, out: &mut CommandBatch) {
        self.stop_movement(out);
        self.cancel_number(out);
        out.push(Command::CancelTimer {
            id: INVENTORY_TIMER.into(),
        });
        out.push(Command::CancelWindowSession(self.session));
        out.push(Command::HideOverlay);
        out.push(match &self.settings.exit_mode {
            crate::api::lifecycle::LifecycleAction::Return if self.modal => Command::PopMode,
            crate::api::lifecycle::LifecycleAction::Return => {
                Command::SwitchMode(self.previous.clone())
            }
            crate::api::lifecycle::LifecycleAction::Mode(mode) => Command::SwitchMode(mode.clone()),
            _ => Command::SwitchMode(ModeId::normal()), // rejected by configuration validation
        });
    }

    fn screen_scale(&self, scale: f64) -> f64 {
        if cfg!(target_os = "windows") {
            scale
        } else {
            1.0
        }
    }
}

impl Mode for WindowMode {
    fn id(&self) -> ModeId {
        ModeId::window()
    }
    fn display_name(&self) -> String {
        "window".into()
    }
    fn wants_pointer_events(&self) -> bool {
        false
    }
    fn window_layout_active(&self) -> bool {
        self.edit.is_some() && !self.temporary
    }
    fn window_action_available(&self, action: &W) -> bool {
        if self.library_open {
            return matches!(action, W::SavedLayouts | W::Cancel | W::Exit | W::Confirm);
        }
        if *action == W::SavedLayouts {
            return true;
        }
        if *action == W::SaveLayout {
            return self
                .edit
                .as_ref()
                .is_some_and(|e| e.ready && matches!(e.model, EditModel::Tree(_)));
        }
        match self.edit.as_ref().map(|e| &e.model) {
            None => !matches!(
                action,
                W::Navigate(_) | W::Split(_) | W::Ratio(_) | W::RemoveRegion
            ),
            Some(EditModel::Quick(_)) => matches!(
                action,
                W::Navigate(_)
                    | W::Layout
                    | W::Edit
                    | W::Select
                    | W::Undo
                    | W::Confirm
                    | W::Cancel
                    | W::Exit
                    | W::Tile
            ),
            Some(EditModel::Tree(_)) => matches!(
                action,
                W::Navigate(_)
                    | W::RemoveRegion
                    | W::Split(_)
                    | W::Ratio(_)
                    | W::Select
                    | W::Undo
                    | W::Confirm
                    | W::Cancel
                    | W::Exit
                    | W::Tile
            ),
        }
    }
    fn indicator_detail(&self) -> Option<String> {
        Some(self.detail())
    }
    fn help_anchor(&self) -> Option<Rect> {
        if self.library_open {
            return None;
        }
        self.target.as_ref().map(|w| w.bounds)
    }
    fn help_previews(&self) -> Vec<(String, String, Rect, bool)> {
        if self.temporary || self.library_open {
            return Vec::new();
        }
        match self.edit.as_ref().map(|e| &e.model) {
            Some(EditModel::Quick(quick)) => {
                vec![(String::new(), quick.caption(), quick.rect(), true)]
            }
            _ => Vec::new(),
        }
    }
    fn claims_key(&self, key: &Key) -> bool {
        if self.library_open {
            return !self.temporary
                && (key.as_char().is_some_and(|c| c.is_ascii_digit())
                    || matches!(key.as_str(), "page_up" | "page_down"));
        }
        !self.temporary
            && key
                .as_char()
                .is_some_and(|c| c.is_ascii_digit() || c == '`')
    }
    fn available_keys(&self) -> Vec<(String, String)> {
        if self.library_open {
            let mut keys = Vec::new();
            if self.library_page > 0 {
                keys.push(("PageUp".into(), "Previous page".into()));
            }
            if (self.library_page + 1) * presets::PAGE_SIZE < self.saved_layouts.len() {
                keys.push(("PageDown".into(), "Next page".into()));
            }
            return keys;
        }
        let mut keys: Vec<_> = ('1'..='9')
            .map(|c| (c.to_string(), "Window number".into()))
            .collect();
        keys.push(("`".into(), "Area number".into()));
        keys
    }
    fn handle_owned(&mut self, event: ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        match event {
            ModeEvent::WindowResult(result) => self.window_result(*result, ctx),
            ModeEvent::WindowLayouts(result) => self.library_result(*result, ctx),
            other => self.handle(&other, ctx),
        }
    }
    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        if let Some(out) = self.library_event(event, ctx) {
            return out;
        }
        let mut out = CommandBatch::new();
        let mut redraw = true;
        match event {
            ModeEvent::Activated { .. } | ModeEvent::Pushed { .. } | ModeEvent::Restarted => {
                match event {
                    ModeEvent::Activated { previous } => {
                        self.previous = previous.clone().unwrap_or_else(ModeId::normal)
                    }
                    ModeEvent::Pushed { previous } => self.previous = previous.clone(),
                    _ => {}
                }
                static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
                if self.session != 0 {
                    out.push(Command::CancelWindowSession(self.session));
                }
                self.modal = matches!(event, ModeEvent::Pushed { .. })
                    || matches!(event, ModeEvent::Restarted) && self.modal;
                self.session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
                self.request = 0;
                self.result = 0;
                self.group = 0;
                self.library_open = false;
                self.save_pending = false;
                self.pending_template = None;
                self.size = false;
                self.temporary = false;
                self.held.clear();
                self.target = None;
                self.edit = None;
                self.inventory.clear();
                self.visible.clear();
                self.inventory_dirty = true;
                self.window_index = NumberIndex::default();
                self.slot_index = NumberIndex::default();
                self.numbered_slots = 0;
                self.numbers.clear();
                self.next_number = 1;
                self.trees.clear();
                self.resume_quick = false;
                self.number.cancel();
                self.swap_source = None;
                self.refresh_pending = None;
                self.screen = ctx
                    .screens
                    .iter()
                    .position(|s| s.bounds.contains(&ctx.cursor))
                    .unwrap_or(0);
                self.status = Some("Finding target…".into());
                self.request(WindowOperation::Acquire(ctx.cursor), &mut out);
                out.push(Command::SetTimer {
                    id: INVENTORY_TIMER.into(),
                    delay: Duration::from_millis(500),
                    repeating: true,
                });
            }
            ModeEvent::Deactivated => {
                self.stop_movement(&mut out);
                out.push(Command::CancelTimer {
                    id: NUMBER_TIMER.into(),
                });
                out.push(Command::CancelTimer {
                    id: INVENTORY_TIMER.into(),
                });
                out.push(Command::CancelWindowSession(self.session));
                self.session = 0;
                self.numbers.clear();
                self.status = None;
                self.target = None;
                self.edit = None;
                self.inventory.clear();
                self.visible.clear();
                self.inventory_dirty = true;
                self.window_index = NumberIndex::default();
                self.slot_index = NumberIndex::default();
                self.numbered_slots = 0;
                self.trees.clear();
                self.number.cancel();
                return out;
            }
            ModeEvent::Suspended => {
                self.stop_movement(&mut out);
            }
            ModeEvent::Resumed => {}
            ModeEvent::TemporaryModeChanged { active } => {
                if *active {
                    if self.edit.is_none() {
                        self.cancel_pending(&mut out);
                    }
                    out.push(Command::CancelTimer {
                        id: NUMBER_TIMER.into(),
                    });
                }
                self.temporary = *active;
                self.stop_movement(&mut out);
                if !active {
                    self.flush_edit(&mut out);
                    if self.number.needs_timer(
                        if self.library_open {
                            &self.library_index
                        } else {
                            &self.window_index
                        },
                        &self.slot_index,
                    ) {
                        out.push(Command::SetTimer {
                            id: NUMBER_TIMER.into(),
                            delay: Duration::from_millis(self.settings.number_timeout_ms),
                            repeating: false,
                        });
                    }
                }
            }
            ModeEvent::WindowResult(result) => return self.window_result((**result).clone(), ctx),
            ModeEvent::WindowLayouts(result) => {
                return self.library_result((**result).clone(), ctx);
            }
            ModeEvent::Binding {
                binding,
                state,
                key,
            } => {
                let Binding::Window(action) = binding.as_ref() else {
                    return out;
                };
                if *state == KeyState::Down {
                    self.status = None;
                }
                self.action(*action, *state, key, ctx, &mut out);
                redraw = *state == KeyState::Down;
            }
            ModeEvent::Key {
                key,
                state: KeyState::Down,
                repeat: false,
            } if !self.temporary => {
                if let Some(c) = key.as_char() {
                    if c == '`' {
                        self.cancel_number(&mut out);
                        if self
                            .edit
                            .as_ref()
                            .is_some_and(|edit| matches!(edit.model, EditModel::Quick(_)))
                        {
                            self.finish_edit(Finish::Tree, &mut out);
                        } else if self.edit.is_none() {
                            self.start_edit(true, ctx, &mut out);
                        }
                        if self.edit.is_some() {
                            self.number.begin_slot();
                        }
                    } else if c.is_ascii_digit() {
                        self.status = None;
                        let completed = if self.number.slot
                            && !self.edit.as_ref().is_some_and(|edit| {
                                edit.ready && matches!(edit.model, EditModel::Tree(_))
                            }) {
                            self.number.queue_digit(c);
                            smallvec::SmallVec::new()
                        } else {
                            self.number.digit(c, &self.window_index, &self.slot_index)
                        };
                        for (slot, number) in completed {
                            self.choose_number(slot, number, ctx, &mut out);
                        }
                        if self
                            .number
                            .needs_timer(&self.window_index, &self.slot_index)
                        {
                            out.push(Command::SetTimer {
                                id: NUMBER_TIMER.into(),
                                delay: Duration::from_millis(self.settings.number_timeout_ms),
                                repeating: false,
                            });
                        } else {
                            out.push(Command::CancelTimer {
                                id: NUMBER_TIMER.into(),
                            });
                        }
                    } else {
                        self.cancel_number(&mut out);
                        self.swap_source = None;
                    }
                } else {
                    redraw = false;
                }
            }
            ModeEvent::Timer { id, .. } if id == NUMBER_TIMER && !self.temporary => {
                if let Some((slot, number)) =
                    self.number.finish(&self.window_index, &self.slot_index)
                {
                    self.choose_number(slot, number, ctx, &mut out);
                }
            }
            ModeEvent::Timer { id, .. } if id == INVENTORY_TIMER => {
                self.refresh(&mut out);
                redraw = false;
            }
            ModeEvent::Frame { elapsed } => {
                self.motion(Some(elapsed.as_secs_f64().min(0.1)), ctx, &mut out);
                redraw = false;
            }
            ModeEvent::ScreensChanged(_) => {
                self.stop_movement(&mut out);
                if self.edit.is_some() {
                    self.finish_edit(Finish::Cancel, &mut out);
                } else {
                    self.cancel_pending(&mut out);
                    self.refresh(&mut out);
                }
                self.trees.clear();
            }
            _ => redraw = false,
        }
        if out
            .iter()
            .any(|c| matches!(c, Command::PopMode | Command::SwitchMode(_)))
        {
            return out;
        }
        if redraw && !out.iter().any(|c| matches!(c, Command::WindowLayouts(_))) {
            out.push(ctx.present(self.view()));
        }
        out
    }
}
