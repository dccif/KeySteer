//! Hint mode: label every clickable element and jump to one by typing.
//!
//! On activation the mode asks the platform to walk the accessibility tree.
//! When results arrive each element receives a short label; typing a label
//! warps the pointer to that element and finishes the targeting session.
//!
//! `/` enters search mode, where typing filters elements by their accessible
//! name instead of matching labels.

use std::collections::HashMap;
use std::time::Duration;

use smallvec::SmallVec;

use crate::api::binding::Binding;
use crate::api::command::{
    Command, CommandBatch, FinishCause, HostContext, Mode, ModeEvent, UiScanRequest, UiScanResult,
    UiScanStatus,
};
use crate::api::geometry::{Rect, UiTarget};
use crate::api::input::{Key, KeyChord, KeyState, ModeId};
use crate::api::overlay::{Color, LabelStyle, OverlayLabel, OverlayScene, OverlayShape};
use crate::config::style::AUTO;
use crate::config::{Config, Palette, UiHint as HintsConfig};
use crate::hints::{self, CompactHint, Match};

const SCAN_RETRY_TIMER_ID: &str = "ui_hint.scan_retry";
/// Reuse the small container backing needed by the common UIHint session.
/// Elements and their strings are still dropped on exit; only empty capacity
/// is retained, and larger scans cannot become an Idle high-water mark.
const MAX_IDLE_RETAINED_TARGETS: usize = 128;
const MAX_SCAN_TIMEOUT_MS: u64 = 30_000;
const AUTO_HINT_PADDING_X_RATIO: f64 = 2.0 / 17.0;
const AUTO_HINT_PADDING_Y_RATIO: f64 = 0.06;

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
    overlap_cycle_chord: Option<KeyChord>,

    /// Everything the current scan has returned so far.
    scanned: Vec<UiTarget>,
    scanned_names_lower: Vec<String>,
    search_names_initialized: bool,
    /// Rectangles normally identify one target. A tiny inline collision list
    /// preserves distinct controls sharing bounds without owning a second copy
    /// of every target name and role.
    seen_targets: HashMap<(i64, i64, i64, i64), SmallVec<[usize; 2]>>,
    /// Labelled subset currently on screen; the value is an index into
    /// `scanned`.
    hints: Vec<CompactHint<usize>>,
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
    held_overlap_keys: SmallVec<[Key; 2]>,
    overlap_cycle: usize,
    /// New scan targets are accumulated while the overlap modifier is held,
    /// but the visible labels stay pinned until release. Otherwise every
    /// streamed partial can change the selected member of a growing stack.
    overlap_labels_dirty: bool,
    selected: Option<usize>,
    finished: bool,
}

