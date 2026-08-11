//! Hint mode: label every clickable element and jump to one by typing.
//!
//! On activation the mode asks the platform to walk the accessibility tree.
//! When results arrive each element receives a short label; typing a label
//! warps the pointer to that element and finishes the targeting session.
//!
//! `/` enters search mode, where typing filters elements by their accessible
//! name instead of matching labels.

use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

use crate::api::binding::Binding;
use crate::api::command::{
    Command, CommandBatch, FinishCause, HostContext, Mode, ModeEvent, UiScanRequest, UiScanStatus,
};
use crate::api::geometry::{Rect, UiTarget};
use crate::api::input::{Key, KeyState, ModeId};
use crate::api::overlay::{Color, OverlayLabel, OverlayScene, OverlayShape};
use crate::config::{Config, Palette, UiHint as HintsConfig};
use crate::hints::{self, Hint, Match};

const SCAN_RETRY_TIMER_ID: &str = "ui_hint.scan_retry";
/// Keep small scans warm, but do not pin a multi-thousand-target allocation
/// after leaving this comparatively heavy mode.
const MAX_IDLE_RETAINED_TARGETS: usize = 128;
const MAX_SCAN_TIMEOUT_MS: u64 = 30_000;

/// What the keyboard is currently doing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Input {
    /// Typing hint labels.
    Labels(String),
    /// Typing a search query that filters by element name.
    Search(String),
}

impl Input {
    /// Text typed so far, whether that is a label prefix or a search query.
    fn text(&self) -> &str {
        match self {
            Input::Labels(s) | Input::Search(s) => s,
        }
    }
}

pub struct HintMode {
    config: HintsConfig,
    alphabet: Vec<char>,

    /// Everything the current scan has returned so far.
    scanned: Vec<UiTarget>,
    scanned_names_lower: Vec<String>,
    seen_targets: HashSet<(i64, i64, i64, i64, String, String)>,
    /// Labelled subset currently on screen; the value is an index into
    /// `scanned`.
    hints: Vec<Hint<usize>>,
    input: Input,
    scanning: bool,
    status: Option<String>,
    return_mode: ModeId,
    scan_id: u64,
    retry_attempt: u32,
    retry_pending: bool,
    /// Display bounds used by the newest scan. Pointer movement to another
    /// display invalidates both pending and rendered results.
    scan_bounds: Option<Rect>,
    held_overlap_keys: BTreeSet<Key>,
    overlap_cycle: usize,
    selected: Option<usize>,
    finished: bool,
}

