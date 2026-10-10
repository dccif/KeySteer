//! Hint mode: label every clickable element and jump to one by typing.
//!
//! On activation the mode asks the platform to walk the accessibility tree.
//! When results arrive each element receives a short label; typing a label
//! warps the pointer to that element and finishes the targeting session.
//!
//! `/` enters search mode, where typing filters elements by their accessible
//! name instead of matching labels.

use std::time::Duration;

use smallvec::SmallVec;

use crate::api::binding::Binding;
use crate::api::command::{
    Command, CommandBatch, FinishCause, FocusedApp, HostContext, Mode, ModeEvent, UiScanRequest,
    UiScanResult, UiScanStatus, UiScanStrategy, VisionOptions,
};
use crate::api::geometry::Rect;
#[cfg(test)]
use crate::api::geometry::{SemanticRole, UiTarget};
use crate::api::hint::LabelDirection;
use crate::api::input::{Key, KeyChord, KeyState, ModeId};
use crate::api::lifecycle::TargetingLifecycle;
use crate::api::overlay::{Color, OverlayText};
use crate::api::style::compiled::{BoundaryHighlight, LabelUi};
use crate::api::style::{CompiledSearchPanel, HintPlacement};
use crate::api::theme::Palette;
pub(crate) mod labeling;
mod point;
mod search;
pub use point::PointSettings;
mod session;

use crate::api::presentation::{
    HintContent, HintSelectionView, HintStyle, HintView, StatusView, View, VisualLayerPlan,
};
use labeling::{self as hints, CompactHint};
use session::ScanSession;

#[derive(Debug, Clone, PartialEq)]
enum Match<T> {
    Complete(T),
    Partial { remaining: usize },
    None,
}

const SCAN_RETRY_TIMER_ID: &str = "ui_hint.scan_retry";
const SEARCH_PREWARM_TIMER_ID: &str = "ui_hint.search_prewarm";
// A work quantum, not a scan batch threshold or a delay. Rearming yields to
// input and native event polling between chunks, including large scans.
const SEARCH_PREWARM_TARGETS: usize = 256;
const NO_WINDOW_UNDER_POINTER: &str =
    "No window under the pointer — move the pointer over a window";
use crate::api::presentation::hint_cache::INLINE_LABELS;
const MAX_INLINE_TARGETS: usize = INLINE_LABELS;
const MAX_SCAN_TIMEOUT_MS: u64 = 30_000;

/// A missing cursor starts at the first target; the last target wraps to it.
fn next_target_position(
    current: Option<usize>,
    first: Option<usize>,
    next: impl FnOnce(usize) -> usize,
) -> Option<usize> {
    first.map(|first| current.map_or(first, next))
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppStrategyOverride {
    pub pattern: String,
    pub strategy: Option<UiScanStrategy>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub strategy: UiScanStrategy,
    pub scan_scope: crate::api::UiScanScope,
    pub vision: VisionOptions,
    pub hint_characters: String,
    pub label_direction: LabelDirection,
    pub max_depth: u32,
    pub scan_timeout_ms: u64,
    pub scan_retry_count: u32,
    pub scan_retry_delay_ms: u64,
    pub clickable_roles: Vec<String>,
    pub ignore_clickable_check: bool,
    pub visible_check_enabled: bool,
    pub placement: HintPlacement,
    pub label_x_offset: i32,
    pub label_y_offset: i32,
    pub ui: LabelUi,
    pub boundary_highlight: BoundaryHighlight,
    pub search_input_ui: CompiledSearchPanel,
    pub search_info_ui: CompiledSearchPanel,
    pub search_copy_keys: std::sync::Arc<[KeyChord]>,
    pub search_edit_keys: std::sync::Arc<[(KeyChord, crate::api::text_edit::EditAction)]>,
    pub search_match_priority: crate::api::hint::CompiledSearchPriority,
    pub search_titles: [String; 4],
    pub search_point: PointSettings,
    pub lifecycle: TargetingLifecycle,
    pub overlap_cycle_key: String,
    pub app_overrides: Vec<AppStrategyOverride>,
}

impl Settings {
    fn strategy_for(&self, app: Option<&FocusedApp>) -> UiScanStrategy {
        let Some(app) = app else {
            return self.strategy;
        };
        self.app_overrides
            .iter()
            .find(|entry| app.matches_pattern(&entry.pattern))
            .and_then(|entry| entry.strategy)
            .unwrap_or(self.strategy)
    }
}
/// What the keyboard is currently doing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Input {
    /// Typing hint labels.
    Labels(OverlayText),
    /// Typing a search query that filters by element name.
    Search(String),
}

#[derive(Clone, Copy)]
enum PendingView {
    Hints,
    Status,
}

impl Input {
    /// Text typed so far, whether that is a label prefix or a search query.
    fn text(&self) -> &str {
        match self {
            Input::Labels(text) => text.as_str(),
            Input::Search(text) => text.as_str(),
        }
    }
}

pub struct HintMode {
    pending_view: Option<PendingView>,
    inspection: Option<Box<point::Inspection>>,
    sample_serial: u64,
    config: Settings,
    alphabet: Vec<char>,
    overlap_cycle_chord: Option<KeyChord>,
    session: ScanSession,
    input: Input,
    return_mode: ModeId,
    held_overlap_keys: SmallVec<[Key; 2]>,
    overlap_cycle: usize,
    /// Source-agnostic visual layers over the merged UIA/Vision Hint list.
    /// Rebuilt at each exponentially batched Partial so overlap input stays hot.
    overlap_plan: VisualLayerPlan,
    wide_placements: Option<Vec<(usize, Rect)>>,
    window_bounds: Option<Rect>,
}

impl HintMode {
    pub fn new(config: Settings) -> Self {
        Self {
            pending_view: None,
            inspection: None,
            sample_serial: 0,
            alphabet: config
                .hint_characters
                .chars()
                .filter_map(|character| Key::new(character.to_string()).ok()?.as_char())
                .collect(),
            overlap_cycle_chord: KeyChord::parse(&config.overlap_cycle_key).ok(),
            config,
            session: ScanSession::default(),
            input: Input::Labels(OverlayText::default()),
            return_mode: ModeId::idle(),
            held_overlap_keys: SmallVec::new(),
            overlap_cycle: 0,
            overlap_plan: VisualLayerPlan::default(),
            wide_placements: None,
            window_bounds: None,
        }
    }

    fn clear_scan_results(&mut self) {
        self.session.clear_results();
        self.overlap_plan.release_retained();
        self.wide_placements = None;
    }

    fn request_scan(&mut self, ctx: &HostContext<'_>) -> CommandBatch {
        self.session.scanning = true;
        self.session.status = None;
        self.clear_scan_results();
        self.input = Input::Labels(OverlayText::default());
        self.session.selected = None;
        self.session.finished = false;
        self.session.retry_attempt = 0;
        self.session.retry_pending = false;
        let request = self.scan_request(ctx);
        let mut commands = CommandBatch::two(
            Command::CancelTimer {
                id: SCAN_RETRY_TIMER_ID.into(),
            },
            Command::HideOverlay,
        );
        commands.push(Command::ReleaseTextPrompt);
        commands.push(Command::CancelTimer {
            id: SEARCH_PREWARM_TIMER_ID.into(),
        });
        commands.push(request);
        self.window_bounds = None;
        if self.config.search_input_ui.position_mode == crate::api::style::PanelPositionMode::Window
            || self.config.search_info_ui.position_mode
                == crate::api::style::PanelPositionMode::Window
        {
            commands.push(Command::RequestPanelWindowBounds(
                self.session.scan_id | (1 << 63),
            ));
        }
        commands
    }

    fn retry_scan(&mut self, ctx: &HostContext<'_>) -> CommandBatch {
        self.session.retry_attempt = self.session.retry_attempt.saturating_add(1);
        self.session.retry_pending = false;
        self.session.scanning = true;
        CommandBatch::one(self.scan_request(ctx))
    }

