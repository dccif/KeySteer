//! Window interaction state. Native objects and checkpoints belong to the worker.
mod editing;
mod mode;
pub use mode::{WindowKind, WindowMode};
mod interaction;
mod inventory;
mod numbering;
mod presets;
mod tabs;
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
    Binding, Command, CommandBatch, Direction, HostContext, Key, KeyState, Mode, ModeEvent, ModeId,
    Point, Rect,
};
use numbering::{NumberIndex, NumberInput};

const NUMBER_TIMER: &str = "window_number";
const INVENTORY_TIMER: &str = "window_inventory";

#[derive(Clone, Debug)]
pub struct Settings {
    pub all_screens: bool,
    pub include_minimized: bool,
    pub lifecycle: crate::api::TargetingLifecycle,
    pub split_ratios: Vec<f64>,
    pub ratio_ticks: std::sync::Arc<[crate::api::window_layout::RatioTick]>,
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
    History { redo: bool },
    ResetInitial,
    Tree,
    Select(WindowId),
    Cycle { backwards: bool },
    Tile,
    Transition,
}

struct LiveEdit {
    additional_trees: BTreeMap<usize, LayoutTree>,
    transaction: u64,
    screen: usize,
    model: EditModel,
    accepted: EditModel,
    history: Vec<EditModel>,
    redo: Vec<EditModel>,
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

impl LiveEdit {
    fn remember(&mut self, before: EditModel) {
        if self.history.len() == 32 {
            self.history.remove(0);
        }
        self.history.push(before);
        self.redo.clear();
    }
}

enum DeferredEdit {
    Direction(crate::api::Direction, bool, bool),
    Number(bool, u32),
}

pub struct WindowSession {
    tabs: tabs::Interaction,
    settings: Settings,
    kind: WindowKind,
    pending_transition: Option<ModeId>,
    pending_handoff: bool,
    preserve_session: bool,
    enter_pending: bool,
    restore_pending: bool,
    finished: bool,
    deleting_presets: bool,
    delete_selection: Option<crate::api::window_presets::SavedPreset>,
    saved_presets: Vec<crate::api::window_presets::SavedPreset>,
    library_open: bool,
    save_pending: bool,
    library_page: usize,
    library_index: NumberIndex,
    pending_template: Option<crate::api::window_presets::SavedPreset>,
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
    reopen_edit: Option<u64>,
    size: bool,
    temporary: bool,
    held: BTreeMap<Key, W>,
    status: Option<String>,
    previous: ModeId,
}

impl WindowSession {
    pub fn new(settings: Settings) -> Self {
        Self {
            tabs: tabs::Interaction::default(),
            settings,
            kind: WindowKind::Move,
            pending_transition: None,
            pending_handoff: false,
            preserve_session: false,
            enter_pending: false,
            restore_pending: false,
            finished: false,
            deleting_presets: false,
            delete_selection: None,
            previous: ModeId::normal(),
            saved_presets: Vec::new(),
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
            reopen_edit: None,
            size: false,
            temporary: false,
            held: BTreeMap::new(),
            status: None,
        }
    }

    fn request(&mut self, operation: WindowOperation, out: &mut CommandBatch) {
        if operation.precedes_inventory() {
            self.refresh_pending = None;
        }
        self.request += 1;
        out.push(Command::WindowRequest(Box::new(WindowRequest {
            scope: Some(crate::api::window::WindowScope {
                screen: (!self.settings.all_screens).then_some(self.screen),
                include_minimized: self.settings.include_minimized,
            }),
            session: self.session,
            id: self.request,
            operation,
        })));
    }