impl HintMode {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.ui_hint.clone(),
            alphabet: config.ui_hint.hint_characters.chars().collect(),
            scanned: Vec::new(),
            scanned_names_lower: Vec::new(),
            seen_targets: HashSet::new(),
            hints: Vec::new(),
            input: Input::Labels(String::new()),
            scanning: false,
            status: None,
            return_mode: ModeId::idle(),
            scan_id: 0,
            retry_attempt: 0,
            retry_pending: false,
            scan_bounds: None,
            held_overlap_keys: BTreeSet::new(),
            overlap_cycle: 0,
            selected: None,
            finished: false,
        }
    }

    fn clear_scan_results(&mut self, release_large_buffers: bool) {
        self.scanned.clear();
        self.scanned_names_lower.clear();
        self.seen_targets.clear();
        self.hints.clear();
        if release_large_buffers {
            if self.scanned.capacity() > MAX_IDLE_RETAINED_TARGETS {
                self.scanned = Vec::new();
            }
            if self.scanned_names_lower.capacity() > MAX_IDLE_RETAINED_TARGETS {
                self.scanned_names_lower = Vec::new();
            }
            if self.hints.capacity() > MAX_IDLE_RETAINED_TARGETS {
                self.hints = Vec::new();
            }
            if self.seen_targets.capacity() > MAX_IDLE_RETAINED_TARGETS {
                self.seen_targets = HashSet::new();
            }
        }
    }

    fn request_scan(&mut self, ctx: &HostContext<'_>) -> Vec<Command> {
        self.scanning = true;
        self.status = None;
        self.clear_scan_results(false);
        self.input = Input::Labels(String::new());
        self.selected = None;
        self.finished = false;
        self.retry_attempt = 0;
        self.retry_pending = false;
        let request = self.scan_request(ctx);
        vec![
            Command::CancelTimer {
                id: SCAN_RETRY_TIMER_ID.into(),
            },
            Command::HideOverlay,
            request,
        ]
    }

    fn retry_scan(&mut self, ctx: &HostContext<'_>) -> Vec<Command> {
        self.retry_attempt = self.retry_attempt.saturating_add(1);
        self.retry_pending = false;
        self.scanning = true;
        vec![self.scan_request(ctx)]
    }

    fn scan_request(&mut self, ctx: &HostContext<'_>) -> Command {
        self.scan_id = self.scan_id.wrapping_add(1);
        let bounds = ctx.active_bounds();
        self.scan_bounds = Some(bounds);
        let timeout_multiplier = u64::from(self.retry_attempt).saturating_add(1);
        let timeout_ms = self
            .config
            .scan_timeout_ms
            .saturating_mul(timeout_multiplier)
            .min(MAX_SCAN_TIMEOUT_MS);
        Command::scan_ui(UiScanRequest {
            id: self.scan_id,
            timeout_ms,
            bounds: Some(bounds),
            roles: self.config.clickable_roles.clone(),
            max_depth: self.config.max_depth,
            visible_only: self.config.visible_check_enabled,
            clickable_only: !self.config.ignore_clickable_check,
            strategy: self.config.strategy_for(ctx.focused_app),
            vision: self.config.vision.clone(),
            app: ctx.focused_app.cloned(),
        })
    }

    fn append_targets(&mut self, targets: &[UiTarget]) -> bool {
        let before = self.scanned.len();
        for target in targets {
            if !self
                .scan_bounds
                .is_none_or(|bounds| bounds.contains(&target.rect.center()))
            {
                continue;
            }
            let rect = target.rect;
            let key = (
                (rect.x * 4.0).round() as i64,
                (rect.y * 4.0).round() as i64,
                (rect.width * 4.0).round() as i64,
                (rect.height * 4.0).round() as i64,
                target.name.clone(),
                target.role.clone(),
            );
            if self.seen_targets.insert(key) {
                self.scanned_names_lower.push(target.name.to_lowercase());
                self.scanned.push(target.clone());
            }
        }
        self.scanned.len() != before
    }

    /// Assign labels to the targets matching the current search query.
    fn relabel(&mut self) {
        let query = match &self.input {
            Input::Search(q) => q.to_lowercase(),
            Input::Labels(_) => String::new(),
        };

        let candidates = self
            .scanned
            .iter()
            .zip(&self.scanned_names_lower)
            .enumerate()
            .filter(|(_, (_, name_lower))| query.is_empty() || name_lower.contains(&query))
            .map(|(i, (target, _))| (target.rect, i));

        self.hints = hints::assign(candidates, &self.alphabet, self.config.label_direction)
            .unwrap_or_default();
    }

    fn hint_is_visible(&self, hint: &Hint<usize>) -> bool {
        match &self.input {
            Input::Labels(typed) => typed.is_empty() || hint.label.starts_with(typed),
            Input::Search(_) => true,
        }
    }

    fn overlap_cycle_key(
        &mut self,
        key: &Key,
        state: KeyState,
        ctx: &HostContext<'_>,
    ) -> Vec<Command> {
        let changed = match state {
            KeyState::Down => {
                let was_released = self.held_overlap_keys.is_empty();
                let inserted = self.held_overlap_keys.insert(key.clone());
                if inserted && was_released {
                    self.overlap_cycle = self.overlap_cycle.wrapping_add(1);
                }
                inserted
            }
            KeyState::Up => self.held_overlap_keys.remove(key),
        };
        if changed && !self.scanning && !self.hints.is_empty() {
            self.redraw(ctx)
        } else {
            Vec::new()
        }
    }

    fn scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
        let palette = ctx.palette;
        let placement = self.config.placement;
        let shape_capacity = if self.config.boundary_highlight.enabled {
            self.hints.len()
        } else {
            0
        };
        let label_capacity =
            self.hints.len() + usize::from(matches!(&self.input, Input::Search(_)));
        let mut scene = OverlayScene::with_capacity(shape_capacity, label_capacity);
        scene.clip = self.scan_bounds.or_else(|| Some(ctx.active_bounds()));

        let mut label_style = self.config.ui.resolve(
            palette,
            palette.surface_label(),
            palette.text,
            palette.accent,
        );
        // This highlight belongs specifically to UI Hint's typed-prefix
        // interaction. Keep the generic overlay/config defaults unchanged.
        if self.config.ui.matched_text_color.is_none() {
            label_style.matched_text_color = Color::rgb(0xE4, 0xB4, 0x00);
        }

        // Optional outlines behind only the currently visible candidates.
        if self.config.boundary_highlight.enabled {
            let bh = &self.config.boundary_highlight;
            for hint in self.hints.iter().filter(|hint| self.hint_is_visible(hint)) {
                scene.push_shape(OverlayShape::Rect {
                    rect: hint.bounds,
                    fill: bh.fill(palette),
                    stroke: bh.stroke(palette),
                    stroke_width: bh.border_width.max(0) as f64,
                    corner_radius: bh.radius(),
                    z_index: 0,
                });
            }
        }

        // Remove non-matching labels as the prefix narrows. The matched part of
        // each remaining label is painted with `matched_text_color`.
        let typed = match &self.input {
            Input::Labels(s) => s.as_str(),
            Input::Search(_) => "",
        };
        for hint in self.hints.iter().filter(|hint| self.hint_is_visible(hint)) {
            let width = label_style.font_size * 0.75 * hint.label.chars().count() as f64
                + label_style.padding_x * 2.0;
            let height = label_style.font_size * 1.4 + label_style.padding_y * 2.0;
            let placed = placement.place(&hint.bounds, width, height);
            let rect = Rect::new(
                placed.x + self.config.label_x_offset as f64,
                placed.y + self.config.label_y_offset as f64,
                placed.width,
                placed.height,
            );
            scene.push_label(
                OverlayLabel::new(hint.label.clone(), rect, label_style.clone())
                    .with_matched_prefix(typed.chars().count())
                    .with_z_index(2),
            );
        }
        if !self.held_overlap_keys.is_empty() {
            rotate_overlapping_labels(&mut scene.labels, self.overlap_cycle);
        }

        // Search box, shown only while searching.
        if let Input::Search(query) = &self.input {
            let cfg = &self.config.search_input_ui;
            let style = cfg.label.resolve(
                palette,
                palette.surface_label(),
                palette.text,
                palette.accent,
            );
            let height = style.font_size * 1.8 + style.padding_y * 2.0;
            let rect = cfg.position.place(
                ctx.active_bounds(),
                cfg.width.max(1) as f64,
                height,
                cfg.x_offset as f64,
                cfg.y_offset as f64,
            );
            scene.push_label(OverlayLabel::new(format!("/{query}"), rect, style).with_z_index(10));
        }

        scene
    }

    fn redraw(&self, ctx: &HostContext<'_>) -> Vec<Command> {
        vec![Command::show_overlay(if self.finished {
            self.finished_scene(ctx)
        } else {
            self.scene(ctx)
        })]
    }

    fn finished_scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
        let mut scene = OverlayScene::new();
        scene.clip = self.scan_bounds.or_else(|| Some(ctx.active_bounds()));
        let Some(target) = self.selected.and_then(|index| self.scanned.get(index)) else {
            return scene;
        };
        let boundary = &self.config.boundary_highlight;
        scene.push_shape(OverlayShape::Rect {
            rect: target.rect,
            fill: boundary.fill(ctx.palette),
            stroke: boundary.stroke(ctx.palette),
            stroke_width: boundary.border_width.max(1) as f64,
            corner_radius: boundary.radius(),
            z_index: 1,
        });
        scene
    }

    fn status_scene(&self, ctx: &HostContext<'_>) -> Vec<Command> {
        let palette = ctx.palette;
        let style = self.config.ui.resolve(
            palette,
            palette.surface_label(),
            palette.text,
            palette.accent,
        );
        let text = self
            .status
            .as_deref()
            .unwrap_or("No accessible targets — Esc to exit");
        let width = text.chars().count() as f64 * style.font_size * 0.65 + style.padding_x * 2.0;
        let height = style.font_size * 1.4 + style.padding_y * 2.0;
        let bounds = ctx.active_bounds();
        let rect = Rect::new(
            bounds.center().x - width / 2.0,
            bounds.center().y - height / 2.0,
            width,
            height,
        );
        let mut scene = OverlayScene::new();
        scene.clip = self.scan_bounds.or(Some(bounds));
        scene.push_label(OverlayLabel::new(text, rect, style).with_z_index(10));
        vec![Command::show_overlay(scene)]
    }

    fn select(&mut self, index: usize) -> Vec<Command> {
        let Some(target) = self.scanned.get(index) else {
            return self.cancel();
        };
        let point = target.rect.center();
        self.selected = Some(index);
        vec![
            Command::warp_to(point),
            Command::FinishMode {
                cause: FinishCause::Selection,
            },
        ]
    }

    fn cancel(&self) -> Vec<Command> {
        vec![
            Command::HideOverlay,
            Command::SwitchMode(self.return_mode.clone()),
        ]
    }

    fn key_down(&mut self, key: &Key, ctx: &HostContext<'_>) -> Vec<Command> {
        if self.finished {
            return match key.as_str() {
                "esc" => self.cancel(),
                "backspace" | "tab" => {
                    self.finished = false;
                    self.selected = None;
                    if let Input::Labels(typed) = &mut self.input {
                        typed.pop();
                    }
                    self.redraw(ctx)
                }
                _ => Vec::new(),
            };
        }
        // Before the first batch arrives only allow bailing out. Once partial
        // labels exist they are immediately usable while scanning continues.
        if self.scanning && self.hints.is_empty() {
            return if key.as_str() == "esc" {
                self.cancel()
            } else {
                Vec::new()
            };
        }

        match key.as_str() {
            "esc" => {
                // Escape leaves search first, then the mode.
                return match &self.input {
                    Input::Search(_) => {
                        self.input = Input::Labels(String::new());
                        self.relabel();
                        self.redraw(ctx)
                    }
                    Input::Labels(_) => self.cancel(),
                };
            }
            "/" if matches!(self.input, Input::Labels(_)) => {
                self.input = Input::Search(String::new());
                self.relabel();
                return self.redraw(ctx);
            }
            "backspace" => {
                let text = self.input.text();
                if text.is_empty() {
                    // Nothing left to undo: leave search, or leave the mode.
                    return match &self.input {
                        Input::Search(_) => {
                            self.input = Input::Labels(String::new());
                            self.relabel();
                            self.redraw(ctx)
                        }
                        Input::Labels(_) => self.cancel(),
                    };
                }
                match &mut self.input {
                    Input::Labels(s) | Input::Search(s) => {
                        s.pop();
                    }
                }
                if matches!(self.input, Input::Search(_)) {
                    self.relabel();
                }
                return self.redraw(ctx);
            }
            "enter" => {
                // Accept the first visible candidate, never one filtered out by
                // the label prefix.
                return match self.hints.iter().find(|hint| self.hint_is_visible(hint)) {
                    Some(hint) => self.select(hint.value),
                    None => self.cancel(),
                };
            }
            _ => {}
        }

        let Some(ch) = key.as_char() else {
            return Vec::new();
        };

        match &mut self.input {
            Input::Search(query) => {
                if ch.is_alphanumeric() || ch == ' ' {
                    query.push(ch);
                    self.relabel();
                    return self.redraw(ctx);
                }
                Vec::new()
            }
            Input::Labels(typed) => {
                if !self.alphabet.contains(&ch) {
                    return Vec::new();
                }
                typed.push(ch);
                let candidate = typed.clone();
                match hints::match_input(&self.hints, &candidate) {
                    Match::Complete(index) => self.select(index),
                    Match::Partial { .. } => self.redraw(ctx),
                    // Dead end: drop the character and keep the hints up.
                    Match::None => {
                        if let Input::Labels(typed) = &mut self.input {
                            typed.pop();
                        }
                        Vec::new()
                    }
                }
            }
        }
    }
}