    fn scan_request(&mut self, ctx: &HostContext<'_>) -> Command {
        self.session.scan_id = self.session.scan_id.wrapping_add(1);
        let bounds = ctx.active_bounds();
        self.session.scan_bounds = Some(bounds);
        let timeout_multiplier = u64::from(self.session.retry_attempt).saturating_add(1);
        let timeout_ms = self
            .config
            .scan_timeout_ms
            .saturating_mul(timeout_multiplier)
            .min(MAX_SCAN_TIMEOUT_MS);
        Command::scan_ui(UiScanRequest {
            id: self.session.scan_id,
            scope: self.config.scan_scope,
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

    fn handle_scan_result(&mut self, result: UiScanResult, ctx: &HostContext<'_>) -> CommandBatch {
        if !self.session.active || self.session.finished || result.id != self.session.scan_id {
            return CommandBatch::new();
        }
        let UiScanResult {
            targets,
            retired,
            status,
            ..
        } = result;
        if status == UiScanStatus::ContextChanged {
            if matches!(self.input, Input::Search(_)) {
                self.session.scanning = false;
                return CommandBatch::new();
            }
            // Context replacement is a normal retarget, not a failed scan.
            // Start a fresh generation immediately without consuming the
            // timeout/empty-result retry budget.
            return self.request_scan(ctx);
        }

        let can_relabel = self.input.text().is_empty() || matches!(self.input, Input::Search(_));
        let searching = matches!(self.input, Input::Search(_));
        if searching {
            std::mem::swap(&mut self.session.hints, &mut self.session.search_hints);
        }
        let (added, stable_labels) = if can_relabel {
            let deferred = self.session.pending_relabel;
            self.session.apply_deferred();
            if !deferred
                && !self.session.hints.is_empty()
                && self.session.hints.len() == self.session.scanned.len()
            {
                self.session.apply_stable_update(
                    targets,
                    &retired,
                    &self.alphabet,
                    self.config.label_direction,
                    !self.config.boundary_highlight.enabled,
                )
            } else {
                (
                    self.session.apply_update(targets, &retired) || deferred,
                    false,
                )
            }
        } else {
            if !targets.is_empty() || !retired.is_empty() {
                self.session.defer_update(targets, retired);
            }
            (false, false)
        };
        if searching {
            if !stable_labels {
                let _ = hints::assign_compact_into(
                    &mut self.session.hints,
                    self.session
                        .scanned
                        .iter()
                        .enumerate()
                        .map(|(index, target)| (target.rect, index)),
                    &self.alphabet,
                    self.config.label_direction,
                );
                self.session.label_plan_count = self.session.hints.len();
                self.session.next_label_index = self.session.hints.len();
            }
            std::mem::swap(&mut self.session.hints, &mut self.session.search_hints);
        }
        // Once the user starts typing, preserve the labels they can already
        // see and select. With no input, every partial remains visible. A
        // source-agnostic plan is rebuilt from the merged UIA/Vision labels at
        // each published batch, so overlap input never pays graph-build latency.
        let labels_changed =
            if added && (self.input.text().is_empty() || matches!(self.input, Input::Search(_))) {
                if stable_labels && !searching {
                    self.refresh_overlap_plan(ctx);
                } else {
                    self.relabel(ctx);
                }
                true
            } else {
                if added {
                    self.session.pending_relabel = true;
                }
                false
            };

        if status == UiScanStatus::Partial {
            self.session.status = None;
            return if labels_changed {
                self.redraw()
            } else {
                CommandBatch::new()
            };
        }

        let retryable_empty = self.session.scanned.is_empty()
            && matches!(status, UiScanStatus::Success | UiScanStatus::TimedOut)
            && self.session.retry_attempt < self.config.scan_retry_count;
        if retryable_empty {
            self.session.scanning = true;
            self.session.retry_pending = true;
            self.session.status = Some(format!(
                "UI scan is taking longer - retrying {}/{}",
                self.session.retry_attempt + 1,
                self.config.scan_retry_count
            ));
            let mut commands = if searching {
                self.redraw()
            } else {
                self.show_status()
            };
            commands.push(Command::SetTimer {
                id: SCAN_RETRY_TIMER_ID.into(),
                delay: Duration::from_millis(self.config.scan_retry_delay_ms),
                repeating: false,
            });
            return commands;
        }

        // Matching/selection only needs targets and labels after the scan ends.
        // Late or deferred updates rebuild the index lazily.
        self.session.release_scan_index();

        if status == UiScanStatus::Success
            && !labels_changed
            && !self.session.hints.is_empty()
            && self.session.status.is_none()
        {
            self.session.scanning = false;
            let needs_redraw = if !self.overlap_plan.is_ready() {
                self.rebuild_overlap_plan(ctx)
            } else {
                false
            };
            return if needs_redraw {
                self.redraw()
            } else {
                CommandBatch::new()
            };
        }

        self.session.scanning = false;
        if !self.session.hints.is_empty() && !self.overlap_plan.is_ready() {
            self.rebuild_overlap_plan(ctx);
        }
        self.session.status = match &status {
            UiScanStatus::Success if self.session.hints.is_empty() => {
                Some("No accessible targets — Esc to exit".into())
            }
            UiScanStatus::Success => None,
            UiScanStatus::Failed(message) if message == NO_WINDOW_UNDER_POINTER => {
                Some(message.clone())
            }
            UiScanStatus::PermissionDenied(message)
            | UiScanStatus::Unsupported(message)
            | UiScanStatus::Failed(message) => Some(format!("{message} — Esc to exit")),
            UiScanStatus::TimedOut => Some("UI scan timed out — Esc to exit".into()),
            UiScanStatus::Partial => self.session.status.clone(),
            UiScanStatus::ContextChanged => None,
        };
        if self.session.hints.is_empty() && !searching {
            return self.show_status();
        }
        self.redraw()
    }

    /// Assign labels to the targets matching the current search query.
    fn relabel(&mut self, ctx: &HostContext<'_>) {
        self.relabel_with_refresh(ctx, true);
    }

    fn relabel_with_refresh(&mut self, ctx: &HostContext<'_>, refresh: bool) -> bool {
        let mut matches_changed = false;
        self.session.apply_deferred();
        let query = match &self.input {
            // Key names are normalized to lowercase before Mode delivery, so
            // the accumulated search query is already canonical.
            Input::Search(query) => Some(query.as_str()),
            Input::Labels(_) => None,
        };
        if query.is_some_and(|query| !query.is_empty()) {
            self.session.ensure_search_names();
        }

        let result = if let Some(query) = query {
            let query = if !query.chars().any(char::is_uppercase) {
                std::borrow::Cow::Borrowed(query)
            } else {
                std::borrow::Cow::Owned(query.to_lowercase())
            };
            let multiple = query.chars().any(char::is_whitespace);
            let show_all = query.is_empty()
                || multiple && query.ends_with(char::is_whitespace)
                || query.split_whitespace().next_back() == Some("@");
            let previous_len = self.session.search_matches.len();
            let previous_selected = self.session.search_selected.len();
            let mut matched = 0;
            let mut selected = 0;
            self.session.search_cycle.clear();
            self.session.search_seen.clear();
            // Each space-separated item runs the same search against original labels.
            // Union in item order; the first occurrence owns each target's position.
            self.session.search_terms.prepare(&query);
            let needs_dedup = self.session.search_terms.needs_dedup();
            if needs_dedup {
                self.session
                    .search_seen
                    .resize(self.session.scanned.len(), false);
            }
            for term in self.session.search_terms.iter(&query) {
                let term = search::Term::new(term);
                let mut buckets = [None; search::RANK_COUNT];
                let (mut exact, mut last_match, mut term_matches) = (None, None, 0);
                for (hint_index, hint) in self.session.search_hints.iter().enumerate() {
                    if let Some(rank) = self.session.search_text.get(hint.value).and_then(|text| {
                        term.rank(
                            text,
                            hint.label.as_str(),
                            &self.config.search_match_priority,
                        )
                    }) {
                        if hint.label.as_str() == term.code {
                            exact = Some(hint);
                        }
                        last_match = Some(hint);
                        term_matches += 1;
                        if needs_dedup {
                            if self.session.search_seen[hint.value] {
                                continue;
                            }
                            self.session.search_seen[hint.value] = true;
                        }
                        if let Some(previous) = self.session.search_matches.get_mut(matched) {
                            matches_changed |= *previous != hint_index;
                            *previous = hint_index;
                        } else {
                            self.session.search_matches.push(hint_index);
                        }
                        // Candidate order keeps indices only. Update the displayed
                        // labels in place, without a second full clone pass.
                        let display_index = if show_all { hint_index } else { matched };
                        if let Some(previous) = self.session.hints.get_mut(display_index) {
                            if previous.value != hint.value
                                || previous.label != hint.label
                                || previous.bounds != hint.bounds
                            {
                                matches_changed = true;
                                if !show_all {
                                    previous.clone_from(hint);
                                }
                            }
                        } else if !show_all {
                            self.session.hints.push(hint.clone());
                        }
                        self.session.search_cycle.push(rank, &mut buckets);
                        matched += 1;
                    }
                }
                self.session.search_cycle.append(buckets);
                // A completed label takes priority over semantic collisions.
                // Other search terms select a point only after disambiguation.
                let resolved = exact.or_else(|| {
                    last_match.filter(|hint| {
                        term_matches == 1
                            && !term.labels_only
                            && self.session.search_text[hint.value].matches_text(term.code)
                    })
                });
                if let Some(hint) = resolved
                    && !self.session.search_selected[..selected]
                        .iter()
                        .any(|previous| previous.value == hint.value)
                {
                    if let Some(previous) = self.session.search_selected.get_mut(selected) {
                        matches_changed |= previous.value != hint.value
                            || previous.label != hint.label
                            || previous.bounds != hint.bounds;
                        previous.clone_from(hint);
                    } else {
                        self.session.search_selected.push(hint.clone());
                    }
                    selected += 1;
                }
            }
            matches_changed |= matched != previous_len;
            matches_changed |= selected != previous_selected;
            self.session.search_matches.truncate(matched);
            self.session.search_focus = self.session.search_focus.take().and_then(|mut focused| {
                focused.index = self.session.search_matches.iter().position(|&index| {
                    let hint = &self.session.search_hints[index];
                    hint.label == focused.label && hint.bounds == focused.bounds
                })?;
                Some(focused)
            });
            self.session.search_selected.truncate(selected);
            if show_all {
                self.session.hints.clear();
                self.session
                    .hints
                    .extend(self.session.search_hints.iter().cloned());
            } else {
                self.session.hints.truncate(matched);
            }
            Ok(())
        } else {
            hints::assign_compact_into(
                &mut self.session.hints,
                self.session
                    .scanned
                    .iter()
                    .enumerate()
                    .map(|(index, target)| (target.rect, index)),
                &self.alphabet,
                self.config.label_direction,
            )
        };

        if result.is_err() {
            self.session.hints.clear();
            self.session.status = Some("Cannot assign Hint labels — check hint_characters".into());
        }
        if query.is_none() {
            self.session.label_plan_count = self.session.hints.len();
            self.session.next_label_index = self.session.hints.len();
        }
        self.session.pending_relabel = false;
        if !self.session.scanning {
            self.session.release_scan_index();
        }
        if refresh {
            self.refresh_overlap_plan(ctx);
        }
        matches_changed
    }

    fn uniform_label_chars(&self) -> Option<usize> {
        labeling::LabelPlan::for_stream(
            self.session.label_plan_count,
            self.alphabet.len(),
            self.config.label_direction,
        )
        .and_then(|plan| plan.uniform_chars(self.alphabet.len()))
    }

    fn rebuild_overlap_plan(&mut self, ctx: &HostContext<'_>) -> bool {
        let content = hint_content(
            &self.config,
            &self.session.hints,
            &self.input,
            self.session.scan_bounds,
            self.session.search_selection,
            self.uniform_label_chars(),
        );
        ctx.presenter.prepare_hints(
            content,
            &mut self.overlap_plan,
            &mut self.wide_placements,
            ctx,
        );
        if self.held_overlap_keys.is_empty() {
            self.overlap_cycle = 0;
        } else if self.overlap_plan.layer_count() > 1 {
            self.overlap_cycle = self
                .overlap_cycle
                .clamp(1, self.overlap_plan.layer_count() - 1);
        }
        self.overlap_plan.layer_count() != 0
    }

    fn refresh_overlap_plan(&mut self, ctx: &HostContext<'_>) {
        self.rebuild_overlap_plan(ctx);
    }

    fn hint_is_visible(&self, hint: &CompactHint<usize>) -> bool {
        match &self.input {
            Input::Labels(typed) => {
                typed.is_empty() || hint.label.as_str().starts_with(typed.as_str())
            }
            Input::Search(_) => true,
        }
    }

    fn overlap_cycle_key(
        &mut self,
        key: &Key,
        state: KeyState,
        ctx: &HostContext<'_>,
    ) -> CommandBatch {
        if state == KeyState::Down && !self.overlap_plan.is_ready() {
            self.rebuild_overlap_plan(ctx);
        }
        let active_layer_changed = match state {
            KeyState::Down => {
                let was_released = self.held_overlap_keys.is_empty();
                let inserted = if self.held_overlap_keys.contains(key) {
                    false
                } else {
                    self.held_overlap_keys.push(key.clone());
                    true
                };
                if inserted && was_released {
                    let layer_count = self.overlap_plan.layer_count();
                    if layer_count > 1 {
                        // Releasing the modifier already restores layer zero.
                        // Cycle only the non-default layers so every press has
                        // an observable effect, especially in a two-layer stack.
                        self.overlap_cycle = self.overlap_cycle % (layer_count - 1) + 1;
                    } else {
                        self.overlap_cycle = 0;
                    }
                }
                inserted && was_released && self.overlap_plan.layer_count() > 1
            }
            KeyState::Up => {
                let was_last = self.held_overlap_keys.len() == 1;
                let removed = self
                    .held_overlap_keys
                    .iter()
                    .position(|candidate| candidate == key)
                    .map(|index| self.held_overlap_keys.swap_remove(index))
                    .is_some();
                removed && was_last && self.overlap_plan.layer_count() > 1
            }
        };
        if active_layer_changed && !self.session.hints.is_empty() {
            self.redraw()
        } else {
            CommandBatch::new()
        }
    }

    fn active_overlap_layer(&self) -> Option<usize> {
        let layer_count = self.overlap_plan.layer_count();
        if !self.overlap_plan.is_ready() || layer_count == 0 {
            return None;
        }
        Some(if self.held_overlap_keys.is_empty() || layer_count == 1 {
            0
        } else {
            self.overlap_cycle.clamp(1, layer_count - 1)
        })
    }

    fn view(&self) -> View<'_> {
        if self.session.finished {
            View::HintSelection(HintSelectionView {
                target: self
                    .session
                    .selected
                    .and_then(|index| self.session.scanned.get(index))
                    .map(|target| target.rect),
                scan_bounds: self.session.scan_bounds,
                boundary: &self.config.boundary_highlight,
            })
        } else {
            View::Hints(HintView {
                point: self
                    .inspection
                    .as_ref()
                    .map(|p| crate::api::presentation::HintPointView {
                        adjusting: p.adjusting,
                        position: p.position,
                        count: p.members.len(),
                        point: &p.point,
                        color: self.config.search_point.marker_color,
                        radius: self.config.search_point.marker_radius,
                        width: self.config.search_point.marker_width,
                    }),
                info: self.search_info(),
                window_bounds: &self.window_bounds,
                content: hint_content(
                    &self.config,
                    &self.session.hints,
                    &self.input,
                    self.session.scan_bounds,
                    self.session.search_selection,
                    self.uniform_label_chars(),
                ),
                layers: &self.overlap_plan,
                active_layer: self.active_overlap_layer(),
            })
        }
    }

    fn redraw(&mut self) -> CommandBatch {
        self.pending_view = Some(PendingView::Hints);
        CommandBatch::new()
    }

    /// Compose once, after selection, point state and scan hit testing agree.
    fn finish_redraw(&mut self, ctx: &HostContext<'_>, commands: &mut CommandBatch) {
        let Some(pending) = self.pending_view.take() else {
            return;
        };
        let view = match pending {
            PendingView::Hints => self.view(),
            PendingView::Status => View::Status(StatusView {
                text: self
                    .session
                    .status
                    .as_deref()
                    .unwrap_or("No accessible targets — Esc to exit"),
                ui: &self.config.ui,
                clip: self.session.scan_bounds,
            }),
        };
        // Keep close/warp effects ahead of the restored view so backend cursor
        // decoration uses the confirmed position and the current prompt owner.
        // Native sampling flushes the pending scene; clipboard acknowledgement
        // may re-enter the mode and close it. Both require this final view first.
        let before_async = commands
            .iter()
            .position(|command| matches!(command, Command::SamplePoint(_) | Command::CopyText(_)));
        let scene = ctx.present(view);
        if let Some(index) = before_async {
            commands.insert(index, scene);
        } else {
            commands.push(scene);
        }
        if matches!(pending, PendingView::Hints)
            && self.session.active
            && !self.session.finished
            && !self.session.scanned.is_empty()
            && !self.session.search_names_initialized
        {
            // Engine flushes this scene before firing due timers in the same
            // turn, so the renderer can start before search preprocessing.
            commands.push(Command::SetTimer {
                id: SEARCH_PREWARM_TIMER_ID.into(),
                delay: Duration::ZERO,
                repeating: false,
            });
        }
    }

    fn search_result_hints(&self) -> Option<(bool, &[CompactHint<usize>])> {
        if !self.session.active || self.session.finished {
            return None;
        }
        let Input::Search(query) = &self.input else {
            return None;
        };
        if let Some(hint) = self
            .session
            .search_focus
            .as_ref()
            .and_then(|focus| self.session.search_matches.get(focus.index))
            .and_then(|&index| self.session.search_hints.get(index))
        {
            return Some((false, std::slice::from_ref(hint)));
        }
        let multiple = query.chars().any(char::is_whitespace);
        let hints = &self.session.search_selected;
        // Explicit space-separated selections retain their collection panel.
        // Otherwise preview the highest-priority match immediately.
        if multiple && !hints.is_empty() && !query.trim().is_empty() {
            return Some((true, hints));
        }
        let first = self.session.search_cycle.first()?;
        Some((
            false,
            std::slice::from_ref(&self.session.search_hints[self.session.search_matches[first]]),
        ))
    }

    fn cycle_search_result(&mut self) -> bool {
        let current = self
            .session
            .search_focus
            .as_ref()
            .map(|focus| focus.index)
            .or_else(|| {
                let (multiple, _) = self.search_result_hints()?;
                if multiple {
                    None
                } else {
                    self.session.search_cycle.first()
                }
            });
        let Some(next) = self.session.search_cycle.next(current) else {
            return false;
        };
        let changed = current != Some(next);
        if changed {
            let hint = &self.session.search_hints[self.session.search_matches[next]];
            self.session.search_focus = Some(search::Focus {
                index: next,
                label: hint.label.clone(),
                bounds: hint.bounds,
            });
        }
        changed
    }

    fn search_info(&self) -> Option<crate::api::presentation::HintInfoView<'_>> {
        let (multiple, hints) = self.search_result_hints()?;
        Some(crate::api::presentation::HintInfoView {
            field_modes: &self.config.search_point.field_modes,
            point: self
                .inspection
                .as_ref()
                .map(|p| crate::api::presentation::HintPointInfo {
                    point: &p.point,
                    target: p.target,
                    color: p.color,
                    format: self.config.search_point.formats[p.format],
                    colors: &p.colors,
                }),
            preview: &self.session.search_preview,
            targets: &self.session.scanned,
            hints,
            multiple,
            ui: &self.config.search_info_ui,
            titles: &self.config.search_titles,
        })
    }

    fn open_search(&self, ctx: &HostContext<'_>) -> Command {
        let (bounds, style) =
            ctx.presenter
                .input_panel(&self.config.search_input_ui, self.window_bounds, ctx);
        Command::OpenTextPrompt(Box::new(crate::api::window_presets::TextPrompt {
            point_keys: Some(self.config.search_point.keys.clone()),
            bounds,
            id: self.session.scan_id | (1 << 63),
            title: String::new(),
            message: String::new(),
            placeholder: String::new(),
            max_chars: 4096,
            live_style: Some(style),
            edit_keys: self.config.search_edit_keys.clone(),
            copy_keys: self.config.search_copy_keys.clone(),
        }))
    }

    fn release_search(&mut self) {
        self.session.search_hints.clear();
        self.session.search_matches.clear();
        self.session.search_cycle.clear();
        self.session.search_focus = None;
        self.session.search_selected.clear();
        self.session.search_seen.clear();
        if let Input::Search(mut text) =
            std::mem::replace(&mut self.input, Input::Labels(OverlayText::default()))
        {
            text.clear();
            self.session.search_query = text;
        }
    }

    fn finish_search(&mut self, accept: bool, ctx: &HostContext<'_>) -> CommandBatch {
        if accept && self.session.pending_relabel {
            self.relabel_with_refresh(ctx, false);
        }
        let destination = accept
            .then(|| {
                let candidates = self.point_candidates();
                if candidates.is_empty() {
                    None
                } else if let Some(point) = self
                    .inspection
                    .as_ref()
                    .filter(|p| self.point_members_match(p))
                {
                    Some(point.point)
                } else {
                    Some(self.point_center(candidates))
                }
            })
            .flatten();
        std::mem::swap(&mut self.session.hints, &mut self.session.search_hints);
        self.release_search();
        self.refresh_overlap_plan(ctx);
        let mut commands = CommandBatch::new();
        if accept {
            commands.push(Command::CloseTextPrompt);
        }
        if let Some(point) = destination {
            commands.push(Command::warp_to(point));
        }
        commands.extend(self.redraw());
        if !accept {
            commands.push(Command::CloseTextPrompt);
        }
        commands
    }

    fn show_status(&mut self) -> CommandBatch {
        self.pending_view = Some(PendingView::Status);
        CommandBatch::new()
    }

    /// One destination for label selection, search acceptance and initial Point inspection.
    fn target_point(&self, index: usize) -> crate::api::Point {
        self.session.scanned[index].rect.center()
    }

    fn select(&mut self, index: usize) -> CommandBatch {
        if index >= self.session.scanned.len() {
            return self.cancel();
        }
        let point = self.target_point(index);
        self.session.selected = Some(index);
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
        if matches!(self.input, Input::Search(_)) {
            if key.as_str() == "enter" {
                return self.finish_search(true, ctx);
            }
            if key.as_str() == "esc" {
                return self.finish_search(false, ctx);
            }
            if key.as_char().is_some() || key.as_str() == "backspace" {
                self.session.search_focus = None;
            }
        }
        if self.session.finished {
            return match key.as_str() {
                "esc" => self.cancel(),
                "backspace" | "tab" => {
                    self.session.finished = false;
                    self.session.selected = None;
                    if let Input::Labels(typed) = &mut self.input {
                        typed.pop();
                    }
                    self.redraw()
                }
                _ => CommandBatch::new(),
            };
        }
        // Search can open before the first batch and filter results as they
        // arrive. Only label selection needs to wait for targets.
        if self.session.scanning
            && self.session.hints.is_empty()
            && matches!(self.input, Input::Labels(_))
            && key.as_str() != "/"
        {
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
                        self.input = Input::Labels(OverlayText::default());
                        self.relabel(ctx);
                        self.redraw()
                    }
                    Input::Labels(_) => self.cancel(),
                };
            }
            "/" if matches!(self.input, Input::Labels(_)) => {
                if self.session.pending_relabel {
                    self.input = Input::Labels(OverlayText::default());
                    self.relabel(ctx);
                }
                // An empty search keeps every existing label and its geometry.
                // Reuse the prepared layers only when no label prefix was active.
                let same_labels =
                    self.input.text().is_empty() && self.session.search_query.is_empty();
                self.session.search_hints.clone_from(&self.session.hints);
                self.input = Input::Search(std::mem::take(&mut self.session.search_query));
                self.session.search_selection = Default::default();
                if self.input.text().is_empty() {
                    // Opening an empty query leaves the full label list intact.
                    // Avoid clearing and copying it back through the filter path.
                    self.session.search_matches.clear();
                    self.session.search_cycle.clear();
                    self.session.search_focus = None;
                    self.session.search_selected.clear();
                    self.session.search_seen.clear();
                    if !same_labels {
                        self.refresh_overlap_plan(ctx);
                    }
                } else {
                    self.relabel(ctx);
                }
                let mut commands = self.redraw();
                commands.push(self.open_search(ctx));
                return commands;
            }
            "backspace" => {
                let text = self.input.text();
                if text.is_empty() {
                    // Nothing left to undo: leave search, or leave the mode.
                    return match &self.input {
                        Input::Search(_) => {
                            self.input = Input::Labels(OverlayText::default());
                            self.relabel(ctx);
                            self.redraw()
                        }
                        Input::Labels(_) => self.cancel(),
                    };
                }
                match &mut self.input {
                    Input::Labels(text) => {
                        text.pop();
                    }
                    Input::Search(text) => {
                        text.pop();
                    }
                }
                if matches!(self.input, Input::Search(_))
                    || (self.input.text().is_empty() && self.session.pending_relabel)
                {
                    self.relabel(ctx);
                } else {
                    self.refresh_overlap_plan(ctx);
                }
                return self.redraw();
            }
            "enter" => {
                // Accept the first visible candidate, never one filtered out by
                // the label prefix.
                return match self
                    .session
                    .hints
                    .iter()
                    .find(|hint| self.hint_is_visible(hint))
                {
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
                if !ch.is_control() {
                    query.push(ch);
                    self.relabel(ctx);
                    return self.redraw();
                }
                CommandBatch::new()
            }
            Input::Labels(typed) => {
                if !self.alphabet.contains(&ch) {
                    return CommandBatch::new();
                }
                typed.push(ch);
                match match_compact_input(&self.session.hints, typed) {
                    Match::Complete(index) => self.select(index),
                    Match::Partial { .. } => {
                        self.refresh_overlap_plan(ctx);
                        self.redraw()
                    }
                    // Dead end: drop the character and keep the hints up.
                    Match::None => {
                        if let Input::Labels(typed) = &mut self.input {
                            typed.pop();
                        }
                        if self.input.text().is_empty() && self.session.pending_relabel {
                            self.relabel(ctx);
                            self.redraw()
                        } else {
                            CommandBatch::new()
                        }
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
    let mut remaining = 0usize;
    for hint in hints {
        let label = hint.label.as_str();
        if label == input {
            return Match::Complete(hint.value);
        }
        remaining += usize::from(label.starts_with(input));
    }
    if remaining == 0 {
        Match::None
    } else {
        Match::Partial { remaining }
    }
}

fn hint_style(config: &Settings) -> HintStyle<'_> {
    HintStyle {
        ui: &config.ui,
        placement: config.placement,
        label_x_offset: config.label_x_offset,
        label_y_offset: config.label_y_offset,
        boundary_highlight: &config.boundary_highlight,
        search_input_ui: &config.search_input_ui,
    }
}
fn hint_content<'a>(
    config: &'a Settings,
    hints: &'a [CompactHint<usize>],
    input: &'a Input,
    scan_bounds: Option<Rect>,
    search_selection: crate::api::text_edit::Selection,
    uniform_label_chars: Option<usize>,
) -> HintContent<'a> {
    let (prefix, search) = match input {
        Input::Labels(prefix) => (prefix.as_str(), None),
        Input::Search(query) => ("", Some(query.as_str())),
    };
    HintContent {
        hints,
        uniform_label_chars,
        prefix,
        search,
        search_selection,
        scan_bounds,
        style: hint_style(config),
    }
}

impl Mode for HintMode {
    fn id(&self) -> ModeId {
        ModeId::ui_hint()
    }

    fn display_name(&self) -> String {
        "Hints".into()
    }

    fn claims_key(&self, key: &Key) -> bool {
        if self
            .inspection
            .as_ref()
            .is_some_and(|point| point.adjusting)
        {
            return false;
        }
        self.overlap_cycle_chord
            .as_ref()
            .is_some_and(|chord| chord.activation_matches(key))
            || key.as_char().is_some_and(|character| {
                character == '/'
                    || character.is_ascii_alphanumeric()
                    || self.alphabet.contains(&character)
            })
    }

    fn available_keys(&self) -> Vec<(String, String)> {
        let mut keys = vec![("esc".into(), "cancel".into())];
        if self.session.finished {
            keys.push(("backspace".into(), "back".into()));
            keys.push(("tab".into(), "back".into()));
            return keys;
        }
        if self.session.scanning && self.session.hints.is_empty() {
            return keys;
        }
        keys.push(("backspace".into(), "back".into()));
        keys.push(("enter".into(), "select first match".into()));
        match &self.input {
            Input::Labels(prefix) => {
                keys.push(("/".into(), "search names".into()));
                let next: std::collections::BTreeSet<_> = self
                    .session
                    .hints
                    .iter()
                    .filter_map(|hint| hint.label.as_str().strip_prefix(prefix.as_str()))
                    .filter_map(|suffix| suffix.chars().next())
                    .collect();
                keys.extend(
                    next.into_iter()
                        .map(|key| (key.to_string(), "select hint".into())),
                );
            }
            Input::Search(_) => {
                keys.extend(
                    ('a'..='z')
                        .chain('0'..='9')
                        .map(|key| (key.to_string(), "search names".into())),
                );
                keys.push(("space".into(), "search names".into()));
            }
        }
        if let Some(chord) = &self.overlap_cycle_chord {
            keys.push((chord.canonical(), "cycle overlapping hints".into()));
        }
        keys
    }

    fn indicator_color(&self, palette: &Palette) -> Option<Color> {
        Some(palette.accent)
    }

    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        let finishing =
            matches!(event, ModeEvent::FinishRequested { .. }) && !self.session.finished;
        let mut out = if let Some(out) = self
            .inspection
            .is_some()
            .then(|| self.point_event(event, ctx))
            .flatten()
        {
            out
        } else {
            let mut out = self.handle_event(event, ctx);
            if matches!(self.input, Input::Search(_)) || self.inspection.is_some() {
                self.sync_point(ctx, &mut out);
            }
            out
        };
        if matches!(event, ModeEvent::UiScanned(_)) && self.refresh_point_hit() {
            self.redraw();
        }
        self.finish_redraw(ctx, &mut out);
        if finishing {
            out.extend(super::targeting::lifecycle_commands(
                &self.config.lifecycle.after_finish,
                &self.return_mode,
            ));
        }
        out
    }

    fn handle_owned(&mut self, event: ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        if let ModeEvent::UiScanned(result) = event {
            let mut out = self.handle_scan_result(result, ctx);
            if matches!(self.input, Input::Search(_)) || self.inspection.is_some() {
                self.sync_point(ctx, &mut out);
            }
            if self.refresh_point_hit() {
                self.redraw();
            }
            self.finish_redraw(ctx, &mut out);
            out
        } else {
            self.handle(&event, ctx)
        }
    }
}