    fn request_audio(
        &mut self,
        target: crate::api::audio::AudioTarget,
        action: crate::api::audio::AudioAction,
        out: &mut CommandBatch,
    ) {
        self.request += 1;
        out.push(Command::AudioRequest(Box::new(
            crate::api::audio::AudioRequest {
                session: self.session,
                id: self.request,
                target,
                action,
            },
        )));
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
            for window in crate::api::window::application_number_order(
                self.inventory.values().filter(|w| {
                    (self.settings.include_minimized || !w.minimized)
                        && (self.settings.all_screens || w.screen == self.screen)
                }),
                self.numbers.iter().map(|(id, number)| (*id, *number)),
            ) {
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

    fn screen_scale(&self, scale: f64) -> f64 {
        if cfg!(target_os = "windows") {
            scale
        } else {
            1.0
        }
    }
}

impl WindowKind {
    fn supports_action(self, action: &W) -> bool {
        if matches!(
            action,
            W::VolumeDown
                | W::VolumeUp
                | W::VolumeMute
                | W::AudioPrevious
                | W::AudioNext
                | W::SystemVolumeDown
                | W::SystemVolumeUp
                | W::SystemAudioPrevious
                | W::SystemAudioNext
        ) {
            return matches!(self, Self::Move | Self::Quick | Self::Editor);
        }
        match self {
            WindowKind::Tab => matches!(
                action,
                W::TabEnd
                    | W::TabPrefix
                    | W::TabSeparator
                    | W::TabRemove
                    | W::TabDissolve
                    | W::TabNext
                    | W::TabPrevious
                    | W::TabMoveLeft
                    | W::TabMoveRight
                    | W::Undo
                    | W::Redo
                    | W::SaveLayout
            ),
            WindowKind::Move => matches!(
                action,
                W::Tile
                    | W::Left
                    | W::Down
                    | W::Up
                    | W::Right
                    | W::Size
                    | W::NextScreen
                    | W::PreviousScreen
                    | W::CycleState
                    | W::Center
                    | W::Close
                    | W::Select
                    | W::SelectPrevious
                    | W::Undo
                    | W::Redo
                    | W::ResetInitial
            ),
            WindowKind::Quick => matches!(
                action,
                W::Navigate(_)
                    | W::Select
                    | W::SelectPrevious
                    | W::Undo
                    | W::Redo
                    | W::ResetInitial
            ),
            WindowKind::Editor => {
                matches!(action, W::SaveLayout)
                    || matches!(
                        action,
                        W::Navigate(_)
                            | W::Split(_)
                            | W::Ratio(_)
                            | W::Select
                            | W::SelectPrevious
                            | W::Undo
                            | W::Redo
                            | W::ResetInitial
                            | W::RemoveRegion
                    )
            }
            WindowKind::Restore => matches!(action, W::Confirm | W::DeletePreset),
        }
    }
}

impl WindowSession {
    fn window_action_available(&self, action: &W) -> bool {
        if !self.kind.supports_action(action) {
            return false;
        }
        match (self.kind, action) {
            (WindowKind::Editor, W::SaveLayout) => self
                .edit
                .as_ref()
                .is_some_and(|e| e.ready && e.finishing.is_none()),
            (WindowKind::Restore, W::Confirm) => {
                !self.restore_pending && (self.number.pending() || self.delete_selection.is_some())
            }
            _ => true,
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
    fn quick_ruler(&self) -> Option<crate::api::window_layout::QuickRuler> {
        if self.temporary || self.kind != WindowKind::Quick {
            return None;
        }
        let selected = match self.edit.as_ref().map(|e| &e.model) {
            Some(EditModel::Quick(quick)) => quick.rect_with(&self.settings.split_ratios),
            _ => Rect::new(0.0, 0.0, 1.0, 1.0),
        };
        Some(crate::api::window_layout::QuickRuler {
            ticks: self.settings.ratio_ticks.clone(),
            selected,
        })
    }
    fn claims_key(&self, key: &Key) -> bool {
        if self.library_open {
            return !self.temporary
                && (key.as_char().is_some_and(|c| c.is_ascii_digit())
                    || matches!(key.as_str(), "page_up" | "page_down"));
        }
        !self.temporary
            && key.as_char().is_some_and(|c| {
                c.is_ascii_digit() || (c == '`' && self.kind == WindowKind::Editor)
            })
    }
    fn available_keys(&self) -> Vec<(String, String)> {
        if self.library_open {
            let mut keys = Vec::new();
            if self.library_page > 0 {
                keys.push(("PageUp".into(), "Previous page".into()));
            }
            if (self.library_page + 1) * presets::PAGE_SIZE < self.saved_presets.len() {
                keys.push(("PageDown".into(), "Next page".into()));
            }
            return keys;
        }
        let mut keys: Vec<_> = ('1'..='9')
            .map(|c| (c.to_string(), "Window number".into()))
            .collect();
        if self.kind == WindowKind::Editor {
            keys.push(("`".into(), "Area number".into()));
        }
        keys
    }
    pub(crate) fn handle_owned(&mut self, event: ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        match event {
            ModeEvent::WindowResult(result) => self.window_result(*result, ctx),
            ModeEvent::WindowPresets(result) => self.library_result(*result, ctx),
            other => self.handle(&other, ctx),
        }
    }
    pub(crate) fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
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
                self.finished = false;
                self.delete_selection = None;
                self.deleting_presets = false;
                self.pending_transition = None;
                self.pending_handoff = false;
                self.tabs.queue.clear();
                self.tabs.in_flight = None;
                if self.session != 0 && !matches!(event, ModeEvent::Restarted) {
                    self.temporary = false;
                    self.enter_kind(ctx, &mut out);
                    out.push(Command::SetTimer {
                        id: INVENTORY_TIMER.into(),
                        delay: Duration::from_millis(500),
                        repeating: true,
                    });
                    out.push(ctx.present(self.view()));
                    return out;
                }
                static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
                if self.session != 0 {
                    out.push(Command::CancelAudioSession(self.session));
                    out.push(Command::CancelWindowSession(self.session));
                }
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
                self.reopen_edit = None;
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
                self.enter_pending = true;
                if self.kind.is_library() {
                    self.open_library(&mut out);
                }
                out.push(Command::SetTimer {
                    id: INVENTORY_TIMER.into(),
                    delay: Duration::from_millis(500),
                    repeating: true,
                });
            }
            ModeEvent::Deactivated => {
                if self.kind == WindowKind::Tab {
                    self.tabs.restore = None;
                    self.tabs.restoring = None;
                }
                self.stop_movement(&mut out);
                self.cancel_number(&mut out);
                self.delete_selection = None;
                self.deleting_presets = false;
                if std::mem::take(&mut self.preserve_session) {
                    self.restore_pending = false;
                    self.pending_template = None;
                    self.save_pending = false;
                    return out;
                }
                out.push(Command::CancelTimer {
                    id: NUMBER_TIMER.into(),
                });
                out.push(Command::CancelTimer {
                    id: INVENTORY_TIMER.into(),
                });
                out.push(Command::CancelAudioSession(self.session));
                out.push(Command::CancelWindowSession(self.session));
                self.session = 0;
                self.reopen_edit = None;
                self.pending_transition = None;
                self.pending_handoff = false;
                self.enter_pending = false;
                self.restore_pending = false;
                self.library_open = false;
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
            ModeEvent::FinishRequested { .. } => {
                if !self.finished {
                    self.finished = true;
                    out.extend(super::targeting::lifecycle_commands(
                        &self.settings.lifecycle.after_finish,
                        &self.previous,
                    ));
                }
            }
            ModeEvent::Clicked { .. } => {
                out.extend(super::targeting::lifecycle_commands(
                    &self.settings.lifecycle.after_click,
                    &self.previous,
                ));
            }
            ModeEvent::TemporaryModeChanged { active } => {
                if *active {
                    if self.edit.is_none() && self.kind != WindowKind::Tab {
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
            ModeEvent::AudioResult(result) => {
                if result.session != self.session {
                    return out;
                }
                self.status = Some(match &result.outcome {
                    Ok(message) | Err(message) => message.clone(),
                });
            }
            ModeEvent::WindowResult(result) => return self.window_result((**result).clone(), ctx),
            ModeEvent::WindowPresets(result) => {
                return self.library_result((**result).clone(), ctx);
            }
            ModeEvent::Binding {
                binding,
                state,
                key,
            } => {
                let action = match binding.as_ref() {
                    Binding::Window(action) => *action,
                    Binding::Move(direction) if self.kind == WindowKind::Tab => match direction {
                        Direction::Left => W::TabMoveLeft,
                        Direction::Right => W::TabMoveRight,
                        Direction::Up => W::TabPrevious,
                        Direction::Down => W::TabNext,
                    },
                    _ => return out,
                };
                if *state == KeyState::Down {
                    self.status = None;
                }
                self.action(action, *state, key, ctx, &mut out);
                redraw = *state == KeyState::Down;
            }
            ModeEvent::Key {
                key,
                state: KeyState::Down,
                repeat: false,
            } if !self.temporary => {
                if let Some(c) = key.as_char() {
                    if c == '`' && self.kind == WindowKind::Editor {
                        self.cancel_number(&mut out);
                        if self.edit.is_some() {
                            self.number.begin_slot();
                        }
                    } else if c.is_ascii_digit() {
                        if self.kind == WindowKind::Tab {
                            self.tab_input(tabs::Input::Digit(c), ctx, &mut out);
                            out.push(ctx.present(self.view()));
                            return out;
                        }
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
                if self.kind == WindowKind::Tab {
                    self.tab_input(tabs::Input::Action(W::TabSeparator), ctx, &mut out);
                    out.push(ctx.present(self.view()));
                    return out;
                }
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
        if redraw && !out.iter().any(|c| matches!(c, Command::WindowPresets(_))) {
            out.push(ctx.present(self.view()));
        }
        out
    }
}