/// Treat intersecting label rectangles as connected overlap groups. While
/// the configured cycle modifier is held, one member of each group is raised.
/// The stable default order is untouched, so releasing it restores that order.
fn rotate_overlapping_labels(labels: &mut [OverlayLabel], cycle: usize) {
    fn root(parents: &mut [usize], mut index: usize) -> usize {
        while parents[index] != index {
            parents[index] = parents[parents[index]];
            index = parents[index];
        }
        index
    }

    let count = labels.len();
    let mut parents: Vec<usize> = (0..count).collect();
    let mut ranks = vec![0u8; count];
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_unstable_by(|left, right| {
        labels[*left]
            .rect
            .left()
            .total_cmp(&labels[*right].rect.left())
    });

    // Sweep from left to right. Only rectangles whose right edge still
    // reaches the current label can intersect it, avoiding the previous full
    // n-by-n scan for ordinary sparse UI layouts.
    let mut active: Vec<usize> = Vec::new();
    for index in order {
        let left = labels[index].rect.left();
        active.retain(|other| labels[*other].rect.right() >= left);
        for &other in &active {
            if labels[index].rect.intersect(&labels[other].rect).is_none() {
                continue;
            }
            let mut left_root = root(&mut parents, index);
            let mut right_root = root(&mut parents, other);
            if left_root == right_root {
                continue;
            }
            if ranks[left_root] < ranks[right_root] {
                std::mem::swap(&mut left_root, &mut right_root);
            }
            parents[right_root] = left_root;
            if ranks[left_root] == ranks[right_root] {
                ranks[left_root] = ranks[left_root].saturating_add(1);
            }
        }
        active.push(index);
    }

    let mut sizes = vec![0usize; count];
    for index in 0..count {
        let component = root(&mut parents, index);
        sizes[component] += 1;
    }
    let mut positions = vec![0usize; count];
    for (index, label) in labels.iter_mut().enumerate() {
        let component = root(&mut parents, index);
        let size = sizes[component];
        if size > 1 && positions[component] == (cycle + size - 1) % size {
            label.z_index = 3;
        }
        positions[component] += 1;
    }
}