impl HintMode {
    fn handle_event(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        if matches!(self.input, Input::Search(_)) {
            use crate::api::text_edit::EditAction;
            let mut encoded = [0; 4];
            match event {
                ModeEvent::CyclePointTarget => {
                    return if self.cycle_search_result() {
                        self.redraw()
                    } else {
                        CommandBatch::new()
                    };
                }
                ModeEvent::TextEdit(EditAction::Accept) => {
                    return self.finish_search(true, ctx);
                }
                ModeEvent::TextEdit(EditAction::Cancel) => {
                    return self.finish_search(false, ctx);
                }
                ModeEvent::TextEdit(EditAction::Paste) => {
                    return CommandBatch::one(Command::ReadClipboard);
                }
                ModeEvent::TextEdit(EditAction::Copy | EditAction::Cut) => {
                    let Input::Search(text) = &mut self.input else {
                        unreachable!()
                    };
                    let range = self.session.search_selection.range();
                    if range.is_empty() {
                        return CommandBatch::new();
                    }
                    return CommandBatch::one(Command::CopyInputText {
                        text: text[range].into(),
                        cut: matches!(event, ModeEvent::TextEdit(EditAction::Cut)),
                    });
                }
                ModeEvent::TextInserted(_) | ModeEvent::TextPasted(_) | ModeEvent::TextEdit(_) => {
                    let previous_count = self.session.hints.len();
                    let pending = self.session.pending_relabel;
                    let previous_selection = self.session.search_selection;
                    let Input::Search(text) = &mut self.input else {
                        unreachable!()
                    };
                    let text_changed = match event {
                        ModeEvent::TextInserted(c) => self
                            .session
                            .search_selection
                            .insert(text, c.encode_utf8(&mut encoded)),
                        ModeEvent::TextPasted(value) => {
                            self.session.search_selection.insert(text, value)
                        }
                        ModeEvent::TextEdit(action) => {
                            self.session.search_selection.edit(text, *action)
                        }
                        _ => unreachable!(),
                    };
                    if !text_changed && previous_selection == self.session.search_selection {
                        return CommandBatch::new();
                    }
                    // Cursor-only changes repaint without rebuilding the search index.
                    if text_changed {
                        self.session.search_focus = None;
                        let changed = self.relabel_with_refresh(ctx, false);
                        if changed || pending || previous_count != self.session.hints.len() {
                            self.refresh_overlap_plan(ctx);
                        }
                    }
                    return self.redraw();
                }
                _ => {}
            }
        }
        match event {
            ModeEvent::PanelWindowBounds { id, bounds }
                if *id == self.session.scan_id | (1 << 63) =>
            {
                self.window_bounds = *bounds;
                CommandBatch::new()
            }
            ModeEvent::TextChanged(text) if matches!(self.input, Input::Search(_)) => {
                if self.input.text() == text {
                    return CommandBatch::new();
                }
                let pending = self.session.pending_relabel;
                self.session.search_focus = None;
                let old_fields = self.search_info().map(|info| info.field_count());
                let shows_all = |query: &str| {
                    query.is_empty()
                        || query.ends_with(char::is_whitespace)
                        || query.split_whitespace().next_back() == Some("@")
                };
                let old_all = shows_all(self.input.text());
                if let Input::Search(query) = &mut self.input {
                    let end = text.char_indices().nth(4096).map_or(text.len(), |(i, _)| i);
                    query.clear();
                    query.push_str(&text[..end]);
                    self.session.search_selection.cursor = query.len();
                    self.session.search_selection.anchor = query.len();
                }
                let changed = self.relabel_with_refresh(ctx, false);
                let unchanged = !pending
                    && !changed
                    && old_all == shows_all(self.input.text())
                    && old_fields == self.search_info().map(|info| info.field_count());
                if unchanged {
                    self.redraw()
                } else {
                    self.refresh_overlap_plan(ctx);
                    self.redraw()
                }
            }
            ModeEvent::TextSubmitted(text) if matches!(self.input, Input::Search(_)) => {
                if let Some(text) = text
                    && self.input.text() != text
                {
                    if let Input::Search(query) = &mut self.input {
                        query.clone_from(text);
                    }
                    // A submitted value can differ from the last live edit.
                    // Filter it once; only the restored full view needs layout.
                    self.session.search_focus = None;
                    self.relabel_with_refresh(ctx, false);
                }
                self.finish_search(text.is_some(), ctx)
            }
            ModeEvent::TextCopied if matches!(self.input, Input::Search(_)) => {
                // Copying from point adjustment also accepts that point. The
                // clipboard acknowledgement replaces a separate Enter press.
                let accept = self
                    .inspection
                    .as_ref()
                    .is_some_and(|point| point.adjusting);
                self.finish_search(accept, ctx)
            }
            ModeEvent::CopyTextField(index) => {
                let value = self
                    .search_info()
                    .filter(|info| *index < info.field_count())
                    .map(|info| info.copy_value(*index));
                value
                    .filter(|text| !text.is_empty())
                    .map_or_else(CommandBatch::new, |text| {
                        CommandBatch::one(Command::CopyText(text))
                    })
            }
            ModeEvent::Activated { previous } => {
                self.session.active = true;
                self.return_mode = previous.clone().unwrap_or_else(ModeId::idle);
                self.request_scan(ctx)
            }
            ModeEvent::Restarted => {
                self.session.active = true;
                self.request_scan(ctx)
            }
            ModeEvent::FinishRequested { .. } if self.session.finished => CommandBatch::new(),
            ModeEvent::FinishRequested { .. } => {
                self.session.finished = true;
                self.redraw()
            }
            ModeEvent::Clicked { .. } => super::targeting::lifecycle_commands(
                &self.config.lifecycle.after_click,
                &self.return_mode,
            ),
            ModeEvent::Deactivated => {
                self.session.active = false;
                self.clear_scan_results();
                self.session.scanning = false;
                self.session.scan_bounds = None;
                self.input = Input::Labels(OverlayText::default());
                self.held_overlap_keys = SmallVec::new();
                self.session.status = None;
                self.overlap_cycle = 0;
                self.session.retry_pending = false;
                self.session.selected = None;
                self.session.finished = false;
                let mut commands = CommandBatch::one(Command::ReleaseTextPrompt);
                commands.push(Command::CancelTimer {
                    id: SCAN_RETRY_TIMER_ID.into(),
                });
                commands.push(Command::CancelTimer {
                    id: SEARCH_PREWARM_TIMER_ID.into(),
                });
                commands
            }
            ModeEvent::UiScanned(result)
                if self.session.finished || result.id != self.session.scan_id =>
            {
                CommandBatch::new()
            }
            ModeEvent::UiScanActivationExpected { id, process_id }
                if *id == self.session.scan_id =>
            {
                self.session.expected_activation = Some(*process_id);
                CommandBatch::new()
            }
            ModeEvent::FocusChanged(Some(app))
                if self.session.expected_activation == Some(app.process_id) =>
            {
                self.session.expected_activation = None;
                CommandBatch::new()
            }
            ModeEvent::UiScanned(result) => self.handle_scan_result(result.clone(), ctx),
            ModeEvent::Timer { id, .. }
                if id == SEARCH_PREWARM_TIMER_ID
                    && self.session.active
                    && !self.session.finished
                    && !self.session.scanned.is_empty() =>
            {
                if self.session.prepare_search_names(SEARCH_PREWARM_TARGETS) {
                    CommandBatch::new()
                } else {
                    // The scheduler snapshots due timers once per turn. A new
                    // zero-delay dispatch runs after the next input poll.
                    CommandBatch::one(Command::SetTimer {
                        id: SEARCH_PREWARM_TIMER_ID.into(),
                        delay: Duration::ZERO,
                        repeating: false,
                    })
                }
            }
            ModeEvent::Timer { id, .. }
                if id == SCAN_RETRY_TIMER_ID
                    && self.session.active
                    && self.session.retry_pending
                    && self.session.scanning
                    && self.session.hints.is_empty() =>
            {
                self.retry_scan(ctx)
            }
            ModeEvent::Binding {
                binding,
                state: KeyState::Down,
                ..
            } if self.session.active && matches!(binding.as_ref(), Binding::RescanUi) => {
                self.request_scan(ctx)
            }
            // The tree we labelled belongs to the old window/geometry.
            ModeEvent::FocusChanged(_) | ModeEvent::ScreensChanged(_)
                if self.session.active && !self.session.finished =>
            {
                self.request_scan(ctx)
            }
            ModeEvent::PointerMoved(_)
                if self.session.active
                    && !self.session.finished
                    && self
                        .session
                        .scan_bounds
                        .is_some_and(|bounds| bounds != ctx.active_bounds()) =>
            {
                self.request_scan(ctx)
            }
            ModeEvent::ScreenRetargeted { screen, .. } => {
                CommandBatch::one(Command::warp_to(screen.bounds.center()))
            }
            ModeEvent::Resumed if self.session.finished => self.redraw(),
            ModeEvent::Resumed if self.session.scanning && self.session.hints.is_empty() => {
                CommandBatch::one(Command::HideOverlay)
            }
            ModeEvent::Resumed if self.session.hints.is_empty() => self.show_status(),
            ModeEvent::Resumed => self.redraw(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_fresh_scene(mode: &HintMode, env: &Env, out: &CommandBatch) {
        let mut scenes = out.iter().filter_map(|command| match command {
            Command::ShowOverlay(scene) => Some(scene),
            _ => None,
        });
        assert_eq!(scenes.next().unwrap().as_ref(), &mode.scene(&env.ctx()));
        assert!(
            scenes.next().is_none(),
            "one event must compose only one final scene"
        );
        let presented = out
            .iter()
            .position(|c| matches!(c, Command::ShowOverlay(_)))
            .unwrap();
        for (index, command) in out.iter().enumerate() {
            if matches!(command, Command::SamplePoint(_) | Command::CopyText(_)) {
                assert!(
                    presented < index,
                    "async effects must observe the final scene"
                );
            }
        }
    }

    #[test]
    fn search_entry_layout_matches_rebuild_with_prefix_and_restored_query() {
        for scale in [1.0, 2.0] {
            for (prefix, query) in [("", ""), ("a", ""), ("", "control 0")] {
                let mut config = Config::default();
                config.ui_hint.hint_characters = "asdfghjkl".into();
                let mut env = Env::with(config);
                env.screens[0].scale = scale;
                let mut mode = crate::app::mode_catalog::hint(&env.config);
                activate(&mut mode, &env);
                deliver(
                    &mut mode,
                    &env,
                    (0..74)
                        .map(|i| target(&format!("control {i}"), (i % 20) as f64 * 10.0))
                        .collect(),
                );
                if !prefix.is_empty() {
                    press(&mut mode, &env, prefix);
                }
                mode.session.search_query = query.into();
                let out = press(&mut mode, &env, "/");
                let actual = scene_of(&out).clone();
                mode.relabel(&env.ctx());
                assert_eq!(
                    actual,
                    mode.scene(&env.ctx()),
                    "scale={scale} prefix={prefix:?} query={query:?}"
                );
            }
        }
    }

    #[test]
    fn point_state_and_sampling_follow_result_panel_visibility() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, point_text_targets());
        assert!(mode.inspection.is_none());
        press(&mut mode, &env, "/");
        for query in ["@", "missing", " ", ""] {
            let out = mode.handle(&ModeEvent::TextChanged(query.into()), &env.ctx());
            assert!(mode.search_info().is_none(), "query: {query:?}");
            assert!(mode.inspection.is_none(), "query: {query:?}");
            assert!(!out.iter().any(|command| matches!(
                command,
                Command::SamplePoint(_)
                    | Command::SetPointAdjustment {
                        available: true,
                        ..
                    }
            )));
        }
        let first = mode.handle(&ModeEvent::TextChanged("alpha".into()), &env.ctx());
        assert_fresh_scene(&mode, &env, &first);
        let stale = sample_request(&first);
        let allocation = std::ptr::from_ref(mode.inspection.as_deref().unwrap());
        let multiple = mode.handle(&ModeEvent::TextChanged("bravo alpha".into()), &env.ctx());
        assert_fresh_scene(&mode, &env, &multiple);
        assert_eq!(
            std::ptr::from_ref(mode.inspection.as_deref().unwrap()),
            allocation
        );
        assert_eq!(mode.inspection.as_ref().unwrap().members.len(), 2);
        assert!(mode.search_info().is_some());
        assert!(
            mode.handle(
                &ModeEvent::PointSampled(crate::api::point_sample::Sample {
                    request: stale,
                    color: Some(Color::rgb(1, 2, 3)),
                }),
                &env.ctx()
            )
            .is_empty()
        );
        let hidden = mode.handle(&ModeEvent::TextChanged("@".into()), &env.ctx());
        assert_fresh_scene(&mode, &env, &hidden);
        assert!(mode.inspection.is_none());
        assert!(hidden.contains(&Command::CancelPointSample));
        assert!(hidden.contains(&Command::CancelTimer {
            id: point::TIMER.into()
        }));
    }

    #[test]
    fn point_selection_reuses_active_timer_and_only_updates_changed_host_state() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, point_text_targets());
        press(&mut mode, &env, "/");
        let first = mode.handle(&ModeEvent::TextChanged("alpha".into()), &env.ctx());
        assert!(first.iter().any(|command| matches!(command,
            Command::SetTimer { id, repeating: true, .. } if id == point::TIMER)));
        let stale = sample_request(&first);
        let changed = mode.handle(&ModeEvent::TextChanged("bravo".into()), &env.ctx());
        assert_eq!(changed.len(), 2);
        assert!(matches!(&changed[0], Command::ShowOverlay(_)));
        let current = sample_request(&changed);
        assert_ne!(stale.id, current.id);
        assert!(
            mode.handle(
                &ModeEvent::PointSampled(crate::api::point_sample::Sample {
                    request: stale,
                    color: Some(Color::rgb(1, 2, 3))
                }),
                &env.ctx()
            )
            .is_empty()
        );
        let timer = ModeEvent::Timer {
            id: point::TIMER.into(),
            elapsed: Duration::from_millis(100),
        };
        assert!(mode.handle(&timer, &env.ctx()).is_empty());
        mode.handle(
            &ModeEvent::PointSampled(crate::api::point_sample::Sample {
                request: current,
                color: Some(Color::rgb(1, 2, 3)),
            }),
            &env.ctx(),
        );
        assert_ne!(
            sample_request(&mode.handle(&timer, &env.ctx())).id,
            current.id
        );

        mode.handle(&ModeEvent::TogglePointAdjustment, &env.ctx());
        let moving = mode.handle(
            &ModeEvent::Binding {
                binding: std::sync::Arc::new(Binding::Move(crate::api::Direction::Right)),
                state: KeyState::Down,
                key: Key::new("l").unwrap(),
            },
            &env.ctx(),
        );
        assert!(moving.contains(&Command::SetFrameClock(true)));
        let changed = mode.handle(&ModeEvent::TextChanged("alpha".into()), &env.ctx());
        assert!(changed.contains(&Command::SetFrameClock(false)));
        assert!(changed.contains(&Command::SetPointAdjustment {
            available: true,
            adjusting: false
        }));
        assert!(
            !changed
                .iter()
                .any(|command| matches!(command, Command::SetTimer { .. }))
        );