impl HintMode {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.ui_hint.clone(),
            alphabet: config.ui_hint.hint_characters.chars().collect(),
            overlap_cycle_chord: KeyChord::parse(&config.ui_hint.overlap_cycle_key).ok(),
            scanned: Vec::new(),
            scanned_names_lower: Vec::new(),
            search_names_initialized: false,
            seen_targets: HashMap::new(),
            hints: Vec::new(),
            input: Input::Labels(String::new()),
            scanning: false,
            status: None,
            return_mode: ModeId::idle(),
            scan_id: 0,
            retry_attempt: 0,
            retry_pending: false,
            scan_bounds: None,
            held_overlap_keys: SmallVec::new(),
            overlap_cycle: 0,
            overlap_labels_dirty: false,
            selected: None,
            finished: false,
        }
    }

    fn clear_scan_results(&mut self, release_large_buffers: bool) {
        self.scanned.clear();
        self.scanned_names_lower.clear();
        self.search_names_initialized = false;
        self.seen_targets.clear();
        self.hints.clear();
        self.overlap_labels_dirty = false;
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
                self.seen_targets = HashMap::new();
            }
        }
    }

    fn resolved_hint_label_style(&self, palette: &Palette) -> LabelStyle {
        let mut style = self.config.ui.resolve(
            palette,
            palette.surface_label(),
            palette.text,
            palette.accent,
        );
        // UI Hint labels are deliberately denser than larger grid cells and
        // badges. Keep -1 as a font-relative auto value without changing the
        // shared LabelUi auto rules used by those other components.
        if self.config.ui.padding_x == AUTO {
            style.padding_x = (style.font_size * AUTO_HINT_PADDING_X_RATIO).round();
        }
        if self.config.ui.padding_y == AUTO {
            style.padding_y = (style.font_size * AUTO_HINT_PADDING_Y_RATIO).round();
        }
        style
    }

    fn request_scan(&mut self, ctx: &HostContext<'_>) -> CommandBatch {
        self.scanning = true;
        self.status = None;
        self.clear_scan_results(false);
        self.input = Input::Labels(String::new());
        self.selected = None;
        self.finished = false;
        self.retry_attempt = 0;
        self.retry_pending = false;
        let request = self.scan_request(ctx);
        let mut commands = CommandBatch::two(
            Command::CancelTimer {
                id: SCAN_RETRY_TIMER_ID.into(),
            },
            Command::HideOverlay,
        );
        commands.push(request);
        commands
    }

    fn retry_scan(&mut self, ctx: &HostContext<'_>) -> CommandBatch {
        self.retry_attempt = self.retry_attempt.saturating_add(1);
        self.retry_pending = false;
        self.scanning = true;
        CommandBatch::one(self.scan_request(ctx))
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

    fn target_key(target: &UiTarget) -> (i64, i64, i64, i64) {
        let rect = target.rect;
        (
            (rect.x * 4.0).round() as i64,
            (rect.y * 4.0).round() as i64,
            (rect.width * 4.0).round() as i64,
            (rect.height * 4.0).round() as i64,
        )
    }

    fn append_target(&mut self, target: UiTarget) -> bool {
        if !self
            .scan_bounds
            .is_none_or(|bounds| bounds.contains(&target.rect.center()))
        {
            return false;
        }
        let key = Self::target_key(&target);
        let duplicate = self.seen_targets.get(&key).is_some_and(|indices| {
            indices.iter().any(|&index| {
                let existing = &self.scanned[index];
                existing.name == target.name && existing.role == target.role
            })
        });
        if duplicate {
            return false;
        }
        let index = self.scanned.len();
        if self.search_names_initialized {
            self.scanned_names_lower.push(target.name.to_lowercase());
        }
        self.scanned.push(target);
        self.seen_targets.entry(key).or_default().push(index);
        true
    }

    fn append_targets_owned(&mut self, targets: Vec<UiTarget>) -> bool {
        let before = self.scanned.len();

        // On the first small result, take ownership of the platform Vec so the
        // common 24/64/100-target path does not allocate a second backing
        // buffer. A retained session buffer is already cheaper to fill by move.
        if self.scanned.is_empty()
            && self.scanned.capacity() == 0
            && self.seen_targets.is_empty()
            && !self.search_names_initialized
            && targets.len() <= MAX_IDLE_RETAINED_TARGETS
        {
            self.scanned = targets;
            self.scanned.retain(|target| {
                self.scan_bounds
                    .is_none_or(|bounds| bounds.contains(&target.rect.center()))
            });
            self.seen_targets.reserve(self.scanned.len());

            let mut index = 0;
            while index < self.scanned.len() {
                let key = Self::target_key(&self.scanned[index]);
                let duplicate = self.seen_targets.get(&key).is_some_and(|indices| {
                    indices.iter().any(|&existing_index| {
                        let existing = &self.scanned[existing_index];
                        let candidate = &self.scanned[index];
                        existing.name == candidate.name && existing.role == candidate.role
                    })
                });
                if duplicate {
                    self.scanned.remove(index);
                } else {
                    self.seen_targets.entry(key).or_default().push(index);
                    index += 1;
                }
            }
            return !self.scanned.is_empty();
        }

        let incoming = targets.len();
        self.scanned.reserve(incoming);
        self.seen_targets.reserve(incoming);
        if self.search_names_initialized {
            self.scanned_names_lower.reserve(incoming);
        }
        for target in targets {
            self.append_target(target);
        }
        self.scanned.len() != before
    }

    fn handle_scan_result(&mut self, result: UiScanResult, ctx: &HostContext<'_>) -> CommandBatch {
        if self.finished || result.id != self.scan_id {
            return CommandBatch::new();
        }
        let UiScanResult {
            targets, status, ..
        } = result;
        if status == UiScanStatus::ContextChanged {
            self.scanning = false;
            self.clear_scan_results(false);
            self.status = Some("Focused window changed — Esc to exit".into());
            return self.status_scene(ctx);
        }

        let added = self.append_targets_owned(targets);
        // Once the user starts typing, preserve the labels they can already
        // see and select. Later batches remain available after the next scan
        // instead of changing codes under their fingers.
        let labels_changed = if added && self.input.text().is_empty() {
            if self.held_overlap_keys.is_empty() || self.hints.is_empty() {
                self.relabel();
                true
            } else {
                // Keep the currently raised label stable while the overlap key
                // is held. The targets are labelled together on final release.
                self.overlap_labels_dirty = true;
                false
            }
        } else {
            false
        };

        if status == UiScanStatus::Partial {
            self.status = None;
            return if labels_changed {
                self.redraw(ctx)
            } else {
                CommandBatch::new()
            };
        }

        let retryable_empty = self.hints.is_empty()
            && matches!(status, UiScanStatus::Success | UiScanStatus::TimedOut)
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
            return commands;
        }

        if status == UiScanStatus::Success
            && !labels_changed
            && !self.hints.is_empty()
            && self.status.is_none()
        {
            self.scanning = false;
            return CommandBatch::new();
        }

        self.scanning = false;
        self.status = match &status {
            UiScanStatus::Success if self.hints.is_empty() => {
                Some("No accessible targets — Esc to exit".into())
            }
            UiScanStatus::Success => None,
            UiScanStatus::PermissionDenied(message)
            | UiScanStatus::Unsupported(message)
            | UiScanStatus::Failed(message) => Some(format!("{message} — Esc to exit")),
            UiScanStatus::TimedOut => Some("UI scan timed out — Esc to exit".into()),
            UiScanStatus::Partial => self.status.clone(),
            UiScanStatus::ContextChanged => Some("Focused window changed - Esc to exit".into()),
        };
        if self.hints.is_empty() {
            return self.status_scene(ctx);
        }
        self.redraw(ctx)
    }

    /// Assign labels to the targets matching the current search query.
    fn relabel(&mut self) {
        let query = match &self.input {
            // Key names are normalized to lowercase before Mode delivery, so
            // the accumulated search query is already canonical.
            Input::Search(query) => Some(query.as_str()),
            Input::Labels(_) => None,
        };
        if query.is_some() && !self.search_names_initialized {
            self.scanned_names_lower.clear();
            self.scanned_names_lower
                .extend(self.scanned.iter().map(|target| target.name.to_lowercase()));
            self.search_names_initialized = true;
        }

        let candidates = self
            .scanned
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                query.is_none_or(|query| self.scanned_names_lower[*index].contains(query))
            })
            .map(|(index, target)| (target.rect, index));

        if hints::assign_compact_into(
            &mut self.hints,
            candidates,
            &self.alphabet,
            self.config.label_direction,
        )
        .is_err()
        {
            self.hints.clear();
        }
        self.overlap_labels_dirty = false;
    }

    fn hint_is_visible(&self, hint: &CompactHint<usize>) -> bool {
        match &self.input {
            Input::Labels(typed) => typed.is_empty() || hint.label.as_str().starts_with(typed),
            Input::Search(_) => true,
        }
    }

    fn overlap_cycle_key(
        &mut self,
        key: &Key,
        state: KeyState,
        ctx: &HostContext<'_>,
    ) -> CommandBatch {
        let changed = match state {
            KeyState::Down => {
                let was_released = self.held_overlap_keys.is_empty();
                let inserted = if self.held_overlap_keys.contains(key) {
                    false
                } else {
                    self.held_overlap_keys.push(key.clone());
                    true
                };
                if inserted && was_released {
                    self.overlap_cycle = self.overlap_cycle.wrapping_add(1);
                }
                inserted
            }
            KeyState::Up => self
                .held_overlap_keys
                .iter()
                .position(|candidate| candidate == key)
                .map(|index| self.held_overlap_keys.swap_remove(index))
                .is_some(),
        };
        if changed
            && state == KeyState::Up
            && self.held_overlap_keys.is_empty()
            && self.overlap_labels_dirty
            && self.input.text().is_empty()
        {
            self.relabel();
        }
        if changed && !self.hints.is_empty() {
            self.redraw(ctx)
        } else {
            CommandBatch::new()
        }
    }

    fn scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
        let palette = ctx.palette;
        let placement = self.config.placement;
        let visible_count = self
            .hints
            .iter()
            .filter(|hint| self.hint_is_visible(hint))
            .count();
        let shape_capacity = if self.config.boundary_highlight.enabled {
            visible_count
        } else {
            0
        };
        let label_capacity = visible_count + usize::from(matches!(&self.input, Input::Search(_)));
        let mut scene = OverlayScene::with_capacity(shape_capacity, label_capacity);
        scene.clip = self.scan_bounds.or_else(|| Some(ctx.active_bounds()));

        let mut label_style = self.resolved_hint_label_style(palette);
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
        let matched_prefix_len = typed.chars().count();
        for hint in self.hints.iter().filter(|hint| self.hint_is_visible(hint)) {
            let width = label_style.font_size * 0.75 * hint.label.as_str().chars().count() as f64
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
                OverlayLabel::new(hint.label.as_str(), rect, label_style.clone())
                    .with_matched_prefix(matched_prefix_len)
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

    fn redraw(&self, ctx: &HostContext<'_>) -> CommandBatch {
        CommandBatch::one(Command::show_overlay(if self.finished {
            self.finished_scene(ctx)
        } else {
            self.scene(ctx)
        }))
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

    fn status_scene(&self, ctx: &HostContext<'_>) -> CommandBatch {
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
        CommandBatch::one(Command::show_overlay(scene))
    }

    fn select(&mut self, index: usize) -> CommandBatch {
        let Some(target) = self.scanned.get(index) else {
            return self.cancel();
        };
        let point = target.rect.center();
        self.selected = Some(index);
        CommandBatch::two(
            Command::warp_to(point),
            Command::FinishMode {
                cause: FinishCause::Selection,
            },
        )
    }

    fn cancel(&self) -> CommandBatch {
        CommandBatch::two(
            Command::HideOverlay,
            Command::SwitchMode(self.return_mode.clone()),
        )
    }

    fn key_down(&mut self, key: &Key, ctx: &HostContext<'_>) -> CommandBatch {
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
                _ => CommandBatch::new(),
            };
        }
        // Before the first batch arrives only allow bailing out. Once partial
        // labels exist they are immediately usable while scanning continues.
        if self.scanning && self.hints.is_empty() {
            return if key.as_str() == "esc" {
                self.cancel()
            } else {
                CommandBatch::new()
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
            return CommandBatch::new();
        };

        match &mut self.input {
            Input::Search(query) => {
                if ch.is_alphanumeric() || ch == ' ' {
                    query.push(ch);
                    self.relabel();
                    return self.redraw(ctx);
                }
                CommandBatch::new()
            }
            Input::Labels(typed) => {
                if !self.alphabet.contains(&ch) {
                    return CommandBatch::new();
                }
                typed.push(ch);
                match match_compact_input(&self.hints, typed) {
                    Match::Complete(index) => self.select(index),
                    Match::Partial { .. } => self.redraw(ctx),
                    // Dead end: drop the character and keep the hints up.
                    Match::None => {
                        if let Input::Labels(typed) = &mut self.input {
                            typed.pop();
                        }
                        CommandBatch::new()
                    }
                }
            }
        }
    }
}