impl Mode for HintMode {
    fn id(&self) -> ModeId {
        ModeId::ui_hint()
    }

    fn display_name(&self) -> String {
        "Hints".into()
    }

    fn indicator_color(&self, palette: &Palette) -> Option<Color> {
        Some(palette.accent)
    }

    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        CommandBatch::from(match event {
            ModeEvent::Activated { previous } => {
                self.return_mode = previous.clone().unwrap_or_else(ModeId::idle);
                self.request_scan(ctx)
            }
            ModeEvent::Restarted => self.request_scan(ctx),
            ModeEvent::FinishRequested { .. } if self.finished => Vec::new(),
            ModeEvent::FinishRequested { .. } => {
                self.finished = true;
                let mut commands = self.redraw(ctx);
                commands.extend(super::lifecycle_commands(
                    &self.config.lifecycle.after_finish,
                    &self.return_mode,
                ));
                commands
            }
            ModeEvent::Clicked { .. } => {
                super::lifecycle_commands(&self.config.lifecycle.after_click, &self.return_mode)
            }
            ModeEvent::Deactivated => {
                self.clear_scan_results(true);
                self.scanning = false;
                self.scan_bounds = None;
                self.input = Input::Labels(String::new());
                self.held_overlap_keys.clear();
                self.overlap_cycle = 0;
                self.retry_pending = false;
                self.selected = None;
                self.finished = false;
                vec![Command::CancelTimer {
                    id: SCAN_RETRY_TIMER_ID.into(),
                }]
            }
            ModeEvent::UiScanned(_) if self.finished => Vec::new(),
            ModeEvent::UiScanned(result) if result.id == self.scan_id => {
                if result.status == UiScanStatus::ContextChanged {
                    self.scanning = false;
                    self.clear_scan_results(false);
                    self.status = Some("Focused window changed — Esc to exit".into());
                    return self.status_scene(ctx).into();
                }

                let added = self.append_targets(&result.targets);
                // Once the user starts typing, preserve the labels they can
                // already see and select. Later batches remain available after
                // the next scan instead of changing codes under their fingers.
                if added && self.input.text().is_empty() {
                    self.relabel();
                }

                if result.status == UiScanStatus::Partial {
                    self.status = None;
                    return if added {
                        self.redraw(ctx).into()
                    } else {
                        CommandBatch::new()
                    };
                }

                let retryable_empty = self.hints.is_empty()
                    && matches!(
                        result.status,
                        UiScanStatus::Success | UiScanStatus::TimedOut
                    )
                    && self.retry_attempt < self.config.scan_retry_count;
                if retryable_empty {
                    self.scanning = true;
                    self.retry_pending = true;
                    self.status = Some(format!(
                        "UI scan is taking longer - retrying {}/{}",
                        self.retry_attempt + 1,
                        self.config.scan_retry_count
                    ));
                    let mut commands = self.status_scene(ctx);
                    commands.push(Command::SetTimer {
                        id: SCAN_RETRY_TIMER_ID.into(),
                        delay: Duration::from_millis(self.config.scan_retry_delay_ms),
                        repeating: false,
                    });
                    return commands.into();
                }

                self.scanning = false;
                self.status = match &result.status {
                    UiScanStatus::Success if self.hints.is_empty() => {
                        Some("No accessible targets — Esc to exit".into())
                    }
                    UiScanStatus::Success => None,
                    UiScanStatus::PermissionDenied(message)
                    | UiScanStatus::Unsupported(message)
                    | UiScanStatus::Failed(message) => Some(format!("{message} — Esc to exit")),
                    UiScanStatus::TimedOut => Some("UI scan timed out — Esc to exit".into()),
                    UiScanStatus::Partial | UiScanStatus::ContextChanged => unreachable!(),
                };
                if self.hints.is_empty() {
                    return self.status_scene(ctx).into();
                }
                self.redraw(ctx)
            }
            ModeEvent::Timer { id, .. }
                if id == SCAN_RETRY_TIMER_ID
                    && self.retry_pending
                    && self.scanning
                    && self.hints.is_empty() =>
            {
                self.retry_scan(ctx)
            }
            ModeEvent::Binding {
                binding,
                state: KeyState::Down,
                ..
            } if matches!(binding.as_ref(), Binding::RescanUi) => self.request_scan(ctx),
            // The tree we labelled belongs to the old window/geometry.
            ModeEvent::FocusChanged(_) | ModeEvent::ScreensChanged(_) if !self.finished => {
                self.request_scan(ctx)
            }
            ModeEvent::PointerMoved(_)
                if !self.finished
                    && self
                        .scan_bounds
                        .is_some_and(|bounds| bounds != ctx.active_bounds()) =>
            {
                self.request_scan(ctx)
            }
            ModeEvent::ScreenRetargeted { screen, .. } => {
                vec![Command::warp_to(screen.bounds.center())]
            }
            ModeEvent::Resumed if self.finished => self.redraw(ctx),
            ModeEvent::Resumed if self.scanning && self.hints.is_empty() => {
                vec![Command::HideOverlay]
            }
            ModeEvent::Resumed if self.hints.is_empty() => self.status_scene(ctx),
            ModeEvent::Resumed => self.redraw(ctx),
            ModeEvent::ConfigReloaded => {
                let return_mode = self.return_mode.clone();
                let scan_id = self.scan_id;
                let scan_bounds = self.scan_bounds;
                let Some(config) = ctx.config.downcast_ref::<Config>() else {
                    return CommandBatch::new();
                };
                *self = Self::new(config);
                self.return_mode = return_mode;
                self.scan_id = scan_id;
                self.scan_bounds = scan_bounds;
                self.request_scan(ctx)
            }
            ModeEvent::Key {
                key,
                state,
                repeat: false,
            } if self.config.overlap_cycle_matches(key) => self.overlap_cycle_key(key, *state, ctx),
            ModeEvent::Key {
                key,
                state: KeyState::Down,
                repeat: false,
            } => self.key_down(key, ctx),
            // In particular, do not treat the trailing repeat from the
            // Primary+F activation chord as an `f` hint selection.
            ModeEvent::Key { repeat: true, .. } => Vec::new(),
            _ => Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::geometry::{Point, Screen};

    struct Env {
        screens: Vec<Screen>,
        cursor: Point,
        palette: Palette,
        config: Config,
    }

    impl Env {
        fn new() -> Self {
            Self::with(Config::default())
        }
        fn with(config: Config) -> Self {
            Self {
                screens: vec![Screen {
                    bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
                    work_area: Rect::new(0.0, 0.0, 1000.0, 800.0),
                    is_primary: true,
                    scale: 1.0,
                    name: None,
                }],
                cursor: Point::new(500.0, 400.0),
                palette: Palette::default(),
                config,
            }
        }
        fn ctx(&self) -> HostContext<'_> {
            HostContext {
                screens: &self.screens,
                cursor: self.cursor,
                focused_app: None,
                palette: &self.palette,
                config: &self.config,
            }
        }
    }

    fn target(name: &str, x: f64) -> UiTarget {
        UiTarget {
            rect: Rect::new(x, 100.0, 80.0, 24.0),
            name: name.into(),
            role: "button".into(),
            native_role: None,
        }
    }

    fn activate(mode: &mut HintMode, env: &Env) -> Vec<Command> {
        mode.handle(&ModeEvent::Activated { previous: None }, &env.ctx())
            .into_iter()
            .collect()
    }

    fn deliver(mode: &mut HintMode, env: &Env, targets: Vec<UiTarget>) -> Vec<Command> {
        mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                id: mode.scan_id,
                targets,
                status: UiScanStatus::Success,
            }),
            &env.ctx(),
        )
        .into_iter()
        .collect()
    }

    fn press(mode: &mut HintMode, env: &Env, name: &str) -> Vec<Command> {
        mode.handle(
            &ModeEvent::Key {
                key: Key::new(name).unwrap(),
                state: KeyState::Down,
                repeat: false,
            },
            &env.ctx(),
        )
        .into_iter()
        .collect()
    }

    fn release(mode: &mut HintMode, env: &Env, name: &str) -> Vec<Command> {
        mode.handle(
            &ModeEvent::Key {
                key: Key::new(name).unwrap(),
                state: KeyState::Up,
                repeat: false,
            },
            &env.ctx(),
        )
        .into_iter()
        .collect()
    }

    fn scene_of<'a>(commands: impl IntoIterator<Item = &'a Command>) -> &'a OverlayScene {
        commands
            .into_iter()
            .find_map(|c| match c {
                Command::ShowOverlay(s) => Some(s),
                _ => None,
            })
            .expect("expected an overlay")
    }

    #[test]
    fn deactivation_releases_large_scan_buffers() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        mode.scanned.reserve(1_024);
        mode.scanned_names_lower.reserve(1_024);
        mode.seen_targets.reserve(1_024);
        mode.hints.reserve(1_024);
        assert!(mode.scanned.capacity() > MAX_IDLE_RETAINED_TARGETS);
        assert!(mode.scanned_names_lower.capacity() > MAX_IDLE_RETAINED_TARGETS);
        assert!(mode.seen_targets.capacity() > MAX_IDLE_RETAINED_TARGETS);
        assert!(mode.hints.capacity() > MAX_IDLE_RETAINED_TARGETS);

        mode.handle(&ModeEvent::Deactivated, &env.ctx());

        assert_eq!(mode.scanned.capacity(), 0);
        assert_eq!(mode.scanned_names_lower.capacity(), 0);
        assert_eq!(mode.seen_targets.capacity(), 0);
        assert_eq!(mode.hints.capacity(), 0);
    }

    #[test]
    fn activation_requests_a_scan_with_configured_roles() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        let out = activate(&mut mode, &env);
        let request = out
            .iter()
            .find_map(|command| match command {
                Command::ScanUi(request) => Some(request),
                _ => None,
            })
            .expect("expected a scan");
        assert_eq!(request.bounds, Some(env.screens[0].bounds));
        assert_eq!(request.timeout_ms, 2_500);
        assert!(request.roles.contains(&"button".to_string()));
        assert_eq!(
            request.strategy,
            crate::api::command::UiScanStrategy::Vision
        );
    }

    #[test]
    fn crossing_displays_restarts_scan_with_the_cursor_screen_bounds() {
        let mut env = Env::new();
        env.screens.push(Screen {
            bounds: Rect::new(1000.0, 0.0, 1200.0, 900.0),
            work_area: Rect::new(1000.0, 0.0, 1200.0, 900.0),
            is_primary: false,
            scale: 2.0,
            name: None,
        });
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        let old_scan_id = mode.scan_id;

        env.cursor = Point::new(1500.0, 300.0);
        let out = mode.handle(&ModeEvent::PointerMoved(env.cursor), &env.ctx());
        let request = out
            .iter()
            .find_map(|command| match command {
                Command::ScanUi(request) => Some(request),
                _ => None,
            })
            .expect("crossing displays should request a new scan");

        assert!(request.id > old_scan_id);
        assert_eq!(request.bounds, Some(env.screens[1].bounds));
        assert_eq!(mode.scan_bounds, Some(env.screens[1].bounds));
    }

    #[test]
    fn scan_results_produce_one_label_per_target() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        let scanning = activate(&mut mode, &env);
        assert!(scanning.contains(&Command::HideOverlay));
        assert!(
            scanning
                .iter()
                .all(|command| !matches!(command, Command::ShowOverlay(_)))
        );
        let out = deliver(
            &mut mode,
            &env,
            vec![target("Save", 0.0), target("Cancel", 200.0)],
        );
        assert_eq!(scene_of(&out).labels.len(), 2);
        assert_eq!(scene_of(&out).clip, Some(env.screens[0].bounds));
    }

    #[test]
    fn partial_scan_batches_appear_immediately_and_remain_after_completion() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);

        let first = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                id: mode.scan_id,
                targets: vec![target("First", 100.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert!(mode.scanning);
        assert_eq!(scene_of(&first).labels.len(), 1);

        let second = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                id: mode.scan_id,
                targets: vec![target("Second", 300.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert_eq!(scene_of(&second).labels.len(), 2);

        let completed = deliver(&mut mode, &env, Vec::new());
        assert!(!mode.scanning);
        assert_eq!(scene_of(&completed).labels.len(), 2);
    }

    #[test]
    fn partial_labels_can_be_selected_before_the_scan_finishes() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                id: mode.scan_id,
                targets: vec![target("First", 100.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        let label = mode.hints[0].label.clone();
        let out = press(&mut mode, &env, &label);
        assert!(
            out.iter()
                .any(|command| matches!(command, Command::WarpPointer { .. }))
        );
    }

    #[test]
    fn scan_results_outside_the_requested_screen_are_discarded() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        let out = deliver(
            &mut mode,
            &env,
            vec![target("Current", 100.0), target("Other display", 1500.0)],
        );

        assert_eq!(mode.scanned.len(), 1);
        assert_eq!(mode.scanned[0].name, "Current");
        assert_eq!(scene_of(&out).labels.len(), 1);
        assert_eq!(scene_of(&out).clip, Some(env.screens[0].bounds));
    }

    #[test]
    fn typed_prefix_removes_other_labels_and_highlights_the_match() {
        let mut config = Config::default();
        config.ui_hint.boundary_highlight.enabled = true;
        let env = Env::with(config);
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        let targets = (0..30)
            .map(|i| target(&format!("target {i}"), i as f64 * 25.0))
            .collect();
        deliver(&mut mode, &env, targets);
        let total = mode.hints.len();
        let prefix = mode
            .alphabet
            .iter()
            .copied()
            .find(|prefix| {
                let prefix = prefix.to_string();
                mode.hints
                    .iter()
                    .filter(|hint| hint.label.starts_with(&prefix))
                    .count()
                    > 1
                    && mode.hints.iter().all(|hint| hint.label != prefix)
            })
            .expect("test data should produce a partial prefix");

        let out = press(&mut mode, &env, &prefix.to_string());
        let scene = scene_of(&out);
        assert!(!scene.labels.is_empty());
        assert!(scene.labels.len() < total);
        assert_eq!(scene.shapes.len(), scene.labels.len());
        assert!(
            scene
                .labels
                .iter()
                .all(|label| { label.text.starts_with(prefix) && label.matched_prefix_len == 1 })
        );
    }

    #[test]
    fn matched_prefix_color_defaults_inside_ui_hint_and_allows_an_override() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        let out = deliver(&mut mode, &env, vec![target("Save", 0.0)]);
        assert_eq!(
            scene_of(&out).labels[0].style.matched_text_color,
            Color::rgb(0xE4, 0xB4, 0x00)
        );

        let mut config = Config::default();
        config.ui_hint.ui.matched_text_color =
            Some(crate::config::ThemedColor::Both("#FF0000FF".to_owned()));
        let env = Env::with(config);
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        let out = deliver(&mut mode, &env, vec![target("Save", 0.0)]);
        assert_eq!(
            scene_of(&out).labels[0].style.matched_text_color,
            Color::rgb(0xFF, 0x00, 0x00)
        );
    }

    #[test]
    fn shift_cycles_overlapping_labels_and_release_restores_default_order() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        let initial = deliver(
            &mut mode,
            &env,
            vec![
                target("One", 100.0),
                target("Two", 100.0),
                target("Three", 100.0),
            ],
        );
        assert!(
            scene_of(&initial)
                .labels
                .iter()
                .all(|label| label.z_index == 2)
        );

        let first = press(&mut mode, &env, "left_shift");
        let first_top = scene_of(&first)
            .labels
            .iter()
            .find(|label| label.z_index == 3)
            .map(|label| label.text.clone())
            .expect("one overlapping label should be raised");

        let restored = release(&mut mode, &env, "left_shift");
        assert!(
            scene_of(&restored)
                .labels
                .iter()
                .all(|label| label.z_index == 2)
        );

        let second = press(&mut mode, &env, "right_shift");
        let second_top = scene_of(&second)
            .labels
            .iter()
            .find(|label| label.z_index == 3)
            .map(|label| label.text.clone())
            .expect("one overlapping label should be raised");
        assert_ne!(first_top, second_top);
    }

    #[test]
    fn configured_overlap_cycle_modifier_replaces_shift() {
        let mut config = Config::default();
        config.ui_hint.overlap_cycle_key = "alt".into();
        let env = Env::with(config);
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![target("One", 100.0), target("Two", 100.0)],
        );

        assert!(press(&mut mode, &env, "left_shift").is_empty());
        let raised = press(&mut mode, &env, "right_alt");
        assert_eq!(
            scene_of(&raised)
                .labels
                .iter()
                .filter(|label| label.z_index == 3)
                .count(),
            1
        );
        let restored = release(&mut mode, &env, "right_alt");
        assert!(
            scene_of(&restored)
                .labels
                .iter()
                .all(|label| label.z_index == 2)
        );
    }

    #[test]
    fn stale_scan_results_are_ignored() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        let out = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                id: mode.scan_id.wrapping_add(1),
                targets: vec![target("stale", 0.0)],
                status: UiScanStatus::Success,
            }),
            &env.ctx(),
        );
        assert!(out.is_empty());
        assert!(mode.scanning);
        assert!(mode.scanned.is_empty());
    }

    #[test]
    fn typing_a_label_moves_and_requests_finish_without_clicking() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![target("Save", 0.0), target("Cancel", 200.0)],
        );

        let label = mode.hints[1].label.clone();
        let expected = mode.scanned[1].rect.center();
        let mut out = Vec::new();
        for ch in label.chars() {
            out = press(&mut mode, &env, &ch.to_string());
        }
        assert!(
            out.iter().any(|c| matches!(
                c,
                Command::WarpPointer { x, y } if *x == expected.x && *y == expected.y
            )),
            "{out:?}"
        );
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::MouseButton { .. }))
        );
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::SwitchMode(_)))
        );
        assert!(out.iter().any(|command| matches!(
            command,
            Command::FinishMode {
                cause: FinishCause::Selection
            }
        )));
    }

    #[test]
    fn empty_scan_stays_visible_and_can_be_retried() {
        let mut config = Config::default();
        config.ui_hint.scan_retry_count = 0;
        let env = Env::with(config);
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        let out = deliver(&mut mode, &env, vec![]);
        assert!(
            scene_of(&out)
                .labels
                .iter()
                .any(|label| label.text.contains("No accessible targets"))
        );
        assert!(
            !press(&mut mode, &env, "r")
                .iter()
                .any(|command| matches!(command, Command::ScanUi(_))),
            "bare r remains available for hint labels"
        );
        let rescanned = mode.handle(
            &ModeEvent::Binding {
                binding: Binding::RescanUi.into(),
                state: KeyState::Down,
                key: Key::new("r").unwrap(),
            },
            &env.ctx(),
        );
        assert!(
            rescanned
                .iter()
                .any(|command| matches!(command, Command::ScanUi(_)))
        );
    }

    #[test]
    fn empty_timeout_retries_with_a_longer_budget() {
        let mut config = Config::default();
        config.ui_hint.scan_timeout_ms = 1_000;
        config.ui_hint.scan_retry_count = 2;
        config.ui_hint.scan_retry_delay_ms = 125;
        let env = Env::with(config);
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        let first_id = mode.scan_id;

        let timed_out = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                id: first_id,
                targets: Vec::new(),
                status: UiScanStatus::TimedOut,
            }),
            &env.ctx(),
        );
        assert!(mode.scanning, "retry wait still captures hint input");
        assert!(timed_out.iter().any(|command| matches!(
            command,
            Command::SetTimer { id, delay, repeating: false }
                if id == SCAN_RETRY_TIMER_ID && *delay == Duration::from_millis(125)
        )));

        let retried = mode.handle(
            &ModeEvent::Timer {
                id: SCAN_RETRY_TIMER_ID.into(),
                elapsed: Duration::from_millis(125),
            },
            &env.ctx(),
        );
        let request = retried
            .iter()
            .find_map(|command| match command {
                Command::ScanUi(request) => Some(request),
                _ => None,
            })
            .expect("retry should submit another scan");
        assert!(request.id > first_id);
        assert_eq!(request.timeout_ms, 2_000);
    }

    #[test]
    fn partial_timeout_keeps_labels_without_retrying() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                id: mode.scan_id,
                targets: vec![target("Ready", 100.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );

        let completed = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                id: mode.scan_id,
                targets: Vec::new(),
                status: UiScanStatus::TimedOut,
            }),
            &env.ctx(),
        );
        assert!(!mode.scanning);
        assert_eq!(scene_of(&completed).labels.len(), 1);
        assert!(
            completed
                .iter()
                .all(|command| !matches!(command, Command::SetTimer { .. }))
        );
    }

    #[test]
    fn scan_failure_status_stays_visible_without_leaving_hint_mode() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        let out = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                id: mode.scan_id,
                targets: Vec::new(),
                status: UiScanStatus::PermissionDenied("Screen Recording required".into()),
            }),
            &env.ctx(),
        );
        assert!(!mode.scanning);
        assert!(
            out.iter()
                .all(|command| !matches!(command, Command::SwitchMode(_)))
        );
        assert!(
            scene_of(&out)
                .labels
                .iter()
                .any(|label| label.text.contains("Screen Recording required"))
        );
    }

    #[test]
    fn activation_key_repeat_does_not_select_an_f_hint() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, vec![target("A", 0.0), target("F", 200.0)]);
        let out = mode.handle(
            &ModeEvent::Key {
                key: Key::new("f").unwrap(),
                state: KeyState::Down,
                repeat: true,
            },
            &env.ctx(),
        );
        assert!(out.is_empty(), "repeat must not select a hint: {out:?}");
        assert!(mode.input.text().is_empty());
    }

    #[test]
    fn keys_are_ignored_while_the_scan_is_in_flight() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        assert!(press(&mut mode, &env, "a").is_empty());
        // Escape still works, so the user is never stuck.
        assert_eq!(press(&mut mode, &env, "esc"), Command::dismiss_to_idle());
    }

    #[test]
    fn slash_opens_search_and_filters_by_name() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![target("Save", 0.0), target("Cancel", 200.0)],
        );

        press(&mut mode, &env, "/");
        let out = press(&mut mode, &env, "c");
        assert_eq!(mode.hints.len(), 1, "only Cancel should survive");
        assert_eq!(mode.scanned[mode.hints[0].value].name, "Cancel");

        // The search box is drawn with the query.
        assert!(
            scene_of(&out).labels.iter().any(|l| l.text == "/c"),
            "search box missing"
        );
    }

    #[test]
    fn escape_leaves_search_before_leaving_the_mode() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, vec![target("Save", 0.0)]);

        press(&mut mode, &env, "/");
        let out = press(&mut mode, &env, "esc");
        assert!(out.iter().any(|c| matches!(c, Command::ShowOverlay(_))));
        assert!(matches!(mode.input, Input::Labels(_)));

        assert_eq!(press(&mut mode, &env, "esc"), Command::dismiss_to_idle());
    }

    #[test]
    fn unmatched_character_is_dropped_and_hints_stay_up() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        // Enough targets that labels are two characters long.
        let targets: Vec<UiTarget> = (0..30)
            .map(|i| target(&format!("b{i}"), i as f64 * 10.0))
            .collect();
        deliver(&mut mode, &env, targets);

        let first = mode.hints[0].label.chars().next().unwrap();
        press(&mut mode, &env, &first.to_string());
        let before = mode.input.text().to_string();

        // A character in the alphabet that cannot follow the current prefix.
        let dead_end = mode
            .alphabet
            .iter()
            .find(|c| {
                !mode
                    .hints
                    .iter()
                    .any(|h| h.label.starts_with(&format!("{before}{c}")))
            })
            .copied();

        if let Some(ch) = dead_end {
            let out = press(&mut mode, &env, &ch.to_string());
            assert!(out.is_empty(), "{out:?}");
            assert_eq!(mode.input.text(), before, "input should be unchanged");
        }
    }

    #[test]
    fn focus_change_triggers_a_fresh_scan() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, vec![target("Save", 0.0)]);

        let out = mode.handle(&ModeEvent::FocusChanged(None), &env.ctx());
        assert!(out.iter().any(|c| matches!(c, Command::ScanUi { .. })));
        assert!(mode.hints.is_empty(), "stale hints must be dropped");
    }

    #[test]
    fn boundary_highlight_adds_outlines_when_enabled() {
        let mut config = Config::default();
        config.ui_hint.boundary_highlight.enabled = true;
        let env = Env::with(config);
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        let out = deliver(
            &mut mode,
            &env,
            vec![target("Save", 0.0), target("Cancel", 200.0)],
        );
        assert_eq!(scene_of(&out).shapes.len(), 2);
    }

    #[test]
    fn placement_config_moves_labels_relative_to_elements() {
        let mut config = Config::default();
        config.ui_hint.placement = crate::api::overlay::Placement::Top;
        let env = Env::with(config);
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        let out = deliver(&mut mode, &env, vec![target("Save", 0.0)]);

        let label = &scene_of(&out).labels[0];
        // Top placement sits above the element's top edge.
        assert!(
            label.rect.bottom() <= 100.0 + f64::EPSILON,
            "{:?}",
            label.rect
        );
    }

    #[test]
    fn configured_label_offsets_adjust_the_placed_hint() {
        let mut config = Config::default();
        config.ui_hint.placement = crate::api::overlay::Placement::Bottom;
        config.ui_hint.label_x_offset = 7;
        config.ui_hint.label_y_offset = -9;
        let env = Env::with(config);
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        let out = deliver(&mut mode, &env, vec![target("Save", 0.0)]);

        let label = &scene_of(&out).labels[0];
        let unshifted = crate::api::overlay::Placement::Bottom.place(
            &Rect::new(0.0, 100.0, 80.0, 24.0),
            label.rect.width,
            label.rect.height,
        );
        assert_eq!(label.rect.x, unshifted.x + 7.0);
        assert_eq!(label.rect.y, unshifted.y - 9.0);
    }

    #[test]
    fn reverse_label_direction_is_honoured() {
        let mut config = Config::default();
        config.ui_hint.label_direction = crate::config::LabelDirection::Reverse;
        config.ui_hint.hint_characters = "asdf".into();
        let env = Env::with(config);
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            (0..5)
                .map(|i| target(&format!("b{i}"), i as f64 * 10.0))
                .collect(),
        );
        let labels: Vec<&str> = mode.hints.iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, ["aa", "sa", "da", "fa", "as"]);
    }

    #[test]
    fn kept_finish_uses_cached_hints_and_backspace_reopens_them() {
        let mut config = Config::default();
        config.ui_hint.lifecycle.after_finish = crate::config::LifecycleAction::Keep;
        let env = Env::with(config);
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, vec![target("Save", 100.0)]);
        let label = mode.hints[0].label.clone();
        let selected = press(&mut mode, &env, &label);
        assert!(selected.iter().any(|command| matches!(
            command,
            Command::FinishMode {
                cause: FinishCause::Selection
            }
        )));

        let finished = mode.handle(
            &ModeEvent::FinishRequested {
                cause: FinishCause::Selection,
            },
            &env.ctx(),
        );
        assert!(mode.finished);
        assert!(
            finished
                .iter()
                .any(|command| matches!(command, Command::ShowOverlay(_)))
        );
        assert!(
            !finished
                .iter()
                .any(|command| matches!(command, Command::ScanUi(_) | Command::SwitchMode(_)))
        );
        assert!(
            mode.handle(
                &ModeEvent::FinishRequested {
                    cause: FinishCause::Explicit,
                },
                &env.ctx(),
            )
            .is_empty()
        );

        let reopened = press(&mut mode, &env, "backspace");
        assert!(!mode.finished);
        assert!(
            reopened
                .iter()
                .any(|command| matches!(command, Command::ShowOverlay(_)))
        );
        assert!(
            !reopened
                .iter()
                .any(|command| matches!(command, Command::ScanUi(_)))
        );
    }

    #[test]
    fn default_finish_returns_to_normal_without_clicking_or_rescanning() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        mode.handle(
            &ModeEvent::Activated {
                previous: Some(ModeId::normal()),
            },
            &env.ctx(),
        );
        let out = mode.handle(
            &ModeEvent::FinishRequested {
                cause: FinishCause::Selection,
            },
            &env.ctx(),
        );
        assert!(out.contains(&Command::SwitchMode(ModeId::normal())));
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::MouseButton { .. } | Command::ScanUi(_)))
        );
    }
}