        let multiple = mode.handle(&ModeEvent::TextChanged("alpha bravo".into()), &env.ctx());
        sample_request(&multiple);
        assert!(!multiple.iter().any(|command| matches!(
            command,
            Command::CancelPointSample | Command::CancelTimer { .. } | Command::SetTimer { .. }
        )));
        let changed = mode.handle(&ModeEvent::TextChanged("bravo alpha".into()), &env.ctx());
        assert!(!changed.iter().any(|command| matches!(
            command,
            Command::SetFrameClock(_)
                | Command::SetPointAdjustment { .. }
                | Command::SetTimer { .. }
                | Command::CancelTimer { .. }
        )));
        let unique = mode.handle(&ModeEvent::TextChanged("alpha".into()), &env.ctx());
        sample_request(&unique);
        assert!(
            !unique
                .iter()
                .any(|command| matches!(command, Command::SetTimer { .. }))
        );
    }

    #[test]
    fn streamed_point_hit_and_finish_submit_final_state_before_lifecycle() {
        for owned in [false, true] {
            let env = Env::new();
            let mut mode = crate::app::mode_catalog::hint(&env.config);
            activate(&mut mode, &env);
            deliver(&mut mode, &env, point_text_targets());
            press(&mut mode, &env, "/");
            mode.handle(&ModeEvent::TextChanged("alpha".into()), &env.ctx());
            let mut child = target("child", 35.0);
            child.rect = Rect::new(35.0, 108.0, 10.0, 10.0);
            let event = ModeEvent::UiScanned(UiScanResult {
                id: mode.session.scan_id,
                retired: Vec::new(),
                targets: vec![child],
                status: UiScanStatus::Partial,
            });
            let out = if owned {
                mode.handle_owned(event, &env.ctx())
            } else {
                mode.handle(&event, &env.ctx())
            };
            assert_fresh_scene(&mode, &env, &out);
            assert_eq!(mode.inspection.as_ref().unwrap().target, Some(2));
            assert_eq!(mode.search_info().unwrap().values()[1], "child · button");
            mode.config.lifecycle.after_finish = crate::config::LifecycleAction::Return;
            let out = mode.handle(
                &ModeEvent::FinishRequested {
                    cause: FinishCause::Explicit,
                },
                &env.ctx(),
            );
            assert_fresh_scene(&mode, &env, &out);
            let position =
                |predicate: fn(&Command) -> bool| out.iter().position(predicate).unwrap();
            let scene = position(|command| matches!(command, Command::ShowOverlay(_)));
            assert!(scene < position(|command| matches!(command, Command::SwitchMode(_))));
            assert!(position(|command| matches!(command, Command::CancelPointSample)) < scene);
            assert!(mode.inspection.is_none());
        }
    }
    #[test]
    fn point_field_modes_mix_joined_and_current_text_and_coordinates() {
        use crate::api::point_sample::FieldMode::{Concat, Switch};
        let mut config = Config::default();
        config.ui_hint.search_point.field_modes = [Switch, Concat, Concat, Switch];
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, point_text_targets());
        press(&mut mode, &env, "/");
        mode.handle(&ModeEvent::TextChanged("alpha bravo".into()), &env.ctx());
        assert_eq!(mode.search_info().unwrap().values()[0], "");
        mode.handle(&ModeEvent::TogglePointAdjustment, &env.ctx());
        for (text, accessibility) in [
            ("alpha", "alpha · button\nbravo · button"),
            ("bravo", "alpha · button\nbravo · button"),
        ] {
            mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
            let values = mode.search_info().unwrap().values();
            assert_eq!(values[0], text);
            assert_eq!(values[1], accessibility);
            assert_eq!(values[2], "40, 112\n240, 112");
            assert_eq!(values[3], "");
        }
        mode.config.search_point.field_modes = [Concat, Switch, Switch, Concat];
        let values = mode.search_info().unwrap().values();
        assert_eq!(values[0], "alpha\nbravo");
        assert_eq!(values[1], "bravo · button");
        assert_eq!(values[2], "240, 112");
    }

    fn point_text_targets() -> Vec<UiTarget> {
        ["alpha", "bravo"]
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let mut item = target(name, i as f64 * 200.0);
                item.details = Some(Box::new(crate::api::geometry::UiTargetDetails {
                    ocr: name.into(),
                    accessibility: name.into(),
                    color: None,
                }));
                item
            })
            .collect()
    }

    fn sample_request(out: &CommandBatch) -> crate::api::point_sample::Request {
        out.iter()
            .find_map(|command| match command {
                Command::SamplePoint(request) => Some(*request),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn point_joined_colors_sample_sequentially_resume_after_cycle_and_copy_once_ready() {
        use crate::api::point_sample::{FieldMode, Sample};
        let mut config = Config::default();
        config.ui_hint.search_point.field_modes[3] = FieldMode::Concat;
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, point_text_targets());
        press(&mut mode, &env, "/");
        let center =
            sample_request(&mode.handle(&ModeEvent::TextChanged("bravo alpha".into()), &env.ctx()));
        let timer = ModeEvent::Timer {
            id: point::TIMER.into(),
            elapsed: Duration::from_millis(100),
        };
        assert!(mode.handle(&timer, &env.ctx()).is_empty());
        let first = sample_request(&mode.handle(
            &ModeEvent::PointSampled(Sample {
                request: center,
                color: Some(Color::rgb(99, 99, 99)),
            }),
            &env.ctx(),
        ));
        assert_eq!(first.point, mode.target_point(1));
        assert_eq!(mode.search_info().unwrap().values()[3], "");
        mode.handle(&ModeEvent::TogglePointAdjustment, &env.ctx());
        let selected = sample_request(&mode.handle(&ModeEvent::CyclePointTarget, &env.ctx()));
        assert!(
            mode.handle(
                &ModeEvent::PointSampled(Sample {
                    request: first,
                    color: Some(Color::rgb(255, 0, 0))
                }),
                &env.ctx()
            )
            .is_empty()
        );
        let second = sample_request(&mode.handle(
            &ModeEvent::PointSampled(Sample {
                request: selected,
                color: Some(Color::rgb(1, 2, 3)),
            }),
            &env.ctx(),
        ));
        assert_eq!(second.point, mode.target_point(0));
        assert_eq!(mode.search_info().unwrap().values()[3], "#010203\n");
        let pending = mode.handle(&ModeEvent::CopyTextField(3), &env.ctx());
        assert!(
            !pending
                .iter()
                .any(|command| matches!(command, Command::CopyText(_) | Command::SamplePoint(_)))
        );
        assert!(mode.handle(&timer, &env.ctx()).is_empty());
        assert!(
            mode.handle(&ModeEvent::CyclePointTarget, &env.ctx())
                .is_empty()
        );
        let out = mode.handle(
            &ModeEvent::PointSampled(Sample {
                request: second,
                color: Some(Color::rgb(4, 5, 6)),
            }),
            &env.ctx(),
        );
        assert!(out.contains(&Command::CopyText("#010203\n#040506".into())));
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::SamplePoint(_)))
        );
        assert_eq!(
            mode.inspection.as_ref().unwrap().point,
            mode.target_point(1)
        );
        mode.handle(&ModeEvent::CyclePointColor, &env.ctx());
        assert_eq!(
            mode.search_info().unwrap().values()[3],
            "rgb(1, 2, 3)\nrgb(4, 5, 6)"
        );
        let refresh = sample_request(&mode.handle(&timer, &env.ctx()));
        assert_eq!(refresh.point, mode.target_point(1));
        let out = mode.handle(
            &ModeEvent::PointSampled(Sample {
                request: refresh,
                color: None,
            }),
            &env.ctx(),
        );
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::CopyText(_)))
        );
        assert_eq!(
            mode.search_info().unwrap().values()[3],
            "\nrgb(4, 5, 6)",
            "failed refresh cannot retain the previous member color"
        );
    }

    #[test]
    fn all_four_point_field_modes_control_display_copy_and_color_swatch() {
        use crate::api::point_sample::{FieldMode, Sample};
        for mask in 0..16 {
            let mut config = Config::default();
            config.ui_hint.search_point.field_modes = std::array::from_fn(|field| {
                if mask & (1 << field) != 0 {
                    FieldMode::Concat
                } else {
                    FieldMode::Switch
                }
            });
            let config = Config::parse(&config.to_toml().unwrap()).unwrap();
            let env = Env::with(config);
            let mut mode = crate::app::mode_catalog::hint(&env.config);
            activate(&mut mode, &env);
            deliver(&mut mode, &env, point_text_targets());
            press(&mut mode, &env, "/");
            let mut out = mode.handle(&ModeEvent::TextChanged("alpha bravo".into()), &env.ctx());
            let first_color = Color::rgb(1, 2, 3);
            let second_color = Color::rgb(4, 5, 6);
            while out.iter().any(|c| matches!(c, Command::SamplePoint(_))) {
                let request = sample_request(&out);
                let color = if request.point == mode.target_point(0) {
                    first_color
                } else {
                    second_color
                };
                out = mode.handle(
                    &ModeEvent::PointSampled(Sample {
                        request,
                        color: Some(color),
                    }),
                    &env.ctx(),
                );
            }
            mode.handle(&ModeEvent::TogglePointAdjustment, &env.ctx());
            mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
            let request = sample_request(&mode.handle(&ModeEvent::CyclePointTarget, &env.ctx()));
            let out = mode.handle(
                &ModeEvent::PointSampled(Sample {
                    request,
                    color: Some(second_color),
                }),
                &env.ctx(),
            );
            assert_fresh_scene(&mode, &env, &out);
            let info = mode.search_info().unwrap();
            assert_eq!(info.field_count(), 4);
            let joined = [
                "alpha\nbravo",
                "alpha · button\nbravo · button",
                "40, 112\n240, 112",
                "#010203\n#040506",
            ];
            let current = ["bravo", "bravo · button", "240, 112", "#040506"];
            let values = info.values();
            let preview = info.preview_values();
            for field in 0..4 {
                let expected = if mask & (1 << field) != 0 {
                    joined[field]
                } else {
                    current[field]
                };
                assert_eq!(values[field], expected, "mask={mask}, field={field}");
                assert_eq!(preview[field], expected);
                assert_eq!(info.copy_value(field), expected);
            }
            let has_swatch = scene_of(&out).shapes.iter().any(|shape| {
                matches!(shape, crate::api::OverlayShape::Rect { fill, .. } if *fill == second_color)
            });
            assert_eq!(has_swatch, mask & 8 == 0, "mask={mask}");
        }
    }

    #[test]
    fn point_joined_color_failures_finish_without_copying_an_empty_field() {
        use crate::api::point_sample::{FieldMode, Sample};
        let mut config = Config::default();
        config.ui_hint.search_point.field_modes[3] = FieldMode::Concat;
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, point_text_targets());
        press(&mut mode, &env, "/");
        let mut request =
            sample_request(&mode.handle(&ModeEvent::TextChanged("alpha bravo".into()), &env.ctx()));
        mode.handle(&ModeEvent::CopyTextField(3), &env.ctx());
        for _ in 0..2 {
            request = sample_request(&mode.handle(
                &ModeEvent::PointSampled(Sample {
                    request,
                    color: None,
                }),
                &env.ctx(),
            ));
        }
        let out = mode.handle(
            &ModeEvent::PointSampled(Sample {
                request,
                color: None,
            }),
            &env.ctx(),
        );
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::CopyText(_) | Command::SamplePoint(_)))
        );
        assert_eq!(mode.search_info().unwrap().values()[3], "");
        assert!(!mode.inspection.as_ref().unwrap().copy_pending);
    }

    #[test]
    fn failed_unique_sample_clears_pending_copy_and_can_retry() {
        use crate::api::point_sample::Sample;
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, point_text_targets());
        press(&mut mode, &env, "/");
        let request =
            sample_request(&mode.handle(&ModeEvent::TextChanged("alpha".into()), &env.ctx()));
        let copy = mode.handle(&ModeEvent::CopyTextField(3), &env.ctx());
        assert!(
            !copy
                .iter()
                .any(|command| matches!(command, Command::SamplePoint(_) | Command::CopyText(_)))
        );
        let out = mode.handle(
            &ModeEvent::PointSampled(Sample {
                request,
                color: None,
            }),
            &env.ctx(),
        );
        assert!(out.is_empty());
        assert!(!mode.inspection.as_ref().unwrap().copy_pending);
        let retry = sample_request(&mode.handle(&ModeEvent::CopyTextField(3), &env.ctx()));
        let out = mode.handle(
            &ModeEvent::PointSampled(Sample {
                request: retry,
                color: Some(Color::rgb(4, 5, 6)),
            }),
            &env.ctx(),
        );
        assert!(out.contains(&Command::CopyText("#040506".into())));
        assert!(!mode.inspection.as_ref().unwrap().copy_pending);
    }
    #[test]
    #[ignore = "allocation measurement; run alone with --test-threads=1"]
    fn point_warmed_idle_frames_and_pending_samples_reuse_collection_storage() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            (0..320)
                .map(|i| {
                    let mut item = target(&format!("item{i}"), 0.0);
                    item.rect =
                        Rect::new((i % 20) as f64 * 45.0, (i / 20) as f64 * 40.0, 20.0, 20.0);
                    item
                })
                .collect(),
        );
        let query = mode
            .session
            .hints
            .iter()
            .map(|hint| format!("@{}", hint.label.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        press(&mut mode, &env, "/");
        mode.handle(&ModeEvent::TextChanged(query), &env.ctx());
        mode.handle(&ModeEvent::TogglePointAdjustment, &env.ctx());
        let members = mode.inspection.as_ref().unwrap().members.as_ptr();
        assert_eq!(mode.inspection.as_ref().unwrap().members.len(), 320);
        let events = [
            ModeEvent::Frame {
                elapsed: Duration::from_millis(16),
            },
            ModeEvent::Timer {
                id: point::TIMER.into(),
                elapsed: Duration::from_millis(100),
            },
            ModeEvent::PointSampled(crate::api::point_sample::Sample {
                request: crate::api::point_sample::Request {
                    id: 0,
                    point: Point::default(),
                },
                color: None,
            }),
        ];
        for event in &events {
            mode.handle(event, &env.ctx());
        }
        let region = stats_alloc::Region::new(crate::TEST_ALLOCATOR);
        for _ in 0..1000 {
            for event in &events {
                mode.handle(event, &env.ctx());
            }
        }
        let stats = region.change();
        assert_eq!(mode.inspection.as_ref().unwrap().members.as_ptr(), members);
        assert_eq!(stats.allocations + stats.reallocations, 0, "{stats:?}");
        println!("320 selected targets, 3000 idle/pending events: {stats:?}");
    }

    #[test]
    fn point_counter_counts_resolved_labels_instead_of_search_candidates() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            (0..155)
                .map(|i| {
                    let mut target = target(&format!("shared item{i}"), (i % 15) as f64 * 40.0);
                    target.rect.y = (i / 15) as f64 * 35.0 + 100.0;
                    target
                })
                .collect(),
        );
        let labels: Vec<_> = [2, 0]
            .into_iter()
            .map(|index| {
                mode.session
                    .hints
                    .iter()
                    .find(|hint| hint.value == index)
                    .unwrap()
                    .label
                    .clone()
            })
            .collect();
        press(&mut mode, &env, "/");
        mode.handle(
            &ModeEvent::TextChanged(format!(
                "{} s {} {} @ missing",
                labels[0].as_str(),
                labels[1].as_str(),
                labels[0].as_str()
            )),
            &env.ctx(),
        );
        assert_eq!(
            mode.session.search_matches.len(),
            155,
            "s still previews all possible matches"
        );
        assert_eq!(
            mode.session
                .search_selected
                .iter()
                .map(|hint| hint.value)
                .collect::<Vec<_>>(),
            [2, 0]
        );
        assert_eq!(mode.inspection.as_ref().unwrap().members.len(), 2);
        mode.handle(&ModeEvent::TogglePointAdjustment, &env.ctx());
        assert!(mode.scene(&env.ctx()).labels.iter().any(|label| {
            crate::api::overlay::split_trailing_text(&label.text, label.trailing_text_len).1
                == "0/2"
        }));
        for (position, target) in [(1, 2), (2, 0), (1, 2)] {
            mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
            assert_eq!(mode.inspection.as_ref().unwrap().position, position);
            assert_eq!(
                mode.inspection.as_ref().unwrap().point,
                mode.target_point(target)
            );
        }
        mode.handle(&ModeEvent::TogglePointAdjustment, &env.ctx());
        mode.handle(&ModeEvent::TextChanged("s missing".into()), &env.ctx());
        assert_eq!(mode.session.search_matches.len(), 155);
        assert!(mode.session.search_selected.is_empty());
        assert_eq!(mode.inspection.as_ref().unwrap().members.len(), 1);
        assert!(mode.search_info().is_some());
    }

    #[test]
    fn point_collection_uses_mean_destinations_and_cycles_in_query_order() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let rectangles = [
            Rect::new(0.0, 100.0, 100.0, 20.0),
            Rect::new(300.0, 200.0, 400.0, 20.0),
            Rect::new(800.0, 500.0, 20.0, 20.0),
        ];
        let targets = rectangles
            .iter()
            .enumerate()
            .map(|(i, rect)| UiTarget {
                rect: *rect,
                name: format!("item{i}"),
                role: SemanticRole::Button,
                details: Some(Box::new(crate::api::geometry::UiTargetDetails {
                    ocr: format!("text{i}"),
                    accessibility: format!("item{i}"),
                    color: None,
                })),
            })
            .collect();
        deliver(&mut mode, &env, targets);
        let labels: Vec<_> = (0..3)
            .map(|i| {
                mode.session
                    .hints
                    .iter()
                    .find(|h| h.value == i)
                    .unwrap()
                    .label
                    .clone()
            })
            .collect();
        let query = format!(
            "@{} @{} @{} @{}",
            labels[2].as_str(),
            labels[0].as_str(),
            labels[1].as_str(),
            labels[2].as_str()
        );
        press(&mut mode, &env, "/");
        mode.handle(&ModeEvent::TextChanged(query.clone()), &env.ctx());
        let point = mode.inspection.as_ref().unwrap();
        assert_eq!(
            point.members.len(),
            3,
            "duplicates do not weight the center twice"
        );
        assert!((point.point.x - (50.0 + 500.0 + 810.0) / 3.0).abs() < 0.001);
        assert!((point.point.y - (110.0 + 210.0 + 510.0) / 3.0).abs() < 0.001);
        assert_eq!(
            mode.search_info().unwrap().values()[0],
            "text2\ntext0\ntext1"
        );
        mode.handle(&ModeEvent::TogglePointAdjustment, &env.ctx());
        let old_request = crate::api::point_sample::Request {
            id: mode.sample_serial,
            point: mode.inspection.as_ref().unwrap().point,
        };
        for (position, index) in [(1, 2), (2, 0), (3, 1), (1, 2)] {
            let out = mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
            let point = mode.inspection.as_ref().unwrap();
            assert_eq!(point.position, position);
            assert_eq!(point.point, mode.target_point(index));
            let destination = mode.target_point(index);
            assert_eq!(
                mode.search_info().unwrap().values()[2],
                format!("{:.0}, {:.0}", destination.x, destination.y)
            );
            assert_eq!(point.color, None);
            assert!(out.iter().any(
                |c| matches!(c, Command::SamplePoint(r) if r.point == mode.target_point(index))
            ));
            assert_eq!(
                mode.search_info().unwrap().values()[0],
                "text2\ntext0\ntext1"
            );
            assert!(mode.scene(&env.ctx()).labels.iter().any(|l| {
                crate::api::overlay::split_trailing_text(&l.text, l.trailing_text_len).1
                    == format!("{position}/3")
            }));
        }
        mode.handle(
            &ModeEvent::PointSampled(crate::api::point_sample::Sample {
                request: old_request,
                color: Some(Color::rgb(255, 0, 0)),
            }),
            &env.ctx(),
        );
        assert_eq!(
            mode.inspection.as_ref().unwrap().color,
            None,
            "old center samples cannot color a member"
        );
        let selected = mode.inspection.as_ref().unwrap().point;
        let out = mode.handle(&ModeEvent::TextCopied, &env.ctx());
        assert!(out.contains(&Command::warp_to(selected)));
        assert!(mode.inspection.is_none());
        assert!(matches!(mode.input, Input::Labels(_)));
    }

    #[test]
    fn long_point_input_stays_left_aligned_with_a_bounded_dot_and_tail() {
        for scale in [1.0, 1.5, 2.0] {
            let mut config = Config::default();
            config.ui_hint.search_input_ui.width = 180;
            let mut env = Env::with(config);
            env.screens[0].scale = scale;
            let mut mode = crate::app::mode_catalog::hint(&env.config);
            activate(&mut mode, &env);
            let targets = ["alpha", "bravo"]
                .into_iter()
                .enumerate()
                .map(|(i, name)| {
                    let mut item = target(name, i as f64 * 200.0);
                    item.details = Some(Box::new(crate::api::geometry::UiTargetDetails {
                        ocr: name.into(),
                        accessibility: name.into(),
                        color: None,
                    }));
                    item
                })
                .collect();
            deliver(&mut mode, &env, targets);
            press(&mut mode, &env, "/");
            let query = "alpha ".repeat(250) + "bravo";
            mode.handle(&ModeEvent::TextChanged(query.clone()), &env.ctx());
            let before = mode.scene(&env.ctx());
            let input = before.labels.iter().find(|l| l.edit.is_some()).unwrap();
            mode.handle(&ModeEvent::TogglePointAdjustment, &env.ctx());
            let scene = mode.scene(&env.ctx());
            let adjusted = scene.labels.iter().find(|l| l.z_index == 10_002).unwrap();
            assert_eq!(
                adjusted.style.text_alignment,
                crate::api::overlay::TextAlignment::Left
            );
            assert_eq!(
                (adjusted.rect.x, adjusted.rect.y),
                (input.rect.x, input.rect.y)
            );
            assert_eq!(input.text.as_str(), query);
            let (visible_query, counter) = crate::api::overlay::split_trailing_text(
                &adjusted.text,
                adjusted.trailing_text_len,
            );
            assert_eq!(visible_query, query);
            assert_eq!(counter, "0/2");
            assert!(input.scroll_to_cursor);
            assert!(adjusted.scroll_to_cursor);
            assert!(adjusted.edit.is_none());
            assert!(
                !scene
                    .labels
                    .iter()
                    .any(|l| l.text.as_str().contains("Point"))
            );
            let dot = scene
                .shapes
                .iter()
                .find_map(|shape| match shape {
                    OverlayShape::Rect {
                        rect,
                        z_index: 10_004,
                        ..
                    } => Some(*rect),
                    _ => None,
                })
                .unwrap();
            assert!(adjusted.rect.right() < dot.x);
            assert!(dot.x - adjusted.rect.right() <= adjusted.style.font_size * scale * 0.31);
            let panel = scene
                .shapes
                .iter()
                .find_map(|shape| match shape {
                    OverlayShape::Rect {
                        rect,
                        z_index: 10_000,
                        ..
                    } => Some(*rect),
                    _ => None,
                })
                .unwrap();
            assert!((dot.x + dot.width - panel.right() + adjusted.rect.x - panel.x).abs() < 0.001);
            assert_eq!(mode.search_info().unwrap().values()[0], "alpha\nbravo");
            assert_eq!(mode.inspection.as_ref().unwrap().members.len(), 2);
        }
    }

    #[test]
    fn point_starts_at_label_jump_destination_independent_of_label_appearance() {
        for (font, scale, offset) in [(12, 1.0, 0), (28, 1.5, 40), (48, 2.0, -30)] {
            let mut config = Config::default();
            config.ui_hint.ui.font_size = font;
            config.ui_hint.label_x_offset = offset;
            config.ui_hint.label_y_offset = -offset;
            let mut env = Env::with(config);
            env.screens[0].scale = scale;
            let mut mode = crate::app::mode_catalog::hint(&env.config);
            activate(&mut mode, &env);
            let mut row = target("Unique", 20.0);
            row.rect.width = 900.0;
            deliver(&mut mode, &env, vec![row, target("Other", 950.0)]);
            let label = mode
                .session
                .hints
                .iter()
                .find(|h| h.value == 0)
                .unwrap()
                .label
                .clone();
            press(&mut mode, &env, "/");
            mode.handle(
                &ModeEvent::TextChanged(format!("@{}", label.as_str())),
                &env.ctx(),
            );
            let point = mode.inspection.as_ref().unwrap().point;
            assert_eq!(point, Point::new(470.0, 112.0));
            press(&mut mode, &env, "esc");
            let mut commands = Vec::new();
            for key in label.as_str().chars() {
                commands.extend(press(&mut mode, &env, &key.to_string()));
            }
            assert!(commands.contains(&Command::warp_to(point)));
        }
    }

    #[test]
    fn expected_scan_activation_does_not_restart_but_external_focus_does() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let id = mode.session.scan_id;
        mode.handle(
            &ModeEvent::UiScanActivationExpected {
                id: id.wrapping_sub(1),
                process_id: 7,
            },
            &env.ctx(),
        );
        assert_eq!(mode.session.expected_activation, None);
        mode.handle(
            &ModeEvent::UiScanActivationExpected { id, process_id: 7 },
            &env.ctx(),
        );
        let focus = |process_id| {
            ModeEvent::FocusChanged(Some(crate::api::FocusedApp {
                process_id,
                bundle_id: "example".into(),
                window_title: String::new(),
            }))
        };
        assert!(mode.handle(&focus(7), &env.ctx()).is_empty());
        assert_eq!(mode.session.scan_id, id);
        assert_eq!(mode.session.expected_activation, None);
        let commands = mode.handle(&focus(9), &env.ctx());
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::ScanUi(_)))
        );
        assert_ne!(mode.session.scan_id, id);
    }

    #[test]
    fn search_enter_commits_the_visible_result_and_reuses_session_index() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![target("复制文件", 0.0), target("粘贴文件", 200.0)],
        );
        let original: Vec<_> = mode
            .session
            .hints
            .iter()
            .map(|h| (h.label.clone(), h.value))
            .collect();
        let index = mode.session.search_text.as_ptr();
        for query in ["文件", "no such target", ""] {
            press(&mut mode, &env, "/");
            mode.handle(&ModeEvent::TextChanged(query.into()), &env.ctx());
            let out = press(&mut mode, &env, "enter");
            assert!(
                !out.iter()
                    .any(|c| matches!(c, Command::FinishMode { .. } | Command::ScanUi(_)))
            );
            assert_eq!(
                out.iter().any(|c| matches!(c, Command::WarpPointer { .. })),
                query == "文件"
            );
            assert_eq!(
                mode.session
                    .hints
                    .iter()
                    .map(|h| (h.label.clone(), h.value))
                    .collect::<Vec<_>>(),
                original
            );
            assert_eq!(mode.session.search_text.as_ptr(), index);
            assert!(matches!(mode.input, Input::Labels(_)));
        }
        press(&mut mode, &env, "/");
        let filtered = mode.handle(&ModeEvent::TextChanged("fzwj".into()), &env.ctx());
        assert_eq!(mode.session.hints.len(), 1);
        assert!(
            !filtered
                .iter()
                .any(|c| matches!(c, Command::FinishMode { .. }))
        );
        assert!(matches!(mode.input, Input::Search(_)));
        let selected = press(&mut mode, &env, "enter");
        assert!(
            selected
                .iter()
                .any(|c| matches!(c, Command::WarpPointer { .. }))
        );
        assert!(
            !selected
                .iter()
                .any(|c| matches!(c, Command::FinishMode { .. }))
        );
        assert!(!mode.session.finished);
        assert_eq!(mode.session.hints.len(), 2);
        // Live acceptance retains the query allocation, while an external
        // submission may carry a value not seen in the last edit event.
        for event in [
            ModeEvent::TextEdit(crate::api::text_edit::EditAction::Accept),
            ModeEvent::TextSubmitted(Some("fzwj".into())),
            ModeEvent::TextSubmitted(Some("ztwj".into())),
        ] {
            press(&mut mode, &env, "/");
            mode.handle(&ModeEvent::TextChanged("fzwj".into()), &env.ctx());
            let query_buffer = mode.input.text().as_ptr();
            let expected = if matches!(&event, ModeEvent::TextSubmitted(Some(text)) if text == "ztwj")
            {
                mode.session.scanned[1].rect.center()
            } else {
                mode.session.scanned[0].rect.center()
            };
            let commands = mode.handle(&event, &env.ctx());
            assert!(commands.iter().any(|command| matches!(
                command, Command::WarpPointer { x, y } if (*x, *y) == (expected.x, expected.y)
            )));
            assert_eq!(mode.session.search_query.as_ptr(), query_buffer);
            assert_eq!(mode.session.hints.len(), 2);
            assert!(mode.session.search_matches.is_empty());
            assert!(mode.session.search_hints.is_empty());
            assert!(!mode.session.finished);
        }
        mode.handle(&ModeEvent::Deactivated, &env.ctx());
        assert_eq!(mode.session.search_text.capacity(), 0);
        assert_eq!(mode.session.search_hints.capacity(), 0);
    }

    #[test]
    fn ambiguous_search_cycles_panels_accepts_current_result_and_resets_after_editing() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![target("alpha", 0.0), target("bravo", 100.0)],
        );
        press(&mut mode, &env, "/");
        let initial = mode.handle(&ModeEvent::TextChanged("button".into()), &env.ctx());
        assert_fresh_scene(&mode, &env, &initial);
        assert_eq!(mode.search_info().unwrap().hints[0].value, 0);
        assert_eq!(mode.session.search_matches.len(), 2);
        let labels: Vec<_> = mode
            .session
            .search_hints
            .iter()
            .map(|hint| hint.label.clone())
            .collect();
        let mut previous_sample = Some(sample_request(&initial));
        for expected in [1, 0, 1] {
            let out = mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
            assert_fresh_scene(&mode, &env, &out);
            assert_eq!(mode.search_info().unwrap().hints[0].value, expected);
            assert_eq!(
                mode.inspection.as_ref().unwrap().point,
                mode.target_point(expected)
            );
            assert!(!mode.inspection.as_ref().unwrap().adjusting);
            let request = sample_request(&out);
            if let Some(stale) = previous_sample {
                assert!(
                    mode.handle(
                        &ModeEvent::PointSampled(crate::api::point_sample::Sample {
                            request: stale,
                            color: Some(Color::rgb(1, 2, 3)),
                        }),
                        &env.ctx()
                    )
                    .is_empty()
                );
            }
            previous_sample = Some(request);
            assert!(
                !out.iter()
                    .any(|command| matches!(command, Command::WarpPointer { .. }))
            );
        }
        let accepted = mode.handle(
            &ModeEvent::TextEdit(crate::api::text_edit::EditAction::Accept),
            &env.ctx(),
        );
        assert!(accepted.contains(&Command::warp_to(mode.target_point(1))));
        assert_eq!(
            mode.session
                .hints
                .iter()
                .map(|hint| hint.label.clone())
                .collect::<Vec<_>>(),
            labels
        );
        press(&mut mode, &env, "/");
        mode.handle(&ModeEvent::TextChanged("button".into()), &env.ctx());
        mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
        let changed = mode.handle(&ModeEvent::TextInserted('x'), &env.ctx());
        assert!(mode.session.search_focus.is_none());
        assert!(mode.search_info().is_none());
        assert!(changed.contains(&Command::CancelPointSample));
        assert!(
            mode.handle(&ModeEvent::CyclePointTarget, &env.ctx())
                .is_empty()
        );
        mode.handle(
            &ModeEvent::TextEdit(crate::api::text_edit::EditAction::SelectAll),
            &env.ctx(),
        );
        mode.handle(&ModeEvent::TextPasted("button".into()), &env.ctx());
        assert_eq!(mode.search_info().unwrap().hints[0].value, 0);
        mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
        assert_eq!(mode.search_info().unwrap().hints[0].value, 1);
    }

    #[test]
    fn search_cycles_respect_configured_match_groups_and_label_only_queries() {
        use crate::api::hint::SearchMatchKind::{Label, Pinyin, Text};
        for (priority, expected) in [
            ([Label, Text, Pinyin], [0, 2, 1]),
            ([Label, Pinyin, Text], [0, 1, 2]),
            ([Pinyin, Text, Label], [1, 2, 0]),
            ([Pinyin, Label, Text], [1, 0, 2]),
            ([Text, Label, Pinyin], [2, 0, 1]),
            ([Text, Pinyin, Label], [2, 1, 0]),
        ] {
            let env = Env::new();
            let mut config = env.config.clone();
            config.ui_hint.search_match_priority = priority;
            let mut mode = crate::app::mode_catalog::hint(&config);
            activate(&mut mode, &env);
            deliver(
                &mut mode,
                &env,
                vec![
                    target("Other", 0.0),
                    target("设置", 100.0),
                    target("SZ command", 200.0),
                ],
            );
            for (hint, code) in
                mode.session
                    .hints
                    .iter_mut()
                    .zip([&b"sz"[..], &b"bbb"[..], &b"ccc"[..]])
            {
                hint.label = crate::api::hint::HintCode(smallvec::SmallVec::from_slice(code));
            }
            press(&mut mode, &env, "/");
            mode.handle(&ModeEvent::TextChanged("sz".into()), &env.ctx());
            assert_eq!(mode.search_info().unwrap().hints[0].value, expected[0]);
            for index in expected
                .into_iter()
                .skip(1)
                .chain(std::iter::once(expected[0]))
            {
                mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
                assert_eq!(mode.search_info().unwrap().hints[0].value, index);
            }
            for query in ["@sz", "sz@"] {
                mode.handle(&ModeEvent::TextChanged(query.into()), &env.ctx());
                assert_eq!(mode.session.search_matches.len(), 1);
                mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
                assert_eq!(mode.search_info().unwrap().hints[0].value, 0);
            }
        }
    }

    #[test]
    fn search_preview_prefers_quality_over_group_priority_and_preserves_it_on_streams() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![
                target("Other", 0.0),
                target("关于设置", 100.0),
                target("szextra", 200.0),
            ],
        );
        for (hint, code) in
            mode.session
                .hints
                .iter_mut()
                .zip([&b"sz"[..], &b"bbb"[..], &b"ccc"[..]])
        {
            hint.label = crate::api::hint::HintCode(smallvec::SmallVec::from_slice(code));
        }
        press(&mut mode, &env, "/");
        mode.handle(&ModeEvent::TextChanged("sz".into()), &env.ctx());
        // An exact label beats text prefixes and pinyin substrings, despite
        // labels being the lowest-priority group by default.
        assert_eq!(mode.search_info().unwrap().hints[0].value, 0);
        for expected in [2, 1, 0] {
            mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
            assert_eq!(mode.search_info().unwrap().hints[0].value, expected);
        }
        // Editing reselects the best result. A newly streamed complete pinyin
        // word can then become the automatic preview before any Tab press.
        mode.handle(&ModeEvent::TextChanged("missing".into()), &env.ctx());
        mode.handle(&ModeEvent::TextChanged("sz".into()), &env.ctx());
        let id = mode.session.scan_id;
        mode.handle_owned(
            ModeEvent::UiScanned(UiScanResult {
                id,
                targets: vec![target("设置", 300.0)],
                retired: vec![],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert_eq!(
            mode.session.scanned[mode.search_info().unwrap().hints[0].value].name,
            "设置"
        );
    }

    #[test]
    fn search_preview_survives_streaming_and_clears_if_the_result_is_retired() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![target("alpha", 0.0), target("bravo", 100.0)],
        );
        press(&mut mode, &env, "/");
        mode.handle(&ModeEvent::TextChanged("button".into()), &env.ctx());
        mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
        let focused = mode.search_info().unwrap().hints[0].label.clone();
        let rect = mode.session.scanned[1].rect;
        let scan_id = mode.session.scan_id;
        mode.handle_owned(
            ModeEvent::UiScanned(UiScanResult {
                id: scan_id,
                targets: vec![target("charlie", 200.0)],
                retired: vec![],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert_eq!(mode.search_info().unwrap().hints[0].label, focused);
        mode.handle_owned(
            ModeEvent::UiScanned(UiScanResult {
                id: scan_id,
                targets: vec![],
                retired: vec![rect],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert!(mode.session.search_focus.is_none());
        assert_eq!(
            mode.session.scanned[mode.search_info().unwrap().hints[0].value].name,
            "alpha"
        );
        assert!(mode.inspection.is_some());
        mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
        assert_eq!(
            mode.session.scanned[mode.search_info().unwrap().hints[0].value].name,
            "charlie"
        );
    }

    #[test]
    fn search_preview_keeps_identity_when_full_view_indices_shift() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![
                target("alpha", 0.0),
                target("bravo", 100.0),
                target("charlie", 200.0),
            ],
        );
        press(&mut mode, &env, "/");
        mode.handle(&ModeEvent::TextChanged("button ".into()), &env.ctx());
        mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
        let focused = mode.search_info().unwrap().hints[0].label.clone();
        let retired = mode.session.scanned[0].rect;
        let id = mode.session.scan_id;
        let out = mode.handle_owned(
            ModeEvent::UiScanned(UiScanResult {
                id,
                targets: vec![],
                retired: vec![retired],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert_fresh_scene(&mode, &env, &out);
        let preview = mode.search_info().unwrap().hints[0].value;
        assert_eq!(mode.session.scanned[preview].name, "bravo");
        assert_eq!(mode.search_info().unwrap().hints[0].label, focused);
        mode.handle(&ModeEvent::CyclePointTarget, &env.ctx());
        let preview = mode.search_info().unwrap().hints[0].value;
        assert_eq!(mode.session.scanned[preview].name, "charlie");
    }

    #[test]
    fn explicit_label_search_excludes_semantic_matches_and_unions_in_order() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![target("Save", 0.0), target("Language", 200.0)],
        );
        // Reproduce an existing label colliding with another target's text.
        mode.session.hints[0].label =
            crate::api::hint::HintCode(smallvec::SmallVec::from_slice(b"la"));
        mode.session.hints[1].label =
            crate::api::hint::HintCode(smallvec::SmallVec::from_slice(b"ka"));
        press(&mut mode, &env, "/");
        for c in ['@', 'l', 'a'] {
            mode.handle(&ModeEvent::TextInserted(c), &env.ctx());
            assert_eq!(mode.session.hints.len(), if c == '@' { 2 } else { 1 });
            assert_eq!(mode.session.search_selected.len(), usize::from(c == 'a'));
            assert_eq!(mode.inspection.is_some(), c != '@');
            assert!(mode.overlap_plan.is_ready());
            assert_eq!(mode.overlap_plan.layer_count(), 0);
        }
        for _ in 0..2 {
            mode.handle(
                &ModeEvent::TextEdit(crate::api::text_edit::EditAction::Backspace),
                &env.ctx(),
            );
        }
        assert_eq!(mode.session.hints.len(), 2);
        assert!(mode.session.search_matches.is_empty());
        mode.handle(&ModeEvent::TextChanged("k".into()), &env.ctx());
        assert_eq!(mode.session.hints.len(), 1);
        assert!(mode.session.search_selected.is_empty());
        assert!(mode.inspection.is_some());
        mode.handle(&ModeEvent::TextChanged("la".into()), &env.ctx());
        assert_eq!(mode.session.hints.len(), 2);
        assert_eq!(mode.session.search_selected.len(), 1);
        assert_eq!(mode.session.search_selected[0].label.as_str(), "la");
        mode.handle(&ModeEvent::TextInserted('@'), &env.ctx());
        assert_eq!(mode.session.hints.len(), 1);
        assert_eq!(mode.session.hints[0].label.as_str(), "la");
        mode.handle(
            &ModeEvent::TextEdit(crate::api::text_edit::EditAction::Backspace),
            &env.ctx(),
        );
        assert_eq!(mode.session.hints.len(), 2);
        let out = mode.handle(&ModeEvent::TextChanged("@la".into()), &env.ctx());
        assert_eq!(mode.session.hints.len(), 1);
        assert_eq!(mode.session.hints[0].label.as_str(), "la");
        assert!(mode.overlap_plan.is_ready());
        assert_eq!(mode.overlap_plan.layer_info(0), None);
        assert_eq!(
            scene_of(&out)
                .labels
                .iter()
                .filter(|l| l.z_index < 10_000)
                .count(),
            1
        );
        mode.handle(&ModeEvent::TextChanged("@ka @".into()), &env.ctx());
        assert_eq!(
            mode.session.hints.len(),
            2,
            "show candidates for the next label"
        );
        assert_eq!(
            mode.session.search_matches.len(),
            1,
            "bare @ must not select all"
        );
        assert_eq!(
            mode.session.search_hints[mode.session.search_matches[0]]
                .label
                .as_str(),
            "ka"
        );
        mode.handle(&ModeEvent::TextChanged("l@ @ka".into()), &env.ctx());
        assert_eq!(mode.session.search_matches.len(), 2);
        assert_eq!(mode.session.search_selected.len(), 1);
        assert_eq!(mode.session.search_selected[0].label.as_str(), "ka");
        mode.handle(
            &ModeEvent::TextChanged("ka@ @la la@ language".into()),
            &env.ctx(),
        );
        assert_eq!(
            mode.session
                .search_matches
                .iter()
                .map(|&index| mode.session.search_hints[index].label.as_str())
                .collect::<Vec<_>>(),
            ["ka", "la"]
        );
        mode.handle(&ModeEvent::TextChanged("la missing".into()), &env.ctx());
        assert_eq!(
            mode.session.search_matches.len(),
            2,
            "semantic collisions remain preview candidates"
        );
        assert_eq!(
            mode.session
                .search_selected
                .iter()
                .map(|hint| hint.value)
                .collect::<Vec<_>>(),
            [0]
        );
        assert_eq!(mode.inspection.as_ref().unwrap().members.len(), 1);
    }

    #[test]
    fn filtered_search_preserves_full_label_plan_when_scan_updates_arrive() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![
                target("Unique", 0.0),
                target("Other", 200.0),
                target("Third", 400.0),
            ],
        );
        let original = mode.session.hints.clone();
        let plan = (mode.session.label_plan_count, mode.session.next_label_index);
        press(&mut mode, &env, "/");
        mode.handle(&ModeEvent::TextChanged("unique".into()), &env.ctx());
        assert_eq!(mode.session.hints.len(), 1);
        assert_eq!(
            (mode.session.label_plan_count, mode.session.next_label_index),
            plan
        );
        let out = deliver(&mut mode, &env, vec![target("Fourth", 600.0)]);
        for old in original {
            let current = mode
                .session
                .search_hints
                .iter()
                .find(|h| h.bounds == old.bounds)
                .unwrap();
            assert_eq!(current.label, old.label);
        }
        assert_eq!(mode.session.hints.len(), 1);
        assert!(mode.overlap_plan.is_ready());
        assert_eq!(mode.overlap_plan.layer_info(0), None);
        let visible: Vec<_> = scene_of(&out)
            .labels
            .iter()
            .filter(|l| l.z_index < 10_000)
            .collect();
        assert_eq!(visible.len(), 1);
        assert_eq!(
            visible[0].text.as_str(),
            mode.session.hints[0].label.as_str()
        );
    }

    #[test]
    fn search_preserves_label_codes_and_information_slot_identity() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let mut targets: Vec<_> = (0..30)
            .map(|i| target(&format!("项目 {i}"), i as f64 * 25.0))
            .collect();
        targets[7].details = Some(Box::new(crate::api::geometry::UiTargetDetails {
            ocr: "复制文本".into(),
            accessibility: "Copy".into(),
            color: None,
        }));
        deliver(&mut mode, &env, targets);
        let label = mode
            .session
            .hints
            .iter()
            .find(|h| h.value == 7)
            .unwrap()
            .label
            .clone();
        press(&mut mode, &env, "/");
        mode.handle(
            &ModeEvent::TextChanged(label.as_str().to_string()),
            &env.ctx(),
        );
        assert_eq!(mode.session.hints.len(), 1);
        assert_eq!(mode.session.hints[0].label, label);
        assert!(mode.session.selected.is_none());
        assert!(
            mode.handle(&ModeEvent::CopyTextField(0), &env.ctx())
                .contains(&Command::CopyText("复制文本".into()))
        );
        let sample = mode.handle(&ModeEvent::CopyTextField(3), &env.ctx());
        assert!(mode.inspection.as_ref().unwrap().copy_pending);
        assert!(
            !sample
                .iter()
                .any(|command| matches!(command, Command::CopyText(_)))
        );
        assert!(
            mode.handle(&ModeEvent::CopyTextField(4), &env.ctx())
                .is_empty()
        );
        mode.handle(&ModeEvent::TextChanged("fzwb".into()), &env.ctx());
        assert_eq!(mode.session.hints[0].value, 7);
        press(&mut mode, &env, "esc");
        assert_eq!(mode.session.hints.len(), 30);
    }

    #[test]
    fn search_details_form_two_columns_inside_the_panel_at_high_dpi() {
        for scale in [1.0, 1.5, 2.0] {
            let mut env = Env::new();
            env.screens[0].scale = scale;
            let mut mode = crate::app::mode_catalog::hint(&env.config);
            activate(&mut mode, &env);
            let original = "这是一段很长的 OCR 内容\n不能换行也不能越过面板边界 ".repeat(60);
            let mut item = target("复制", 200.0);
            item.details = Some(Box::new(crate::api::geometry::UiTargetDetails {
                ocr: original.clone(),
                accessibility: "复制".into(),
                color: None,
            }));
            deliver(&mut mode, &env, vec![item]);
            press(&mut mode, &env, "/");
            mode.handle(&ModeEvent::TextChanged("复制".into()), &env.ctx());
            let scene = mode.scene(&env.ctx());
            let labels: Vec<_> = scene
                .labels
                .iter()
                .filter(|label| label.fixed_bounds && label.z_index == 10_001)
                .collect();
            assert_eq!(labels.len(), 12);
            assert_eq!(labels[0].rect.y, labels[3].rect.y);
            assert!(labels[1].rect.right() < labels[3].rect.x);
            assert_eq!(labels[6].rect.y, labels[9].rect.y);
            assert_eq!(labels[0].text, "1");
            assert_eq!(labels[1].text, "OCR");
            assert!(labels[0].style.border_width > 0.0);
            assert!(labels[3].style.border_width > 0.0);
            assert!(labels[2].text.ends_with('…'));
            assert!(!labels[2].text.contains('\n'));
            assert!(
                mode.handle(&ModeEvent::CopyTextField(0), &env.ctx())
                    .contains(&Command::CopyText(
                        "这是一段很长的 OCR 内容\n不能换行也不能越过面板边界".repeat(60)
                    ))
            );
            let panel = scene
                .shapes
                .iter()
                .find_map(|shape| match shape {
                    OverlayShape::Rect {
                        rect,
                        z_index: 10_000,
                        ..
                    } => Some(*rect),
                    _ => None,
                })
                .unwrap();
            for label in labels {
                assert!(label.rect.x >= panel.x && label.rect.right() <= panel.right());
                assert!(label.rect.y >= panel.y && label.rect.bottom() <= panel.bottom());
            }
        }
    }
    #[test]
    fn search_shortcuts_default_to_ctrl_and_preserve_explicit_legacy_bindings() {
        let mut config = Config::default();
        let compiled = crate::app::mode_catalog::hint_settings(&config);
        assert_eq!(
            crate::api::input::display_key_chord(&compiled.search_copy_keys[0].to_string()),
            "CTRL+1"
        );
        assert_eq!(
            compiled.search_copy_keys[0],
            KeyChord::parse("ctrl+1").unwrap()
        );
        config.ui_hint.search_copy_keys = vec![
            "ctrl+shift+9".into(),
            "ctrl+2".into(),
            "ctrl+3".into(),
            "ctrl+4".into(),
        ];
        let compiled = crate::app::mode_catalog::hint_settings(&config);
        assert_eq!(
            compiled.search_copy_keys[0],
            KeyChord::parse("ctrl+shift+9").unwrap()
        );
    }

    #[test]
    #[ignore = "allocation measurement; run alone with --test-threads=1"]
    fn search_equivalent_queries_reuse_warmed_input_and_result_storage() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, vec![target("复制文字", 0.0)]);
        press(&mut mode, &env, "/");
        let events = [
            ModeEvent::TextChanged("复制".into()),
            ModeEvent::TextChanged("fzw".into()),
        ];
        for event in &events {
            mode.handle(event, &env.ctx());
        }
        let region = stats_alloc::Region::new(crate::TEST_ALLOCATOR);
        for _ in 0..100 {
            for event in &events {
                let ModeEvent::TextChanged(text) = event else {
                    unreachable!()
                };
                let Input::Search(query) = &mut mode.input else {
                    unreachable!()
                };
                query.clear();
                query.push_str(text);
                // Measure reusable query/filter storage independently of scene
                // ownership: overlay text now requires an actual repaint.
                assert!(!mode.relabel_with_refresh(&env.ctx(), false));
            }
        }
        let stats = region.change();
        assert_eq!(
            stats.allocations + stats.reallocations,
            0,
            "warmed query/index buffers must not allocate: {stats:?}"
        );
        println!("200 equivalent query/index updates excluding overlay redraw: {stats:?}");
    }

    #[test]
    #[ignore = "response comparison; run in release with --test-threads=1"]
    fn search_response_comparison_probe() {
        let env = Env::new();
        for count in [30, 300] {
            for queries in [["复制", "fz"], ["fz", "zt"]] {
                let mut mode = crate::app::mode_catalog::hint(&env.config);
                activate(&mut mode, &env);
                deliver(
                    &mut mode,
                    &env,
                    (0..count)
                        .map(|i| {
                            let mut item = target(if i % 2 == 0 { "复制" } else { "粘贴" }, 0.0);
                            item.rect = Rect::new(
                                (i % 20) as f64 * 45.0,
                                (i / 20) as f64 * 35.0,
                                20.0,
                                15.0,
                            );
                            item
                        })
                        .collect(),
                );
                press(&mut mode, &env, "/");
                assert_eq!(mode.session.scanned.len(), count);
                let events = queries.map(|q| ModeEvent::TextChanged(q.into()));
                for optimized in [false, true, true, false] {
                    let mut samples = Vec::new();
                    for round in 0..110 {
                        let start = std::time::Instant::now();
                        for event in &events {
                            if optimized {
                                std::hint::black_box(mode.handle(event, &env.ctx()));
                            } else if let ModeEvent::TextChanged(text) = event {
                                mode.input = Input::Search(text.clone());
                                mode.relabel(&env.ctx());
                                let mut commands = mode.redraw();
                                mode.finish_redraw(&env.ctx(), &mut commands);
                                std::hint::black_box(commands);
                            }
                        }
                        if round >= 10 {
                            samples.push(start.elapsed().as_nanos() / 2);
                        }
                    }
                    samples.sort_unstable();
                    println!(
                        "targets={count} queries={queries:?} optimized={optimized} p50={}ns p95={}ns",
                        samples[50], samples[95]
                    );
                }
            }
        }
    }

    #[test]
    fn space_searches_union_in_input_order_and_copy_returns_to_hints() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let targets = ["红苹果", "蓝香蕉", "灰葡萄"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                let mut target = target(name, index as f64 * 200.0);
                target.details = Some(Box::new(crate::api::geometry::UiTargetDetails {
                    ocr: name.into(),
                    accessibility: name.into(),
                    color: Some(Color::rgb(1, 2, 3)),
                }));
                target
            })
            .collect();
        deliver(&mut mode, &env, targets);
        press(&mut mode, &env, "/");
        mode.handle(&ModeEvent::TextChanged("lxj ".into()), &env.ctx());
        assert!(
            mode.handle(&ModeEvent::TextChanged("lxj ".into()), &env.ctx())
                .is_empty()
        );
        assert_eq!(
            mode.session.hints.len(),
            3,
            "empty next search restores candidates"
        );
        assert_eq!(mode.search_info().unwrap().values()[0], "蓝香蕉");
        mode.handle(&ModeEvent::TextChanged("lxj  hpg lxj".into()), &env.ctx());
        assert_eq!(
            mode.session
                .search_matches
                .iter()
                .map(|&index| mode.session.search_hints[index].value)
                .collect::<Vec<_>>(),
            [1, 0]
        );
        let info = mode.search_info().unwrap();
        assert_eq!(info.field_count(), 4);
        assert_eq!(info.values()[0], "蓝香蕉\n红苹果");
        assert_eq!(info.values()[2], "140, 112");
        assert!(
            !mode
                .handle(&ModeEvent::CopyTextField(3), &env.ctx())
                .iter()
                .any(|c| matches!(c, Command::CopyText(_)))
        );
        assert!(
            mode.handle(&ModeEvent::CopyTextField(0), &env.ctx())
                .contains(&Command::CopyText("蓝香蕉\n红苹果".into()))
        );
        let scene = mode.scene(&env.ctx());
        assert_eq!(
            scene
                .labels
                .iter()
                .filter(|label| label.fixed_bounds && label.z_index == 10_001)
                .count(),
            12
        );
        let capacity = mode.session.search_matches.capacity();
        let selected_capacity = mode.session.search_selected.capacity();
        let out = mode.handle(&ModeEvent::TextCopied, &env.ctx());
        assert!(out.contains(&Command::CloseTextPrompt));
        assert!(
            !out.iter()
                .any(|c| matches!(c, Command::SwitchMode(_) | Command::WarpPointer { .. }))
        );
        assert_eq!(mode.session.hints.len(), 3);
        assert!(matches!(mode.input, Input::Labels(_)));
        assert_eq!(mode.session.search_matches.capacity(), capacity);
        assert_eq!(mode.session.search_selected.capacity(), selected_capacity);
        press(&mut mode, &env, "/");
        mode.handle(&ModeEvent::TextChanged("hpg lxj".into()), &env.ctx());
        assert_eq!(mode.search_info().unwrap().values()[0], "红苹果\n蓝香蕉");
        mode.handle(&ModeEvent::TextChanged("hpg".into()), &env.ctx());
        assert_eq!(mode.search_info().unwrap().field_count(), 4);
        mode.handle(&ModeEvent::Deactivated, &env.ctx());
        assert_eq!(mode.session.search_matches.capacity(), 0);
        assert_eq!(mode.session.search_selected.capacity(), 0);
        assert_eq!(mode.session.search_seen.capacity(), 0);
        assert_eq!(mode.session.search_query.capacity(), 0);
    }
    use crate::api::overlay::{LabelStyle, OverlayLabel, OverlayScene, OverlayShape};
    use crate::api::style::AUTO;
    use crate::presentation::hint::layers::build_visual_layer_plan;
    use crate::presentation::hint::visually_stacked;
    impl HintMode {
        fn scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
            ctx.presenter.compose(self.view(), ctx)
        }
        fn resolved_hint_label_style(&self, palette: &Palette) -> LabelStyle {
            crate::presentation::hint::resolved_hint_label_style(&hint_style(&self.config), palette)
        }
    }
    fn placed_hint_rect(config: &Settings, hint: &CompactHint<usize>, style: &LabelStyle) -> Rect {
        crate::presentation::hint::placed_hint_rect(&hint_style(config), hint, style, None)
    }
    #[cfg(test)]
    fn rotate_overlapping_labels(labels: &mut [OverlayLabel], cycle: usize) {
        let placements: SmallVec<[(usize, Rect); INLINE_LABELS]> = labels
            .iter()
            .enumerate()
            .map(|(index, label)| (index, label.rect))
            .collect();
        let mut plan = VisualLayerPlan::default();
        let style = LabelStyle::default();
        build_visual_layer_plan(
            &placements,
            labels.len(),
            true,
            |left, right| visually_stacked(left, right, style.padding_x, style.padding_y),
            &mut plan,
        );
        if plan.layer_count() == 0 {
            return;
        }
        let selected_layer = cycle.wrapping_sub(1) % plan.layer_count();
        for (index, label) in labels.iter_mut().enumerate() {
            if plan.is_selected(index, selected_layer) {
                label.z_index = 3;
            }
        }
    }

    use crate::api::geometry::{Point, Screen};
    use crate::config::Config;
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
                presenter: &crate::presentation::COMPOSER,
                screens: &self.screens,
                cursor: self.cursor,
                focused_app: None,
                palette: &self.palette,
            }
        }
    }

    #[test]
    fn uniform_label_geometry_uses_retained_plan_after_stream_retirement_and_search() {
        let mut config = Config::default();
        config.ui_hint.hint_characters = "asdfghjkl".into();
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let values: Vec<_> = (0..74)
            .map(|i| target(&format!("Control {i}"), i as f64 * 10.0))
            .collect();
        let retired: Vec<_> = values[9..].iter().map(|v| v.rect).collect();
        mode.handle_owned(
            ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: vec![],
                id: mode.session.scan_id,
                targets: values,
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert_eq!(mode.uniform_label_chars(), Some(2));
        mode.handle_owned(
            ModeEvent::UiScanned(crate::api::UiScanResult {
                retired,
                id: mode.session.scan_id,
                targets: vec![],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert_eq!(mode.session.hints.len(), 9);
        assert_eq!(
            mode.uniform_label_chars(),
            Some(2),
            "retired labels do not regenerate the code space"
        );
        press(&mut mode, &env, "/");
        mode.handle(&ModeEvent::TextChanged("Control 0".into()), &env.ctx());
        assert_eq!(mode.uniform_label_chars(), Some(2));
        let View::Hints(mut view) = mode.view() else {
            panic!()
        };
        let optimized = view.scene(&env.ctx());
        view.content.uniform_label_chars = None;
        let reference = view.scene(&env.ctx());
        assert_eq!(optimized.labels.len(), reference.labels.len());
        for (actual, expected) in optimized.labels.iter().zip(&reference.labels) {
            assert_eq!(actual.rect, expected.rect);
            assert_eq!(actual.text, expected.text);
            assert_eq!(actual.z_index, expected.z_index);
        }
    }

    #[test]
    fn canonical_stack_scene_matches_general_ranks_after_filtering_and_cycling() {
        for scale in [1.0, 1.25] {
            let mut env = Env::new();
            env.screens[0].scale = scale;
            for count in [2, 24, 128, 254, 255, 512, 513] {
                let mut mode = crate::app::mode_catalog::hint(&env.config);
                activate(&mut mode, &env);
                deliver(
                    &mut mode,
                    &env,
                    (0..count)
                        .map(|index| target(&format!("Control {index}"), 100.0))
                        .collect(),
                );
                assert!(mode.overlap_plan.is_canonical_stack());
                let prefix = mode.session.hints[0].label.as_str()[..1].to_owned();
                mode.held_overlap_keys.push(Key::new("shift").unwrap());
                for prefix in ["", prefix.as_str(), "!", ""] {
                    mode.input = Input::Labels(prefix.into());
                    mode.refresh_overlap_plan(&env.ctx());
                    let placements: Vec<_> = mode
                        .session
                        .hints
                        .iter()
                        .enumerate()
                        .filter(|(_, hint)| mode.hint_is_visible(hint))
                        .map(|(index, hint)| (index, hint.bounds))
                        .collect();
                    let packed: Vec<_> = placements
                        .iter()
                        .map(|(index, _)| {
                            mode.overlap_plan
                                .layer_info(*index)
                                .map_or(u32::MAX, |(layer, depth)| {
                                    ((depth as u32) << 16) | layer as u32
                                })
                        })
                        .collect();
                    let mut general = VisualLayerPlan::default();
                    general.finish(&placements, count, &packed, mode.overlap_plan.layer_count());
                    assert!(!general.is_canonical_stack());
                    for cycle in [0, 1, count / 2, count - 1] {
                        mode.overlap_cycle = cycle;
                        let actual = mode.scene(&env.ctx());
                        std::mem::swap(&mut mode.overlap_plan, &mut general);
                        assert_eq!(
                            actual,
                            mode.scene(&env.ctx()),
                            "count={count} prefix={prefix} cycle={cycle}"
                        );
                        std::mem::swap(&mut mode.overlap_plan, &mut general);
                    }
                }
                mode.overlap_plan.clear();
                assert!(!mode.overlap_plan.is_canonical_stack());
                mode.refresh_overlap_plan(&env.ctx());
                mode.overlap_plan.finish_unstacked();
                assert!(!mode.overlap_plan.is_canonical_stack());
                mode.refresh_overlap_plan(&env.ctx());
                mode.overlap_plan.release_retained();
                assert!(!mode.overlap_plan.is_canonical_stack());
            }
        }
    }

    #[test]
    fn prepared_layout_matches_materialized_plan_at_capacity_boundaries() {
        use crate::presentation::hint::{visual_layer_rect, visual_layer_scale};

        for scale in [1.0, 1.25, 2.0] {
            let mut env = Env::new();
            env.screens[0].bounds = Rect::new(0.0, 0.0, 12_000.0, 12_000.0);
            env.screens[0].work_area = env.screens[0].bounds;
            env.screens[0].scale = scale;
            for count in [24, 64, 128, 511, 512, 513] {
                for late_overlap in [false, true] {
                    let mut mode = crate::app::mode_catalog::hint(&env.config);
                    activate(&mut mode, &env);
                    deliver(
                        &mut mode,
                        &env,
                        (0..count)
                            .map(|index| {
                                let position = if late_overlap && index + 1 == count {
                                    0
                                } else {
                                    index
                                };
                                UiTarget {
                                    details: None,
                                    rect: Rect::new(
                                        (position % 100) as f64 * 80.0,
                                        (position / 100) as f64 * 40.0,
                                        64.0,
                                        24.0,
                                    ),
                                    name: format!("Control {index}"),
                                    role: SemanticRole::Button,
                                }
                            })
                            .collect(),
                    );
                    assert_eq!(mode.session.hints.len(), count);
                    let prefix = mode.session.hints[0].label.as_str()[..1].to_owned();
                    // Filter away the overlap, then restore it on the same plan.
                    for prefix in ["", prefix.as_str(), "!", ""] {
                        mode.input = Input::Labels(prefix.into());
                        mode.refresh_overlap_plan(&env.ctx());
                        let style = mode.resolved_hint_label_style(&env.palette);
                        let visual_scale = visual_layer_scale(&env.ctx(), mode.session.scan_bounds);
                        let placements: Vec<_> = mode
                            .session
                            .hints
                            .iter()
                            .enumerate()
                            .filter(|(_, hint)| mode.hint_is_visible(hint))
                            .map(|(index, hint)| {
                                let rect = placed_hint_rect(&mode.config, hint, &style);
                                (index, visual_layer_rect(rect, visual_scale))
                            })
                            .collect();
                        let mut reference = VisualLayerPlan::default();
                        build_visual_layer_plan(
                            &placements,
                            count,
                            mode.uniform_label_chars().is_some(),
                            |a, b| {
                                visually_stacked(
                                    a,
                                    b,
                                    (style.padding_x * visual_scale).round(),
                                    (style.padding_y * visual_scale).round(),
                                )
                            },
                            &mut reference,
                        );
                        assert_eq!(mode.overlap_plan.layer_count(), reference.layer_count());
                        for index in 0..count {
                            assert_eq!(
                                mode.overlap_plan.layer_info(index),
                                reference.layer_info(index)
                            );
                        }
                        for cycle in [0, 1, 2] {
                            mode.overlap_cycle = cycle;
                            let actual = mode.scene(&env.ctx());
                            std::mem::swap(&mut mode.overlap_plan, &mut reference);
                            assert_eq!(actual, mode.scene(&env.ctx()));
                            std::mem::swap(&mut mode.overlap_plan, &mut reference);
                        }
                    }
                }
            }
        }
    }

    fn target(name: &str, x: f64) -> UiTarget {
        UiTarget {
            details: None,
            rect: Rect::new(x, 100.0, 80.0, 24.0),
            name: name.into(),
            role: SemanticRole::Button,
        }
    }

    fn activate(mode: &mut HintMode, env: &Env) -> Vec<Command> {
        mode.handle(&ModeEvent::Activated { previous: None }, &env.ctx())
            .into_iter()
            .collect()
    }

    fn deliver(mode: &mut HintMode, env: &Env, targets: Vec<UiTarget>) -> Vec<Command> {
        let commands: Vec<_> = mode
            .handle_owned(
                ModeEvent::UiScanned(crate::api::UiScanResult {
                    retired: Vec::new(),
                    id: mode.session.scan_id,
                    targets,
                    status: UiScanStatus::Success,
                }),
                &env.ctx(),
            )
            .into_iter()
            .collect();
        // Simulate successive Engine turns after the scene has been submitted.
        if commands.iter().any(|command| {
            matches!(command,
                Command::SetTimer { id, .. } if id == SEARCH_PREWARM_TIMER_ID
            )
        }) {
            let mut completed = false;
            for _ in 0..=mode.session.scanned.len() / SEARCH_PREWARM_TARGETS {
                if mode
                    .handle(
                        &ModeEvent::Timer {
                            id: SEARCH_PREWARM_TIMER_ID.into(),
                            elapsed: Duration::ZERO,
                        },
                        &env.ctx(),
                    )
                    .is_empty()
                {
                    completed = true;
                    break;
                }
            }
            assert!(completed, "prewarm must make bounded progress");
        }
        commands
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

    fn top_label(scene: &OverlayScene) -> &OverlayLabel {
        scene
            .labels
            .iter()
            .max_by_key(|label| label.z_index)
            .expect("expected at least one label")
    }

    fn top_layer_count(scene: &OverlayScene) -> usize {
        let top = top_label(scene).z_index;
        scene
            .labels
            .iter()
            .filter(|label| label.z_index == top)
            .count()
    }

    #[test]
    fn default_auto_padding_resolves_to_compact_hint_spacing() {
        let env = Env::new();
        let mode = crate::app::mode_catalog::hint(&env.config);
        let style = mode.resolved_hint_label_style(&env.palette);

        assert_eq!(mode.config.ui.padding_x, AUTO);
        assert_eq!(mode.config.ui.padding_y, AUTO);
        assert_eq!(style.font_size, 17.0);
        assert_eq!(style.padding_x, 2.0);
        assert_eq!(style.padding_y, 1.0);
    }

    #[test]
    fn semantic_refinement_keeps_labels_but_clicks_the_authoritative_control() {
        for highlight in [false, true] {
            let mut config = Config::default();
            config.ui_hint.boundary_highlight.enabled = highlight;
            let env = Env::with(config);
            let mut mode = crate::app::mode_catalog::hint(&env.config);
            activate(&mut mode, &env);
            let initial: Vec<_> = [0.0, 100.0, 200.0, 500.0]
                .into_iter()
                .map(|x| target("fragment", x))
                .collect();
            let retired = initial[..3].iter().map(|t| t.rect).collect();
            deliver(&mut mode, &env, initial);
            let code = mode.session.hints[1].label.clone();
            let anchor = mode.session.hints[1].bounds;
            let unaffected = mode.session.hints[3].label.clone();
            let row = UiTarget {
                details: None,
                rect: Rect::new(0.0, 90.0, 300.0, 44.0),
                name: "file row".into(),
                role: SemanticRole::ListItem,
            };
            let center = row.rect.center();
            mode.handle_owned(
                ModeEvent::UiScanned(UiScanResult {
                    id: mode.session.scan_id,
                    targets: vec![row.clone()],
                    retired,
                    status: UiScanStatus::Success,
                }),
                &env.ctx(),
            );
            assert_eq!(mode.session.hints.len(), 2);
            assert_eq!(mode.session.hints[0].label, unaffected);
            assert_eq!(mode.session.hints[1].label, code);
            assert_eq!(
                mode.session.hints[1].bounds,
                if highlight { row.rect } else { anchor }
            );
            press(&mut mode, &env, "/");
            mode.handle(
                &ModeEvent::TextChanged(format!("@{}", code.as_str())),
                &env.ctx(),
            );
            assert_eq!(
                mode.inspection.as_ref().unwrap().point,
                center,
                "Point uses the same authoritative row destination after refinement"
            );
            press(&mut mode, &env, "esc");
            let commands = press(&mut mode, &env, code.as_str());
            assert!(commands.iter().any(|command| matches!(command, Command::WarpPointer { x, y } if *x == center.x && *y == center.y)));
        }
    }

    #[test]
    fn explicit_hint_padding_still_overrides_compact_auto_spacing() {
        let mut config = Config::default();
        config.ui_hint.ui.padding_x = 7;
        config.ui_hint.ui.padding_y = 3;
        let env = Env::with(config);
        let mode = crate::app::mode_catalog::hint(&env.config);
        let style = mode.resolved_hint_label_style(&env.palette);

        assert_eq!(style.padding_x, 7.0);
        assert_eq!(style.padding_y, 3.0);
    }

    #[test]
    fn deactivation_releases_large_scan_buffers() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        mode.session.scanned.reserve(1_024);
        mode.session.search_text.reserve(1_024);
        mode.session.seen_targets.reserve(1_024);
        mode.session.hints.reserve(1_024);
        mode.overlap_plan.reserve_for_test(1_024);
        assert!(mode.session.scanned.capacity() >= 1_024);
        assert!(mode.session.search_text.capacity() >= 1_024);
        assert!(mode.session.seen_targets.capacity() >= 1_024);
        assert!(mode.session.hints.capacity() >= 1_024);
        assert!(mode.overlap_plan.retained_capacity() >= 1_024);

        mode.handle(&ModeEvent::Deactivated, &env.ctx());

        assert_eq!(mode.session.scanned.capacity(), 0);
        assert_eq!(mode.session.search_text.capacity(), 0);
        assert_eq!(mode.session.seen_targets.capacity(), 0);
        assert_eq!(mode.session.hints.capacity(), 0);
        assert_eq!(mode.overlap_plan.retained_capacity(), INLINE_LABELS);
    }

    #[test]
    fn deactivation_releases_small_container_capacity_too() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            (0..100)
                .map(|index| target(&format!("Target {index}"), index as f64 * 2.0))
                .collect(),
        );
        assert_eq!(mode.session.scanned.len(), 100);

        mode.handle(&ModeEvent::Deactivated, &env.ctx());

        assert!(mode.session.scanned.is_empty());
        assert_eq!(mode.session.scanned.capacity(), 0);
        assert_eq!(mode.session.search_text.capacity(), 0);
        assert!(mode.session.seen_targets.is_empty());
        assert_eq!(mode.session.seen_targets.capacity(), 0);
        assert!(mode.session.hints.is_empty());
        assert_eq!(mode.session.hints.capacity(), 0);
        assert!(mode.overlap_plan.retained_capacity() <= INLINE_LABELS);
    }

    #[test]
    fn late_context_change_cannot_restart_a_deactivated_hint_mode() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let retired_scan_id = mode.session.scan_id;

        mode.handle(&ModeEvent::Deactivated, &env.ctx());
        let out = mode.handle_owned(
            ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: Vec::new(),
                id: retired_scan_id,
                targets: Vec::new(),
                status: UiScanStatus::ContextChanged,
            }),
            &env.ctx(),
        );

        assert!(out.is_empty());
        assert!(!mode.session.active);
        assert!(!mode.session.scanning);
        assert_eq!(mode.session.scan_id, retired_scan_id);
        assert!(mode.session.scanned.is_empty());
        assert!(mode.session.hints.is_empty());
    }

    #[test]
    fn activation_requests_a_scan_with_configured_roles() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let old_scan_id = mode.session.scan_id;

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
        assert_eq!(mode.session.scan_bounds, Some(env.screens[1].bounds));
    }

    #[test]
    fn scan_results_produce_one_label_per_target() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
    fn search_prewarm_follows_first_scene_and_tolerates_early_search_and_exit() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        let warm = ModeEvent::Timer {
            id: SEARCH_PREWARM_TIMER_ID.into(),
            elapsed: Duration::ZERO,
        };
        for early_search in [false, true] {
            activate(&mut mode, &env);
            let commands = mode.handle_owned(
                ModeEvent::UiScanned(UiScanResult {
                    id: mode.session.scan_id,
                    targets: vec![target("Save", 0.0)],
                    retired: Vec::new(),
                    status: UiScanStatus::Partial,
                }),
                &env.ctx(),
            );
            assert!(matches!(commands[0], Command::ShowOverlay(_)));
            assert!(
                matches!(&commands[1], Command::SetTimer { id, delay, repeating: false }
                if id == SEARCH_PREWARM_TIMER_ID && delay.is_zero())
            );
            assert!(!mode.session.search_names_initialized);
            if early_search {
                press(&mut mode, &env, "/");
                press(&mut mode, &env, "s");
                assert_eq!(mode.session.hints.len(), 1);
            }
            assert!(mode.handle(&warm, &env.ctx()).is_empty());
            assert!(mode.session.search_names_initialized);
            let storage = mode.session.search_text.as_ptr();
            assert!(mode.handle(&warm, &env.ctx()).is_empty());
            assert_eq!(mode.session.search_text.as_ptr(), storage);
            mode.handle(&ModeEvent::Deactivated, &env.ctx());
            assert!(mode.handle(&warm, &env.ctx()).is_empty());
            assert_eq!(mode.session.search_text.capacity(), 0);
        }
    }

    #[test]
    fn bounded_prewarm_yields_to_scan_updates_and_empty_search() {
        let mut env = Env::new();
        env.screens[0].bounds = Rect::new(0.0, 0.0, 12_000.0, 12_000.0);
        env.screens[0].work_area = env.screens[0].bounds;
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let warm = ModeEvent::Timer {
            id: SEARCH_PREWARM_TIMER_ID.into(),
            elapsed: Duration::ZERO,
        };
        let values = (0..1_024)
            .map(|index| UiTarget {
                rect: Rect::new(
                    (index % 100) as f64 * 80.0,
                    (index / 100) as f64 * 40.0,
                    64.0,
                    24.0,
                ),
                name: format!("Control {index} 设置"),
                role: SemanticRole::Button,
                details: None,
            })
            .collect();
        let commands = mode.handle_owned(
            ModeEvent::UiScanned(UiScanResult {
                id: mode.session.scan_id,
                targets: values,
                retired: Vec::new(),
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert!(matches!(commands[0], Command::ShowOverlay(_)));
        assert_eq!(scene_of(&commands).labels.len(), 1_024);
        assert_eq!(mode.session.search_text.capacity(), 0);
        let continuation = mode.handle(&warm, &env.ctx());
        assert_eq!(mode.session.search_text.len(), SEARCH_PREWARM_TARGETS);
        assert!(!mode.session.search_names_initialized);
        assert!(
            matches!(&continuation[0], Command::SetTimer { id, delay, repeating: false }
            if id == SEARCH_PREWARM_TIMER_ID && delay.is_zero())
        );
        assert_eq!(continuation.len(), 1);
        let text_storage = mode.session.search_text.as_ptr();
        let commands = mode.handle_owned(
            ModeEvent::UiScanned(UiScanResult {
                id: mode.session.scan_id,
                targets: vec![target("Late unique", 8_500.0)],
                retired: Vec::new(),
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert_eq!(scene_of(&commands).labels.len(), 1_025);
        assert_eq!(mode.session.search_text.len(), SEARCH_PREWARM_TARGETS);
        assert_eq!(mode.session.search_text.as_ptr(), text_storage);
        press(&mut mode, &env, "/");
        assert_eq!(
            mode.session.search_text.len(),
            SEARCH_PREWARM_TARGETS,
            "empty search must preserve the bounded prewarm, not finish it synchronously"
        );
        mode.handle(&ModeEvent::TextChanged("unique".into()), &env.ctx());
        assert!(mode.session.search_names_initialized);
        assert_eq!(mode.session.search_text.len(), 1_025);
        assert_eq!(mode.session.hints.len(), 1);
        assert_eq!(
            mode.session.scanned[mode.session.hints[0].value].name,
            "Late unique"
        );
        assert!(mode.handle(&warm, &env.ctx()).is_empty());
        mode.handle(&ModeEvent::Deactivated, &env.ctx());
        assert!(mode.handle(&warm, &env.ctx()).is_empty());
        assert_eq!(mode.session.search_text.capacity(), 0);
    }

    #[test]
    fn prewarm_finishes_without_search_and_cancels_partial_work_on_exit() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        let warm = ModeEvent::Timer {
            id: SEARCH_PREWARM_TIMER_ID.into(),
            elapsed: Duration::ZERO,
        };
        for complete in [false, true] {
            activate(&mut mode, &env);
            // Coincident rectangles still represent distinct named targets.
            let values = (0..600)
                .map(|i| target(&format!("Control {i}"), 20.0))
                .collect();
            mode.handle_owned(
                ModeEvent::UiScanned(UiScanResult {
                    id: mode.session.scan_id,
                    targets: values,
                    retired: Vec::new(),
                    status: UiScanStatus::Success,
                }),
                &env.ctx(),
            );
            assert!(!mode.handle(&warm, &env.ctx()).is_empty());
            if complete {
                assert!(!mode.handle(&warm, &env.ctx()).is_empty());
                assert!(mode.handle(&warm, &env.ctx()).is_empty());
                assert!(mode.session.search_names_initialized);
                assert_eq!(mode.session.search_text.len(), 600);
                assert!(
                    matches!(mode.input, Input::Labels(_)),
                    "prepare before the user searches"
                );
            }
            let commands = mode.handle(&ModeEvent::Deactivated, &env.ctx());
            assert!(
                commands
                    .iter()
                    .any(|command| matches!(command, Command::CancelTimer { id }
                if id == SEARCH_PREWARM_TIMER_ID))
            );
            assert!(mode.handle(&warm, &env.ctx()).is_empty());
            assert_eq!(mode.session.search_text.capacity(), 0);
        }
    }

    #[test]
    fn partial_scan_batches_appear_immediately_and_remain_after_completion() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);

        let first = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id,
                targets: vec![target("First", 100.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert!(mode.session.scanning);
        assert_eq!(scene_of(&first).labels.len(), 1);

        let second = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id,
                targets: vec![target("Second", 100.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert_eq!(scene_of(&second).labels.len(), 2);
        assert!(mode.overlap_plan.is_ready());
        assert_eq!(mode.overlap_plan.len(), 2);
        assert_eq!(mode.overlap_plan.layer_count(), 2);
        assert_eq!(
            scene_of(&second)
                .labels
                .iter()
                .filter(|label| label.z_index == 3)
                .count(),
            1,
            "each merged Partial prepares the default visual layer"
        );

        let completed = deliver(&mut mode, &env, Vec::new());
        assert!(!mode.session.scanning);
        assert!(completed.is_empty(), "terminal reuses the prepared plan");
        assert_eq!(mode.session.hints.len(), 2);
        assert!(mode.overlap_plan.is_ready());
        assert_eq!(mode.overlap_plan.len(), 2);
        assert_eq!(mode.overlap_plan.layer_count(), 2);

        let cycled = press(&mut mode, &env, "left_shift");
        assert_eq!(
            scene_of(&cycled)
                .labels
                .iter()
                .filter(|label| label.z_index == 3)
                .count(),
            1
        );
        release(&mut mode, &env, "left_shift");
    }

    #[test]
    fn later_partial_rebuilds_the_merged_plan_instead_of_appending_layers() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);

        mode.handle(
            &ModeEvent::UiScanned(UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id,
                targets: vec![target("Vision one", 100.0), target("Vision two", 100.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert_eq!(mode.overlap_plan.layer_count(), 2);

        mode.handle(
            &ModeEvent::UiScanned(UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id,
                targets: vec![target("UIA one", 400.0), target("UIA two", 400.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );

        assert_eq!(mode.session.hints.len(), 4);
        assert_eq!(mode.overlap_plan.len(), 4);
        assert_eq!(
            mode.overlap_plan.layer_count(),
            2,
            "disconnected UIA/Vision stacks must reuse global layers"
        );
    }

    #[test]
    #[ignore = "microbenchmark probe; run in release with --test-threads=1"]
    fn unchanged_terminal_performance_probe() {
        const WARMUP: usize = 2_000;
        const SAMPLES: usize = 20_000;

        fn prepared(env: &Env) -> HintMode {
            let mut mode = crate::app::mode_catalog::hint(&env.config);
            activate(&mut mode, env);
            let scan_id = mode.session.scan_id;
            mode.handle_owned(
                ModeEvent::UiScanned(UiScanResult {
                    retired: Vec::new(),
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
        let scan_id = fast.session.scan_id;
        let no_op = measure(|| {
            std::hint::black_box(fast.handle_owned(
                ModeEvent::UiScanned(UiScanResult {
                    retired: Vec::new(),
                    id: scan_id,
                    targets: Vec::new(),
                    status: UiScanStatus::Success,
                }),
                &env.ctx(),
            ));
        });
        let mut legacy = prepared(&env);
        let redraw = measure(|| {
            let mut commands = legacy.redraw();
            legacy.finish_redraw(&env.ctx(), &mut commands);
            std::hint::black_box(commands);
        });
        println!(
            "hint_terminal_probe samples={SAMPLES} no_op_p50={}ns no_op_p95={}ns no_op_p99={}ns redraw_p50={}ns redraw_p95={}ns redraw_p99={}ns",
            no_op.0, no_op.1, no_op.2, redraw.0, redraw.1, redraw.2,
        );
    }

    #[test]
    fn partial_labels_can_be_selected_before_the_scan_finishes() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id,
                targets: vec![target("First", 100.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        let label = mode.session.hints[0].label.clone();
        let out = press(&mut mode, &env, label.as_str());
        assert!(
            out.iter()
                .any(|command| matches!(command, Command::WarpPointer { .. }))
        );
    }

    #[test]
    fn scan_results_outside_the_requested_screen_are_discarded() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let out = deliver(
            &mut mode,
            &env,
            vec![target("Current", 100.0), target("Other display", 1500.0)],
        );

        assert_eq!(mode.session.scanned.len(), 1);
        assert_eq!(mode.session.scanned[0].name, "Current");
        assert_eq!(scene_of(&out).labels.len(), 1);
        assert_eq!(scene_of(&out).clip, Some(env.screens[0].bounds));
    }

    #[test]
    fn typed_prefix_removes_other_labels_and_highlights_the_match() {
        let mut config = Config::default();
        config.ui_hint.boundary_highlight.enabled = true;
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let targets = (0..30)
            .map(|i| target(&format!("target {i}"), i as f64 * 25.0))
            .collect();
        deliver(&mut mode, &env, targets);
        let total = mode.session.hints.len();
        let prefix = mode
            .alphabet
            .iter()
            .copied()
            .find(|prefix| {
                let prefix = prefix.to_string();
                mode.session
                    .hints
                    .iter()
                    .filter(|hint| hint.label.as_str().starts_with(&prefix))
                    .count()
                    > 1
                    && mode
                        .session
                        .hints
                        .iter()
                        .all(|hint| hint.label.as_str() != prefix)
            })
            .expect("test data should produce a partial prefix");

        let out = press(&mut mode, &env, &prefix.to_string());
        let expected_next: std::collections::BTreeSet<_> = mode
            .session
            .hints
            .iter()
            .filter_map(|hint| hint.label.as_str().strip_prefix(prefix))
            .filter_map(|suffix| suffix.chars().next())
            .map(|key| key.to_string())
            .collect();
        let help_next: std::collections::BTreeSet<_> = mode
            .available_keys()
            .into_iter()
            .filter(|(_, action)| action == "select hint")
            .map(|(key, _)| key)
            .collect();
        assert_eq!(help_next, expected_next);
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
    fn typed_prefix_rebuilds_overlap_layers_for_visible_labels() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            (0..30)
                .map(|index| target(&format!("Target {index}"), 100.0))
                .collect(),
        );
        let total = mode.session.hints.len();
        let prefix = mode
            .alphabet
            .iter()
            .copied()
            .find(|prefix| {
                let prefix = prefix.to_string();
                mode.session
                    .hints
                    .iter()
                    .filter(|hint| hint.label.as_str().starts_with(&prefix))
                    .count()
                    > 1
                    && mode
                        .session
                        .hints
                        .iter()
                        .all(|hint| hint.label.as_str() != prefix)
            })
            .expect("test data should produce a partial prefix");

        let filtered = press(&mut mode, &env, &prefix.to_string());
        let visible = scene_of(&filtered).labels.len();
        assert!(visible > 1 && visible < total);
        assert!(mode.overlap_plan.is_ready());
        assert_eq!(mode.overlap_plan.len(), mode.session.hints.len());
        assert_eq!(mode.overlap_plan.layer_count(), visible);

        let raised_label = |output: &Vec<Command>| top_label(scene_of(output)).text.clone();
        let mut raised = HashSet::from([raised_label(&filtered)]);
        let cycled = press(&mut mode, &env, "left_shift");
        assert_eq!(scene_of(&cycled).labels.len(), visible);
        assert_eq!(top_layer_count(scene_of(&cycled)), 1);
        raised.insert(raised_label(&cycled));
        release(&mut mode, &env, "left_shift");
        for _ in 2..visible {
            let cycled = press(&mut mode, &env, "left_shift");
            raised.insert(raised_label(&cycled));
            release(&mut mode, &env, "left_shift");
        }
        assert_eq!(
            raised.len(),
            visible,
            "every layer must remain reachable after a Hint prefix"
        );

        let restored = press(&mut mode, &env, "backspace");
        assert_eq!(scene_of(&restored).labels.len(), total);
        assert_eq!(mode.overlap_plan.len(), mode.session.hints.len());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn high_dpi_ajh_and_ajj_switch_before_and_after_prefix_filtering() {
        let mut env = Env::new();
        env.screens[0].scale = 1.5;
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            (0..100)
                .map(|index| target(&format!("Target {index}"), 100.0))
                .collect(),
        );

        for (index, hint) in mode.session.hints.iter_mut().enumerate() {
            hint.bounds = Rect::new(1_000.0 + index as f64 * 100.0, 300.0, 20.0, 20.0);
        }
        let ajh = mode
            .session
            .hints
            .iter()
            .position(|hint| hint.label.as_str() == "ajh")
            .expect("100 labels should include ajh");
        let ajj = mode
            .session
            .hints
            .iter()
            .position(|hint| hint.label.as_str() == "ajj")
            .expect("100 labels should include ajj");
        let style = mode.resolved_hint_label_style(&env.palette);
        mode.session.hints[ajh].bounds = Rect::new(50.0, 100.0, 20.0, 20.0);
        let logical_ajh = placed_hint_rect(&mode.config, &mode.session.hints[ajh], &style);
        mode.session.hints[ajj].bounds =
            Rect::new(50.0 + logical_ajh.width * 1.10, 100.0, 20.0, 20.0);
        let logical_ajj = placed_hint_rect(&mode.config, &mode.session.hints[ajj], &style);
        assert!(
            logical_ajh.intersect(&logical_ajj).is_none(),
            "the old 96-DPI planner must miss this real-world overlap"
        );
        let rendered_ajh = crate::api::overlay::scaled_label_geometry(
            "ajh",
            logical_ajh,
            &style,
            env.screens[0].scale,
        )
        .0;
        let rendered_ajj = crate::api::overlay::scaled_label_geometry(
            "ajj",
            logical_ajj,
            &style,
            env.screens[0].scale,
        )
        .0;
        assert!(visually_stacked(
            rendered_ajh,
            rendered_ajj,
            (style.padding_x * env.screens[0].scale).round(),
            (style.padding_y * env.screens[0].scale).round(),
        ));

        mode.refresh_overlap_plan(&env.ctx());
        assert_eq!(mode.overlap_plan.component_layer_count(ajh), Some(2));
        assert_eq!(mode.overlap_plan.component_layer_count(ajj), Some(2));
        let default_scene = mode.scene(&env.ctx()).sorted();
        let z = |scene: &OverlayScene, text: &str| {
            scene
                .labels
                .iter()
                .find(|label| label.text == text)
                .map(|label| label.z_index)
                .expect("the pair must remain visible")
        };
        assert!(z(&default_scene, "ajj") > z(&default_scene, "ajh"));
        let shifted = press(&mut mode, &env, "left_shift");
        assert!(z(scene_of(&shifted), "ajh") > z(scene_of(&shifted), "ajj"));
        release(&mut mode, &env, "left_shift");

        press(&mut mode, &env, "a");
        assert_eq!(mode.overlap_plan.component_layer_count(ajh), Some(2));
        assert_eq!(mode.overlap_plan.component_layer_count(ajj), Some(2));

        let shifted = press(&mut mode, &env, "left_shift");
        let sorted = scene_of(&shifted).clone().sorted();
        let labels = &sorted.labels;
        let ajh_position = labels
            .iter()
            .position(|label| label.text == "ajh")
            .expect("ajh should remain visible");
        let ajj_position = labels
            .iter()
            .position(|label| label.text == "ajj")
            .expect("ajj should remain visible");
        assert!(labels[ajh_position].z_index > labels[ajj_position].z_index);
        assert!(
            ajh_position > ajj_position,
            "the Engine's stable z sort must draw the raised ajh after ajj"
        );
    }

    #[test]
    fn matched_prefix_color_defaults_inside_ui_hint_and_allows_an_override() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
        let initial_top = top_label(scene_of(&initial)).text.clone();

        let first = press(&mut mode, &env, "left_shift");
        let first_top = top_label(scene_of(&first)).text.clone();

        let restored = release(&mut mode, &env, "left_shift");
        assert_eq!(
            Some(top_label(scene_of(&restored)).text.clone()),
            Some(initial_top.clone())
        );

        let second = press(&mut mode, &env, "right_shift");
        let second_top = top_label(scene_of(&second)).text.clone();
        assert_ne!(first_top, second_top);

        release(&mut mode, &env, "right_shift");
        let third = press(&mut mode, &env, "left_shift");
        let third_top = top_label(scene_of(&third)).text.clone();
        assert_ne!(second_top, third_top);
        assert_ne!(third_top, initial_top);
        assert_eq!(third_top, first_top, "presses must cycle 1→2→1");
    }

    #[test]
    fn shift_cycles_every_computed_layer_instead_of_stopping_at_three() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let initial = deliver(
            &mut mode,
            &env,
            (0..5)
                .map(|index| target(&format!("Layer {index}"), 100.0))
                .collect(),
        );
        let top_text = |output: &[Command]| top_label(scene_of(output)).text.clone();
        let mut observed = vec![top_text(&initial)];
        for index in 0_usize..5 {
            let shift = if index.is_multiple_of(2) {
                "left_shift"
            } else {
                "right_shift"
            };
            observed.push(top_text(&press(&mut mode, &env, shift)));
            release(&mut mode, &env, shift);
        }

        assert_eq!(observed.len(), 6);
        assert_ne!(observed[0], observed[5]);
        assert_eq!(observed[1], observed[5], "non-default layers must wrap");
        observed.truncate(5);
        observed.sort_unstable();
        observed.dedup();
        assert_eq!(observed.len(), 5, "every computed layer must be reachable");
    }

    #[test]
    fn two_layers_show_the_other_layer_on_every_shift_press() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let initial = deliver(
            &mut mode,
            &env,
            vec![target("One", 100.0), target("Two", 100.0)],
        );
        let top_text = |output: &[Command]| top_label(scene_of(output)).text.clone();
        let initial_top = top_text(&initial);
        let first_top = top_text(&press(&mut mode, &env, "left_shift"));
        assert_ne!(first_top, initial_top);
        assert_eq!(
            top_text(&release(&mut mode, &env, "left_shift")),
            initial_top
        );
        let second_top = top_text(&press(&mut mode, &env, "right_shift"));
        assert_eq!(second_top, first_top);
    }

    #[test]
    fn layer_switch_changes_only_draw_order_and_z_index() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
        let shifted = press(&mut mode, &env, "left_shift");
        let normalize = |output: &[Command]| {
            let mut labels = scene_of(output)
                .labels
                .iter()
                .cloned()
                .map(|mut label| {
                    label.z_index = 2;
                    label
                })
                .collect::<Vec<_>>();
            labels.sort_by(|left, right| left.text.cmp(&right.text));
            labels
        };

        assert_eq!(normalize(&initial), normalize(&shifted));
        let sorted = scene_of(&shifted).clone().sorted();
        assert_eq!(
            sorted.labels.last().unwrap().z_index,
            top_label(&sorted).z_index
        );
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
            ["b", "d"]
        );

        let mut second = labels;
        rotate_overlapping_labels(&mut second, 2);
        assert_eq!(
            second
                .iter()
                .filter(|label| label.z_index == 3)
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>(),
            ["a", "c"]
        );
    }

    #[test]
    fn tiny_edge_contacts_do_not_invent_extra_visual_layers() {
        let label = |text: &str, x: f64| {
            OverlayLabel::new(text, Rect::new(x, 0.0, 20.0, 20.0), LabelStyle::default())
                .with_z_index(2)
        };
        // Each pair is a real two-label stack. The pairs only touch across a
        // 2px strip, which remains readable and must not create four layers.
        let labels = vec![
            label("left-0", 0.0),
            label("left-1", 0.0),
            label("right-0", 18.0),
            label("right-1", 18.0),
        ];

        let mut first = labels.clone();
        rotate_overlapping_labels(&mut first, 1);
        assert_eq!(
            first
                .iter()
                .filter(|label| label.z_index == 3)
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>(),
            ["left-1", "right-1"]
        );

        let mut second = labels;
        rotate_overlapping_labels(&mut second, 2);
        assert_eq!(
            second
                .iter()
                .filter(|label| label.z_index == 3)
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>(),
            ["left-0", "right-0"]
        );
    }

    #[test]
    fn visual_stack_threshold_and_center_occlusion_are_stable() {
        let base = Rect::new(0.0, 0.0, 20.0, 20.0);
        assert!(visually_stacked(
            base,
            Rect::new(16.0, 0.0, 20.0, 20.0),
            4.0,
            2.0
        ));
        assert!(!visually_stacked(
            base,
            Rect::new(16.01, 0.0, 20.0, 20.0),
            4.0,
            2.0
        ));
        assert!(
            visually_stacked(base, Rect::new(17.0, 0.0, 20.0, 20.0), 2.0, 2.0),
            "sub-20% overlap that reaches text content must be switchable"
        );
        assert!(
            !visually_stacked(base, Rect::new(17.0, 0.0, 20.0, 20.0), 4.0, 2.0),
            "padding-only overlap must not create another layer"
        );
        assert!(visually_stacked(
            Rect::new(0.0, 0.0, 100.0, 100.0),
            Rect::new(49.5, -450.0, 1.0, 1_000.0),
            4.0,
            2.0
        ));
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
    fn visual_layers_follow_draw_order_instead_of_spatial_order() {
        let label = |text: &str, x: f64| {
            OverlayLabel::new(text, Rect::new(x, 0.0, 10.0, 10.0), LabelStyle::default())
                .with_z_index(2)
        };
        // `bottom` is emitted first and is obscured by both later labels. A
        // spatial left-to-right coloring reverses this visual relationship.
        let labels = vec![
            label("bottom", 8.0),
            label("top-left", 0.0),
            label("top-right", 16.0),
        ];

        let mut first = labels.clone();
        rotate_overlapping_labels(&mut first, 1);
        assert_eq!(
            first
                .iter()
                .filter(|label| label.z_index == 3)
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>(),
            ["top-left", "top-right"]
        );

        let mut second = labels;
        rotate_overlapping_labels(&mut second, 2);
        assert_eq!(
            second
                .iter()
                .filter(|label| label.z_index == 3)
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>(),
            ["bottom"]
        );
    }

    #[test]
    fn final_layer_plan_never_removes_or_moves_scan_labels() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let targets = (0..128)
            .map(|index| target(&format!("Target {index}"), 100.0))
            .collect();

        let completed = deliver(&mut mode, &env, targets);
        let completed_labels: Vec<_> = scene_of(&completed)
            .labels
            .iter()
            .map(|label| label.text.clone())
            .collect();
        let completed_rects: Vec<_> = scene_of(&completed)
            .labels
            .iter()
            .map(|label| label.rect)
            .collect();
        assert_eq!(mode.session.scanned.len(), 128);
        assert_eq!(mode.session.hints.len(), 128);
        assert_eq!(completed_labels.len(), 128);
        assert_eq!(mode.overlap_plan.len(), 128);
        assert_eq!(mode.overlap_plan.layer_count(), 128);

        let shifted = press(&mut mode, &env, "left_shift");
        let shifted_scene = scene_of(&shifted);
        assert_eq!(shifted_scene.labels.len(), 128);
        let mut shifted_labels = shifted_scene
            .labels
            .iter()
            .map(|label| label.text.as_str())
            .collect::<Vec<_>>();
        let mut expected_labels = completed_labels
            .iter()
            .map(crate::api::overlay::OverlayText::as_str)
            .collect::<Vec<_>>();
        shifted_labels.sort_unstable();
        expected_labels.sort_unstable();
        assert_eq!(shifted_labels, expected_labels);
        for label in &shifted_scene.labels {
            let original_index = completed_labels
                .iter()
                .position(|text| text == &label.text)
                .expect("every shifted label came from the completed scene");
            assert_eq!(label.rect, completed_rects[original_index]);
        }
        assert_eq!(top_layer_count(shifted_scene), 1);
        let sorted = shifted_scene.clone().sorted();
        assert_eq!(
            sorted.labels.last().unwrap().z_index,
            top_label(&sorted).z_index
        );
    }

    #[test]
    fn shallow_groups_wrap_their_non_default_layers_during_a_deeper_global_cycle() {
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
            ["two-0", "three-0"]
        );
    }

    #[test]
    fn overlap_key_during_scan_uses_all_merged_partial_labels() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id,
                targets: vec![target("One", 100.0), target("Two", 100.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );

        let held = press(&mut mode, &env, "left_shift");
        assert_eq!(scene_of(&held).labels.len(), 2);
        assert_eq!(mode.held_overlap_keys.len(), 1);
        assert!(mode.overlap_plan.is_ready());
        assert_eq!(mode.overlap_plan.len(), 2);
        assert_eq!(mode.overlap_plan.layer_count(), 2);

        let streamed = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id,
                targets: vec![target("Three", 100.0), target("Four", 100.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        let streamed_scene = scene_of(&streamed);
        assert_eq!(streamed_scene.labels.len(), 4);
        assert_eq!(
            mode.session.hints.len(),
            4,
            "new partial labels remain visible"
        );
        assert!(mode.overlap_plan.is_ready());
        assert_eq!(mode.overlap_plan.len(), 4);
        assert_eq!(mode.overlap_plan.layer_count(), 4);
        assert_eq!(
            top_layer_count(streamed_scene),
            1,
            "UIA and Vision partials share one visual layer plan"
        );

        let completed = deliver(&mut mode, &env, Vec::new());
        assert!(completed.is_empty(), "terminal reuses the merged plan");
        assert!(!mode.session.scanning);
        assert!(mode.overlap_plan.is_ready());
        assert_eq!(mode.overlap_plan.len(), 4);
        assert_eq!(mode.overlap_plan.layer_count(), 4);

        let released = release(&mut mode, &env, "left_shift");
        assert_eq!(top_layer_count(scene_of(&released)), 1);

        let mut raised_labels = HashSet::from([top_label(scene_of(&released)).text.clone()]);
        for key in ["left_shift", "right_shift", "left_shift", "right_shift"] {
            let cycled = press(&mut mode, &env, key);
            raised_labels.insert(top_label(scene_of(&cycled)).text.clone());
            release(&mut mode, &env, key);
        }
        assert_eq!(
            raised_labels.len(),
            4,
            "every stacked label must be reachable"
        );
    }

    #[test]
    fn partials_after_a_typed_prefix_merge_when_the_prefix_is_cleared() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        mode.handle(
            &ModeEvent::UiScanned(UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id,
                targets: (0..30)
                    .map(|index| target(&format!("Initial {index}"), index as f64 * 24.0))
                    .collect(),
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        let original_hint_count = mode.session.hints.len();
        let prefix = mode
            .alphabet
            .iter()
            .copied()
            .find(|prefix| {
                let prefix = prefix.to_string();
                mode.session
                    .hints
                    .iter()
                    .filter(|hint| hint.label.as_str().starts_with(&prefix))
                    .count()
                    > 1
                    && mode
                        .session
                        .hints
                        .iter()
                        .all(|hint| hint.label.as_str() != prefix)
            })
            .expect("test data should provide a partial label prefix");
        press(&mut mode, &env, &prefix.to_string());

        let late = mode.handle(
            &ModeEvent::UiScanned(UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id,
                targets: (0..5)
                    .map(|index| target(&format!("Late {index}"), 720.0 + index as f64 * 24.0))
                    .collect(),
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );
        assert!(late.is_empty(), "typed label codes must remain stable");
        assert_eq!(mode.session.hints.len(), original_hint_count);
        assert_eq!(mode.session.scanned.len(), original_hint_count);
        assert!(mode.session.pending_relabel);

        deliver(&mut mode, &env, Vec::new());
        assert_eq!(mode.session.hints.len(), original_hint_count);
        assert!(mode.session.pending_relabel);

        let merged = press(&mut mode, &env, "backspace");
        assert_eq!(mode.session.scanned.len(), original_hint_count + 5);
        assert_eq!(scene_of(&merged).labels.len(), mode.session.scanned.len());
        assert_eq!(mode.session.hints.len(), mode.session.scanned.len());
        assert!(!mode.session.pending_relabel);
        assert!(mode.overlap_plan.is_ready());
    }

    #[test]
    fn repeated_overlap_down_does_not_cycle_again_while_held() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
        assert_eq!(
            scene_of(&restored)
                .labels
                .iter()
                .filter(|label| label.z_index == 3)
                .count(),
            1
        );
    }

    #[test]
    fn stale_scan_results_are_ignored() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let out = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id.wrapping_add(1),
                targets: vec![target("stale", 0.0)],
                status: UiScanStatus::Success,
            }),
            &env.ctx(),
        );
        assert!(out.is_empty());
        assert!(mode.session.scanning);
        assert!(mode.session.scanned.is_empty());
    }

    #[test]
    fn repeated_display_type_and_exit_keeps_index_released() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        for count in [24, 128, 512].into_iter().cycle().take(30) {
            activate(&mut mode, &env);
            let targets = (0..count)
                .map(|i| UiTarget {
                    details: None,
                    rect: Rect::new((i % 20) as f64 * 40.0, (i / 20) as f64 * 30.0, 20.0, 20.0),
                    name: format!("Control {i}"),
                    role: SemanticRole::Button,
                })
                .collect();
            deliver(&mut mode, &env, targets);
            assert_eq!(mode.session.scanned.len(), count);
            assert_eq!(mode.session.seen_targets.capacity(), 0);
            let label = mode.session.hints[count / 2].label.clone();
            let mut out = Vec::new();
            for ch in label.as_str().chars() {
                out = press(&mut mode, &env, &ch.to_string());
                assert_eq!(mode.session.seen_targets.capacity(), 0);
            }
            assert!(out.iter().any(|command| matches!(
                command,
                Command::FinishMode {
                    cause: FinishCause::Selection
                }
            )));
            mode.handle(&ModeEvent::Deactivated, &env.ctx());
            assert_eq!(mode.session.scanned.capacity(), 0);
            assert_eq!(mode.session.hints.capacity(), 0);
            assert_eq!(mode.session.search_text.capacity(), 0);
            assert!(mode.wide_placements.is_none());
        }
    }

    #[test]
    fn typing_a_label_moves_and_requests_finish_without_clicking() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![target("Save", 0.0), target("Cancel", 200.0)],
        );

        let label = mode.session.hints[1].label.clone();
        let expected = mode.session.scanned[1].rect.center();
        let mut out = Vec::new();
        for ch in label.as_str().chars() {
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let first_id = mode.session.scan_id;

        let timed_out = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: Vec::new(),
                id: first_id,
                targets: Vec::new(),
                status: UiScanStatus::TimedOut,
            }),
            &env.ctx(),
        );
        assert!(
            mode.session.scanning,
            "retry wait still captures hint input"
        );
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
    fn context_change_retargets_immediately_without_consuming_retry_budget() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, vec![target("Old", 20.0)]);
        let retired_scan_id = mode.session.scan_id;
        mode.session.retry_attempt = 2;

        let out = mode.handle_owned(
            ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: Vec::new(),
                id: retired_scan_id,
                targets: Vec::new(),
                status: UiScanStatus::ContextChanged,
            }),
            &env.ctx(),
        );

        assert!(mode.session.scan_id > retired_scan_id);
        assert_eq!(mode.session.retry_attempt, 0);
        assert!(mode.session.scanning);
        assert!(mode.session.scanned.is_empty());
        assert!(mode.session.hints.is_empty());
        assert!(
            out.iter()
                .any(|command| matches!(command, Command::ScanUi(_)))
        );
        assert!(
            out.iter()
                .all(|command| !matches!(command, Command::SetTimer { .. }))
        );
    }

    #[test]
    fn no_window_prompt_does_not_retry_or_advertise_a_fixed_shortcut() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);

        let out = mode.handle_owned(
            ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id,
                targets: Vec::new(),
                status: UiScanStatus::Failed(NO_WINDOW_UNDER_POINTER.into()),
            }),
            &env.ctx(),
        );

        assert!(!mode.session.scanning);
        assert_eq!(
            mode.session.status.as_deref(),
            Some(NO_WINDOW_UNDER_POINTER)
        );
        assert!(
            out.iter()
                .all(|command| !matches!(command, Command::SetTimer { .. }))
        );
        let scene = scene_of(&out);
        assert_eq!(scene.labels.len(), 1);
        assert_eq!(scene.labels[0].text, NO_WINDOW_UNDER_POINTER);
        assert!(!scene.labels[0].text.contains("Primary"));
    }

    #[test]
    fn partial_timeout_keeps_labels_without_retrying() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id,
                targets: vec![target("Ready", 100.0)],
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );

        let completed = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id,
                targets: Vec::new(),
                status: UiScanStatus::TimedOut,
            }),
            &env.ctx(),
        );
        assert!(!mode.session.scanning);
        assert_eq!(scene_of(&completed).labels.len(), 1);
        assert!(
            completed
                .iter()
                .all(|command| !matches!(command, Command::SetTimer { id, .. }
                    if id == SCAN_RETRY_TIMER_ID))
        );
    }

    #[test]
    fn scan_failure_status_stays_visible_without_leaving_hint_mode() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        let out = mode.handle(
            &ModeEvent::UiScanned(crate::api::UiScanResult {
                retired: Vec::new(),
                id: mode.session.scan_id,
                targets: Vec::new(),
                status: UiScanStatus::PermissionDenied("Screen Recording required".into()),
            }),
            &env.ctx(),
        );
        assert!(!mode.session.scanning);
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        assert!(press(&mut mode, &env, "a").is_empty());
        // Escape still works, so the user is never stuck.
        assert_eq!(press(&mut mode, &env, "esc"), Command::dismiss_to_idle());
    }

    #[test]
    fn search_opens_before_first_batch_and_survives_empty_retry() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        mode.config.scan_retry_count = 1;
        activate(&mut mode, &env);
        let opened = press(&mut mode, &env, "/");
        assert!(
            opened
                .iter()
                .any(|cmd| matches!(cmd, Command::OpenTextPrompt(_)))
        );
        let has_shell = |commands: &[Command]| {
            scene_of(commands).shapes.iter().any(|shape| {
                matches!(
                    shape,
                    OverlayShape::Rect {
                        z_index: 10_000,
                        ..
                    }
                )
            })
        };
        assert!(has_shell(&opened));
        press(&mut mode, &env, "c");
        let retry = deliver(&mut mode, &env, Vec::new());
        assert!(mode.session.retry_pending);
        assert!(has_shell(&retry));
        mode.retry_scan(&env.ctx());
        let results = deliver(
            &mut mode,
            &env,
            vec![target("Save", 0.0), target("Cancel", 200.0)],
        );
        assert!(has_shell(&results));
        assert!(matches!(&mode.input, Input::Search(query) if query.as_str() == "c"));
        assert_eq!(mode.session.hints.len(), 1);
        assert_eq!(
            mode.session.scanned[mode.session.hints[0].value].name,
            "Cancel"
        );
    }

    #[test]
    fn slash_opens_search_and_filters_by_name() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![target("Save", 0.0), target("Cancel", 200.0)],
        );
        assert!(mode.session.search_names_initialized);
        let search_storage = mode.session.search_text.as_ptr();

        press(&mut mode, &env, "/");
        assert_eq!(mode.session.search_text.as_ptr(), search_storage);
        assert!(mode.session.search_names_initialized);
        assert!(mode.session.search_text[0].matches("save", ""));
        assert!(mode.session.search_text[1].matches("cancel", ""));
        let out = press(&mut mode, &env, "c");
        assert_eq!(mode.session.hints.len(), 1, "only Cancel should survive");
        assert_eq!(
            mode.session.scanned[mode.session.hints[0].value].name,
            "Cancel"
        );

        // Native editor draws the query; the overlay owns its physical shell.
        assert!(
            scene_of(&out).shapes.iter().any(|shape| matches!(
                shape,
                OverlayShape::Rect {
                    z_index: 10_000,
                    ..
                }
            )),
            "search box missing"
        );
    }

    #[test]
    fn name_search_rebuilds_overlap_layers_for_each_query() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![target("Save", 100.0), target("Cancel", 100.0)],
        );

        press(&mut mode, &env, "/");
        let filtered = press(&mut mode, &env, "a");
        assert_eq!(mode.session.hints.len(), 2);
        assert_eq!(mode.overlap_plan.len(), 2);
        let scene = scene_of(&filtered);
        assert_eq!(
            scene
                .labels
                .iter()
                .filter(|label| label.z_index < 10_000)
                .count(),
            2
        );
        assert_eq!(
            scene
                .labels
                .iter()
                .filter(|label| label.z_index == 10_002)
                .count(),
            1
        );
        assert_eq!(mode.session.search_selected.len(), 1);
        assert_eq!(mode.session.search_selected[0].label.as_str(), "a");

        let cycled = press(&mut mode, &env, "left_shift");
        assert_eq!(
            scene_of(&cycled)
                .labels
                .iter()
                .filter(|label| label.z_index == 3)
                .count(),
            1
        );
    }

    #[test]
    fn unicode_search_uses_the_normalized_key_without_changing_matches() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            vec![target("École", 0.0), target("Cancel", 200.0)],
        );

        press(&mut mode, &env, "/");
        press(&mut mode, &env, "É");

        assert_eq!(mode.input.text(), "é");
        assert_eq!(mode.session.hints.len(), 1);
        assert_eq!(
            mode.session.scanned[mode.session.hints[0].value].name,
            "École"
        );
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);

        let save = target("Save", 0.0);
        let mut different_name = save.clone();
        different_name.name = "Save as".into();
        let mut different_role = save.clone();
        different_role.role = SemanticRole::Checkbox;
        deliver(
            &mut mode,
            &env,
            vec![save.clone(), save, different_name, different_role],
        );

        assert_eq!(mode.session.scanned.len(), 3);
        assert_eq!(mode.session.seen_targets.capacity(), 0);
        assert_eq!(mode.session.search_text.len(), 3);
    }

    #[test]
    fn owned_first_partial_adopts_target_and_string_storage() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);

        let targets = vec![target("Save 设置", 20.0), target("Cancel", 120.0)];
        let targets_ptr = targets.as_ptr();
        let name_ptr = targets[0].name.as_ptr();
        let scan_id = mode.session.scan_id;
        let _ = mode.handle_owned(
            ModeEvent::UiScanned(UiScanResult {
                retired: Vec::new(),
                id: scan_id,
                targets,
                status: UiScanStatus::Partial,
            }),
            &env.ctx(),
        );

        assert_eq!(mode.session.scanned.as_ptr(), targets_ptr);
        assert_eq!(mode.session.scanned[0].name.as_ptr(), name_ptr);
        assert_eq!(
            mode.session
                .scanned
                .iter()
                .map(|target| target.name.as_str())
                .collect::<Vec<_>>(),
            ["Save 设置", "Cancel"]
        );
    }

    #[test]
    fn escape_leaves_search_before_leaving_the_mode() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        // Enough targets that labels are two characters long.
        let targets: Vec<UiTarget> = (0..30)
            .map(|i| target(&format!("b{i}"), i as f64 * 10.0))
            .collect();
        deliver(&mut mode, &env, targets);

        let first = mode.session.hints[0].label.as_str().chars().next().unwrap();
        press(&mut mode, &env, &first.to_string());
        let before = mode.input.text().to_string();

        // A character in the alphabet that cannot follow the current prefix.
        let dead_end = mode
            .alphabet
            .iter()
            .find(|c| {
                !mode
                    .session
                    .hints
                    .iter()
                    .any(|h| h.label.as_str().starts_with(&format!("{before}{c}")))
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, vec![target("Save", 0.0)]);

        let out = mode.handle(&ModeEvent::FocusChanged(None), &env.ctx());
        assert!(out.iter().any(|c| matches!(c, Command::ScanUi { .. })));
        assert!(mode.session.hints.is_empty(), "stale hints must be dropped");
    }

    #[test]
    fn boundary_highlight_adds_outlines_when_enabled() {
        let mut config = Config::default();
        config.ui_hint.boundary_highlight.enabled = true;
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(
            &mut mode,
            &env,
            (0..5)
                .map(|i| target(&format!("b{i}"), i as f64 * 10.0))
                .collect(),
        );
        let labels: Vec<&str> = mode
            .session
            .hints
            .iter()
            .map(|h| h.label.as_str())
            .collect();
        assert_eq!(labels, ["aa", "sa", "da", "fa", "as"]);
    }

    #[test]
    fn kept_finish_uses_cached_hints_and_backspace_reopens_them() {
        let mut config = Config::default();
        config.ui_hint.lifecycle.after_finish = crate::config::LifecycleAction::Keep;
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::hint(&env.config);
        activate(&mut mode, &env);
        deliver(&mut mode, &env, vec![target("Save", 100.0)]);
        let label = mode.session.hints[0].label.clone();
        let selected = press(&mut mode, &env, label.as_str());
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
        assert!(mode.session.finished);
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
        assert!(!mode.session.finished);
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
        let mut mode = crate::app::mode_catalog::hint(&env.config);
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