fn match_compact_input(hints: &[CompactHint<usize>], input: &str) -> Match<usize> {
    if input.is_empty() {
        return Match::Partial {
            remaining: hints.len(),
        };
    }
    if let Some(hit) = hints.iter().find(|hint| hint.label.as_str() == input) {
        return Match::Complete(hit.value);
    }
    let remaining = hints
        .iter()
        .filter(|hint| hint.label.as_str().starts_with(input))
        .count();
    if remaining == 0 {
        Match::None
    } else {
        Match::Partial { remaining }
    }
}

/// Greedily assign every intersecting label to a global, non-overlapping
/// display layer. While the configured cycle modifier is held, every label in
/// one layer is raised together. The stable default order is untouched, so
/// releasing it restores that order.
fn rotate_overlapping_labels(labels: &mut [OverlayLabel], cycle: usize) {
    let count = labels.len();
    let mut order: SmallVec<[usize; 128]> = (0..count).collect();
    order.sort_unstable_by(|left, right| {
        labels[*left]
            .rect
            .left()
            .total_cmp(&labels[*right].rect.left())
            .then_with(|| {
                labels[*left]
                    .rect
                    .top()
                    .total_cmp(&labels[*right].rect.top())
            })
            .then_with(|| left.cmp(right))
    });

    let mut layers: SmallVec<[usize; 128]> = std::iter::repeat_n(0, count).collect();
    let mut overlaps: SmallVec<[bool; 128]> = std::iter::repeat_n(false, count).collect();
    let mut layer_marks: SmallVec<[usize; 128]> = SmallVec::new();
    let mut active: SmallVec<[usize; 128]> = SmallVec::new();
    let mut layer_count = 0usize;
    let mut stamp = 0usize;

    // Sweep from left to right. Active rectangles are the only possible
    // conflicts. Generation marks find the smallest free layer without
    // clearing a scratch bitset for every label.
    for index in order {
        let left = labels[index].rect.left();
        active.retain(|other| labels[*other].rect.right() >= left);
        stamp = stamp.wrapping_add(1);
        if stamp == 0 {
            layer_marks.fill(0);
            stamp = 1;
        }
        for &other in &active {
            if labels[index].rect.intersect(&labels[other].rect).is_none() {
                continue;
            }
            overlaps[index] = true;
            overlaps[other] = true;
            let occupied = layers[other];
            if layer_marks.len() <= occupied {
                layer_marks.resize(occupied + 1, 0);
            }
            layer_marks[occupied] = stamp;
        }
        let layer = layer_marks
            .iter()
            .position(|marked| *marked != stamp)
            .unwrap_or(layer_marks.len());
        layers[index] = layer;
        layer_count = layer_count.max(layer + 1);
        active.push(index);
    }

    if !overlaps.iter().any(|overlap| *overlap) {
        return;
    }
    let selected_layer = (cycle + layer_count - 1) % layer_count;
    for (index, label) in labels.iter_mut().enumerate() {
        if overlaps[index] && layers[index] == selected_layer {
            label.z_index = 3;
        }
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
        match event {
            ModeEvent::Activated { previous } => {
                self.return_mode = previous.clone().unwrap_or_else(ModeId::idle);
                self.request_scan(ctx)
            }
            ModeEvent::Restarted => self.request_scan(ctx),
            ModeEvent::FinishRequested { .. } if self.finished => CommandBatch::new(),
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
                CommandBatch::one(Command::CancelTimer {
                    id: SCAN_RETRY_TIMER_ID.into(),
                })
            }
            ModeEvent::UiScanned(result) if self.finished || result.id != self.scan_id => {
                CommandBatch::new()
            }
            ModeEvent::UiScanned(result) => self.handle_scan_result(result.clone(), ctx),
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
                CommandBatch::one(Command::warp_to(screen.bounds.center()))
            }
            ModeEvent::Resumed if self.finished => self.redraw(ctx),
            ModeEvent::Resumed if self.scanning && self.hints.is_empty() => {
                CommandBatch::one(Command::HideOverlay)
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
            } if self
                .overlap_cycle_chord
                .as_ref()
                .is_some_and(|chord| chord.activation_matches(key)) =>
            {
                self.overlap_cycle_key(key, *state, ctx)
            }
            ModeEvent::Key {
                key,
                state: KeyState::Down,
                repeat: false,
            } => self.key_down(key, ctx),
            // In particular, do not treat the trailing repeat from the
            // Primary+F activation chord as an `f` hint selection.
            ModeEvent::Key { repeat: true, .. } => CommandBatch::new(),
            _ => CommandBatch::new(),
        }
    }

    fn handle_owned(&mut self, event: ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        match event {
            ModeEvent::UiScanned(result) => self.handle_scan_result(result, ctx),
            event => self.handle(&event, ctx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::geometry::{Point, Screen};
    use std::collections::HashSet;

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
        mode.handle_owned(
            ModeEvent::UiScanned(crate::api::UiScanResult {
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
    fn default_auto_padding_resolves_to_compact_hint_spacing() {
        let env = Env::new();
        let mode = HintMode::new(&env.config);
        let style = mode.resolved_hint_label_style(&env.palette);

        assert_eq!(mode.config.ui.padding_x, AUTO);
        assert_eq!(mode.config.ui.padding_y, AUTO);
        assert_eq!(style.font_size, 17.0);
        assert_eq!(style.padding_x, 2.0);
        assert_eq!(style.padding_y, 1.0);
    }

    #[test]
    fn explicit_hint_padding_still_overrides_compact_auto_spacing() {
        let mut config = Config::default();
        config.ui_hint.ui.padding_x = 7;
        config.ui_hint.ui.padding_y = 3;
        let env = Env::with(config);
        let mode = HintMode::new(&env.config);
        let style = mode.resolved_hint_label_style(&env.palette);

        assert_eq!(style.padding_x, 7.0);
        assert_eq!(style.padding_y, 3.0);
    }

    #[test]
    fn deactivation_releases_large_scan_buffers() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        mode.scanned.reserve(1_024);
        mode.scanned_names_lower.reserve(1_024);
        mode.seen_targets.reserve(1_024);
        mode.hints.reserve(1_024);
        assert!(mode.scanned.capacity() >= 1_024);
        assert!(mode.scanned_names_lower.capacity() >= 1_024);
        assert!(mode.seen_targets.capacity() >= 1_024);
        assert!(mode.hints.capacity() >= 1_024);

        mode.handle(&ModeEvent::Deactivated, &env.ctx());

        assert_eq!(mode.scanned.capacity(), 0);
        assert_eq!(mode.scanned_names_lower.capacity(), 0);
        assert_eq!(mode.seen_targets.capacity(), 0);
        assert_eq!(mode.hints.capacity(), 0);
    }

    #[test]
    fn deactivation_reuses_only_small_empty_container_capacity() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            (0..100)
                .map(|index| target(&format!("Target {index}"), index as f64 * 2.0))
                .collect(),
        );
        assert_eq!(mode.scanned.len(), 100);

        mode.handle(&ModeEvent::Deactivated, &env.ctx());

        assert!(mode.scanned.is_empty());
        assert!(mode.scanned.capacity() >= 100);
        assert_eq!(mode.scanned_names_lower.capacity(), 0);
        assert!(mode.seen_targets.is_empty());
        assert!(mode.seen_targets.capacity() >= 100);
        assert!(mode.hints.is_empty());
        assert!(mode.hints.capacity() >= 100);
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
            crate::api::command::UiScanStrategy::Hybrid
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
        assert!(
            completed.is_empty(),
            "an unchanged terminal must not redraw"
        );
        assert_eq!(mode.hints.len(), 2);
    }

    #[test]
    #[ignore = "microbenchmark probe; run in release with --test-threads=1"]
    fn unchanged_terminal_performance_probe() {
        const WARMUP: usize = 2_000;
        const SAMPLES: usize = 20_000;

        fn prepared(env: &Env) -> HintMode {
            let mut mode = HintMode::new(&env.config);
            activate(&mut mode, env);
            let scan_id = mode.scan_id;
            mode.handle_owned(
                ModeEvent::UiScanned(UiScanResult {
                    id: scan_id,
                    targets: vec![target("Save", 0.0), target("Cancel", 200.0)],
                    status: UiScanStatus::Partial,
                }),
                &env.ctx(),
            );
            mode
        }

        fn measure(mut operation: impl FnMut()) -> (u128, u128, u128) {
            for _ in 0..WARMUP {
                operation();
            }
            let mut samples = Vec::with_capacity(SAMPLES);
            for _ in 0..SAMPLES {
                let started = std::time::Instant::now();
                operation();
                samples.push(started.elapsed().as_nanos());
            }
            samples.sort_unstable();
            let last = samples.len() - 1;
            (
                samples[last * 50 / 100],
                samples[last * 95 / 100],
                samples[last * 99 / 100],
            )
        }

        let env = Env::new();
        let mut fast = prepared(&env);
        let scan_id = fast.scan_id;
        let no_op = measure(|| {
            std::hint::black_box(fast.handle_owned(
                ModeEvent::UiScanned(UiScanResult {
                    id: scan_id,
                    targets: Vec::new(),
                    status: UiScanStatus::Success,
                }),
                &env.ctx(),
            ));
        });
        let legacy = prepared(&env);
        let redraw = measure(|| {
            std::hint::black_box(legacy.redraw(&env.ctx()));
        });
        println!(
            "hint_terminal_probe samples={SAMPLES} no_op_p50={}ns no_op_p95={}ns no_op_p99={}ns redraw_p50={}ns redraw_p95={}ns redraw_p99={}ns",
            no_op.0, no_op.1, no_op.2, redraw.0, redraw.1, redraw.2,
        );
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

        release(&mut mode, &env, "right_shift");
        let third = press(&mut mode, &env, "left_shift");
        let third_top = scene_of(&third)
            .labels
            .iter()
            .find(|label| label.z_index == 3)
            .map(|label| label.text.clone())
            .expect("one overlapping label should be raised");
        assert_ne!(first_top, third_top);
        assert_ne!(second_top, third_top);
    }

    #[test]
    fn overlap_cycle_raises_one_global_non_intersecting_layer() {
        let label = |text: &str, rect: Rect| {
            OverlayLabel::new(text, rect, LabelStyle::default()).with_z_index(2)
        };
        let labels = vec![
            label("a", Rect::new(0.0, 0.0, 20.0, 20.0)),
            label("b", Rect::new(0.0, 0.0, 20.0, 20.0)),
            label("c", Rect::new(100.0, 0.0, 20.0, 20.0)),
            label("d", Rect::new(100.0, 0.0, 20.0, 20.0)),
        ];

        let mut first = labels.clone();
        rotate_overlapping_labels(&mut first, 1);
        assert_eq!(
            first
                .iter()
                .filter(|label| label.z_index == 3)
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>(),
            ["a", "c"]
        );

        let mut second = labels;
        rotate_overlapping_labels(&mut second, 2);
        assert_eq!(
            second
                .iter()
                .filter(|label| label.z_index == 3)
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>(),
            ["b", "d"]
        );
    }

    #[test]
    fn overlap_layers_reuse_space_for_non_intersecting_chain_members() {
        let mut labels = vec![
            OverlayLabel::new(
                "left",
                Rect::new(0.0, 0.0, 10.0, 10.0),
                LabelStyle::default(),
            )
            .with_z_index(2),
            OverlayLabel::new(
                "middle",
                Rect::new(8.0, 0.0, 10.0, 10.0),
                LabelStyle::default(),
            )
            .with_z_index(2),
            OverlayLabel::new(
                "right",
                Rect::new(16.0, 0.0, 10.0, 10.0),
                LabelStyle::default(),
            )
            .with_z_index(2),
        ];

        rotate_overlapping_labels(&mut labels, 1);
        assert_eq!(
            labels
                .iter()
                .filter(|label| label.z_index == 3)
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>(),
            ["left", "right"]
        );
    }

    #[test]
    fn shallow_groups_do_not_wrap_into_a_deeper_global_layer() {
        let label = |text: &str, x: f64| {
            OverlayLabel::new(text, Rect::new(x, 0.0, 20.0, 20.0), LabelStyle::default())
                .with_z_index(2)
        };
        let mut labels = vec![
            label("two-0", 0.0),
            label("two-1", 0.0),
            label("three-0", 100.0),
            label("three-1", 100.0),
            label("three-2", 100.0),
        ];

        rotate_overlapping_labels(&mut labels, 3);
        assert_eq!(
            labels
                .iter()
                .filter(|label| label.z_index == 3)
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>(),
            ["three-2"]
        );
    }

    #[test]
    fn held_overlap_key_pins_streamed_stack_until_release() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                id: mode.scan_id,
                targets: vec![target("One", 100.0), target("Two", 100.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );

        let raised = press(&mut mode, &env, "left_shift");
        let raised_text = scene_of(&raised)
            .labels
            .iter()
            .find(|label| label.z_index == 3)
            .map(|label| label.text.clone())
            .expect("one overlapping label should be raised immediately");

        let streamed = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                id: mode.scan_id,
                targets: vec![target("Three", 100.0), target("Four", 100.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert!(
            streamed.is_empty(),
            "a held stack must not redraw per partial"
        );
        assert_eq!(mode.hints.len(), 2, "visible labels stay pinned while held");
        assert_eq!(
            mode.scene(&env.ctx())
                .labels
                .iter()
                .find(|label| label.z_index == 3)
                .map(|label| label.text.as_str()),
            Some(raised_text.as_str())
        );

        let released = release(&mut mode, &env, "left_shift");
        assert_eq!(scene_of(&released).labels.len(), 4);
        assert!(!mode.overlap_labels_dirty);

        let mut raised_labels = HashSet::new();
        for key in ["left_shift", "right_shift", "left_shift", "right_shift"] {
            let cycled = press(&mut mode, &env, key);
            raised_labels.insert(
                scene_of(&cycled)
                    .labels
                    .iter()
                    .find(|label| label.z_index == 3)
                    .map(|label| label.text.clone())
                    .expect("one label should be raised"),
            );
            release(&mut mode, &env, key);
        }
        assert_eq!(
            raised_labels.len(),
            4,
            "every stacked label must be reachable"
        );
    }

    #[test]
    fn repeated_overlap_down_does_not_cycle_again_while_held() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![target("One", 100.0), target("Two", 100.0)],
        );

        let first = press(&mut mode, &env, "left_shift");
        let first_top = scene_of(&first)
            .labels
            .iter()
            .find(|label| label.z_index == 3)
            .map(|label| label.text.clone());
        let cycle = mode.overlap_cycle;

        assert!(press(&mut mode, &env, "left_shift").is_empty());
        assert!(
            mode.handle(
                &ModeEvent::Key {
                    key: Key::new("left_shift").unwrap(),
                    state: KeyState::Down,
                    repeat: true,
                },
                &env.ctx(),
            )
            .is_empty()
        );
        assert_eq!(mode.overlap_cycle, cycle);
        assert_eq!(
            mode.scene(&env.ctx())
                .labels
                .iter()
                .find(|label| label.z_index == 3)
                .map(|label| label.text.clone()),
            first_top
        );
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
        assert!(!mode.search_names_initialized);
        assert!(mode.scanned_names_lower.is_empty());

        press(&mut mode, &env, "/");
        assert!(mode.search_names_initialized);
        assert_eq!(mode.scanned_names_lower, ["save", "cancel"]);
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
    fn unicode_search_uses_the_normalized_key_without_changing_matches() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![target("École", 0.0), target("Cancel", 200.0)],
        );

        press(&mut mode, &env, "/");
        press(&mut mode, &env, "É");

        assert_eq!(mode.input.text(), "é");
        assert_eq!(mode.hints.len(), 1);
        assert_eq!(mode.scanned[mode.hints[0].value].name, "École");
    }

    #[test]
    #[ignore = "microbenchmark probe; run in release with --test-threads=1"]
    fn normalized_search_query_performance_probe() {
        const WARMUP: usize = 2_000;
        const SAMPLES: usize = 20_000;
        const CALLS_PER_SAMPLE: usize = 100;

        fn measure(mut operation: impl FnMut()) -> (u128, u128, u128) {
            for _ in 0..WARMUP {
                operation();
            }
            let mut samples = Vec::with_capacity(SAMPLES);
            for _ in 0..SAMPLES {
                let started = std::time::Instant::now();
                for _ in 0..CALLS_PER_SAMPLE {
                    operation();
                }
                samples.push(started.elapsed().as_nanos() / CALLS_PER_SAMPLE as u128);
            }
            samples.sort_unstable();
            let last = samples.len() - 1;
            (
                samples[last * 50 / 100],
                samples[last * 95 / 100],
                samples[last * 99 / 100],
            )
        }

        let query = String::from("école");
        let borrowed = measure(|| {
            std::hint::black_box(query.as_str());
        });
        let normalized_again = measure(|| {
            std::hint::black_box(query.to_lowercase());
        });
        println!(
            "hint_search_query_probe samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} borrowed_p50={}ns borrowed_p95={}ns borrowed_p99={}ns normalized_again_p50={}ns normalized_again_p95={}ns normalized_again_p99={}ns",
            borrowed.0,
            borrowed.1,
            borrowed.2,
            normalized_again.0,
            normalized_again.1,
            normalized_again.2,
        );
    }

    #[test]
    fn spatial_dedup_reuses_canonical_strings_but_keeps_real_collisions() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);

        let save = target("Save", 0.0);
        let mut different_name = save.clone();
        different_name.name = "Save as".into();
        let mut different_role = save.clone();
        different_role.role = "checkbox".into();
        deliver(
            &mut mode,
            &env,
            vec![save.clone(), save, different_name, different_role],
        );

        assert_eq!(mode.scanned.len(), 3);
        assert_eq!(mode.seen_targets.len(), 1);
        assert_eq!(mode.seen_targets.values().next().unwrap().len(), 3);
        assert!(mode.scanned_names_lower.is_empty());
    }

    #[test]
    fn owned_first_partial_adopts_target_and_string_storage() {
        let env = Env::new();
        let mut mode = HintMode::new(&env.config);
        activate(&mut mode, &env);

        let targets = vec![target("Save 设置", 20.0), target("Cancel", 120.0)];
        let targets_ptr = targets.as_ptr();
        let name_ptr = targets[0].name.as_ptr();
        let scan_id = mode.scan_id;
        let _ = mode.handle_owned(
            ModeEvent::UiScanned(UiScanResult {
                id: scan_id,
                targets,
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );

        assert_eq!(mode.scanned.as_ptr(), targets_ptr);
        assert_eq!(mode.scanned[0].name.as_ptr(), name_ptr);
        assert_eq!(
            mode.scanned
                .iter()
                .map(|target| target.name.as_str())
                .collect::<Vec<_>>(),
            ["Save 设置", "Cancel"]
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
