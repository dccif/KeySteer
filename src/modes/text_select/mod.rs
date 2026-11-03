//! OCR text selection on the same scan, search, multi-selection and input model as UIHint.
mod document;
mod selection_input;
#[cfg(test)]
mod tests;
use super::hint::{Settings as HintSettings, search, session::ScanSession};
use crate::api::hint as labeling;
use crate::api::text_select_preview as preview;
use crate::api::{
    Binding, Color, Command, CommandBatch, HostContext, Key, KeyChord, KeyState, Mode, ModeEvent,
    ModeId, Palette, Rect, UiScanRequest, UiScanStrategy,
    command::{UiScanResult, UiScanStatus},
    presentation::{
        HintContent, HintSceneCache, HintStyle, HintView, TextSelectView, View, VisualLayerPlan,
    },
    text_edit::{EditAction, Selection},
    text_select::{Action, Lifecycle, Request, Segmented},
    window_presets::TextPrompt,
};
use document::{Frame, Level};
use std::{fmt::Write, sync::Arc, time::Duration};
use unicode_segmentation::UnicodeSegmentation;

pub struct Settings {
    pub hint: HintSettings,
    pub alphabet: Vec<char>,
    pub ui: crate::api::text_select::Ui<crate::api::theme::CompiledColor>,
    pub lifecycle: Lifecycle,
    pub bindings: Vec<(KeyChord, Arc<Binding>)>,
}

pub struct TextSelectMode {
    config: Settings,
    active: bool,
    scan_id: u64,
    segment_id: u64,
    pending_segment: bool,
    preview_actions: preview::Actions,
    frozen: bool,
    source: ScanSession,
    search: ScanSession,
    frame: Frame,
    automatic_selection: Option<document::ToggleUndo>,
    history: Vec<Frame>,
    base_selected: Vec<bool>,
    page: usize,
    page_keys: String,
    output: String,
    summary: String,
    status: String,
    window: Option<Rect>,
    bounds: Option<Rect>,
    return_mode: ModeId,
    region: super::scan_region::ScanRegionSession,
    copying: bool,
    layers: VisualLayerPlan,
    wide_placements: Option<Vec<(usize, Rect)>>,
    held_overlap_keys: Vec<Key>,
    overlap_cycle: usize,
    scene_cache: HintSceneCache,
}

const PAGE_SIZE: usize = 24;
const MAINTENANCE: &str = "text-select-maintenance";

fn panel_hints(
    hints: &[labeling::CompactHint<usize>],
    page: usize,
) -> &[labeling::CompactHint<usize>] {
    let start = (page * PAGE_SIZE).min(hints.len());
    &hints[start..(start + PAGE_SIZE).min(hints.len())]
}

impl TextSelectMode {
    pub fn new(mut config: Settings) -> Self {
        // Compile both meanings of selection keys once, independent of Primary.
        let mut edit_keys: Vec<_> = config
            .bindings
            .iter()
            .filter_map(|(key, binding)| {
                let action = match binding.as_ref() {
                    Binding::TextSelect(Action::SelectAll) => EditAction::SelectAll,
                    Binding::TextSelect(Action::ClearSelection) => EditAction::Backspace,
                    _ => return None,
                };
                Some((key.clone(), action))
            })
            .collect();
        for (key, action) in config.hint.search_edit_keys.iter() {
            // This prompt is selection syntax; copying always targets the panel.
            if !matches!(action, EditAction::Copy | EditAction::Cut)
                && !edit_keys.iter().any(|(existing, _)| existing == key)
            {
                edit_keys.push((key.clone(), *action));
            }
        }
        config.hint.search_edit_keys = edit_keys.into();
        Self {
            page_keys: config
                .bindings
                .iter()
                .filter(|(_, binding)| {
                    matches!(binding.as_ref(), Binding::TextSelect(Action::CyclePage))
                })
                .map(|(chord, _)| crate::api::input::display_key_chord(&chord.canonical()))
                .collect::<Vec<_>>()
                .join(" / "),
            config,
            active: false,
            scan_id: 0,
            segment_id: 0,
            pending_segment: false,
            preview_actions: preview::Actions::default(),
            frozen: false,
            source: ScanSession::default(),
            search: ScanSession::default(),
            frame: Frame::default(),
            automatic_selection: None,
            history: Vec::new(),
            base_selected: Vec::new(),
            page: 0,
            output: String::new(),
            summary: String::new(),
            status: String::new(),
            window: None,
            bounds: None,
            return_mode: ModeId::normal(),
            region: super::scan_region::ScanRegionSession::default(),
            copying: false,
            layers: VisualLayerPlan::default(),
            wide_placements: None,
            held_overlap_keys: Vec::new(),
            overlap_cycle: 0,
            scene_cache: HintSceneCache::default(),
        }
    }

    fn reindex(&mut self) {
        self.automatic_selection = None;
        self.scene_cache.invalidate();
        self.layers.clear();
        if self.frame.level != Level::Regions {
            self.held_overlap_keys.clear();
            self.overlap_cycle = 0;
        }
        self.search.clear_results();
        self.search.search_selection = Selection::default();
        if self.frame.level == Level::Regions {
            self.base_selected.clone_from(&self.frame.selected);
            let _ = labeling::assign_compact_into(
                &mut self.search.search_hints,
                self.frame
                    .units
                    .iter()
                    .enumerate()
                    .map(|(i, _)| (self.frame.region_bounds[i], i)),
                &self.config.alphabet,
                self.config.hint.label_direction,
            );
        } else {
            // Only region queries restore a base selection. Refined selection
            // belongs to Frame and text filtering never changes it.
            self.base_selected = Vec::new();
            // Reuse formatting storage instead of allocating once per label.
            let mut number = String::new();
            self.search
                .search_hints
                .extend((0..self.frame.units.len()).map(|index| {
                    number.clear();
                    let _ = write!(number, "{}", index + 1);
                    labeling::CompactHint {
                        label: labeling::HintCode(smallvec::SmallVec::from_slice(
                            number.as_bytes(),
                        )),
                        bounds: Rect::default(),
                        value: index,
                    }
                }));
            // No text index is needed until a refined search is actually entered.
            self.search.search_names_initialized = true;
        }
        self.page = 0;
    }

    fn filter(&mut self, budget: usize) {
        if self.frame.level != Level::Regions {
            self.filter_refined(budget.min(256));
            self.update_summary();
            return;
        }
        // Transfer the existing query buffer while borrowing the shared session.
        let query = std::mem::take(&mut self.search.search_query);
        let names_ready = !search::needs_names(&query) || self.prepare_search_text(budget);
        let previous_count = self.search.hints.len();
        if search::filter_prepared(
            &mut self.search,
            &query,
            &self.config.hint.search_match_priority,
            names_ready,
        ) || self.search.hints.len() != previous_count
        {
            self.scene_cache.invalidate();
            self.layers.clear();
        }
        if query.trim().is_empty() {
            self.frame.selected.clone_from(&self.base_selected);
        } else {
            self.frame.selected.fill(false);
            for hint in &self.search.search_selected {
                self.frame.selected[hint.value] = true;
            }
            // Unresolved text/initials preview the same highest-ranked candidate
            // as UIHint; resolved space-separated terms form the copy selection.
            if self.search.search_selected.is_empty()
                && let Some(position) = self
                    .search
                    .search_focus
                    .as_ref()
                    .map(|f| f.index)
                    .or_else(|| self.search.search_cycle.first())
                && let Some(&index) = self.search.search_matches.get(position)
            {
                self.frame.selected[self.search.search_hints[index].value] = true;
            }
        }
        // An early unresolved query must still see later OCR batches. Freeze
        // the source only once an item actually resolves, or on refinement.
        self.frozen |= !self.search.search_selected.is_empty();
        self.search.search_query = query;
        self.update_summary();
    }

    fn update_summary(&mut self) {
        let selected = if self.pending_segment {
            // Preserve the waiting preview/count without disposable character
            // ranges, selection flags, order, or numeric hint lists.
            self.output.clear();
            self.output.push_str(&self.frame.text);
            self.output
                .graphemes(true)
                .filter(|g| !g.trim().is_empty())
                .count()
        } else {
            self.frame.write_selected_text(&mut self.output);
            self.frame.selected.iter().filter(|&&s| s).count()
        };
        if self.frame.selection_waiting_for_preview() || self.frame.selection_incomplete() {
            self.output.clear();
        }
        self.summary.clear();
        if selected != 0 {
            let unit = match self.frame.level {
                Level::Regions => "regions",
                Level::Words => "words",
                Level::Characters => "characters",
            };
            let characters = if self.output.is_ascii() {
                // CRLF is the only multi-byte grapheme in ASCII text.
                self.output.len()
                    - self
                        .output
                        .as_bytes()
                        .windows(2)
                        .filter(|pair| *pair == b"\r\n")
                        .count()
            } else {
                self.output.graphemes(true).count()
            };
            let _ = write!(self.summary, "{selected} {unit} · {characters} chars");
        }
        self.page = self
            .page
            .min(self.search.hints.len().saturating_sub(1) / PAGE_SIZE);
    }

    fn timer() -> Command {
        Command::SetTimer {
            id: MAINTENANCE.into(),
            delay: Duration::ZERO,
            repeating: false,
        }
    }

    #[inline(always)]
    fn holds_preview_labels(&self) -> bool {
        preview::holds_labels(
            self.frozen,
            self.frame.level == Level::Regions,
            &self.frame.previews,
            &self.search.search_query,
            &self.search.search_hints,
        )
    }

    fn finish(&mut self, ctx: &HostContext<'_>, mut commands: CommandBatch) -> CommandBatch {
        if self.frame.level == Level::Regions
            && !self.held_overlap_keys.is_empty()
            && !self.layers.is_ready()
        {
            self.prepare_overlap(ctx);
        }
        commands.push(self.render(ctx));
        // Submit the input/first hints before building pinyin or later batches.
        let needs_maintenance = !self.pending_segment
            && ((self.frame.level != Level::Regions && self.search.search_pending)
                || (!self.source.scanned.is_empty() && !self.search.search_names_initialized)
                || (!self.frozen && self.source.pending_relabel && !self.holds_preview_labels()));
        if needs_maintenance {
            commands.push(Self::timer());
        }
        commands
    }

    fn rebuild_source(&mut self) {
        let query = std::mem::take(&mut self.search.search_query);
        let edit = self.search.search_selection;
        self.frame = Frame::from_targets(&self.source.scanned);
        self.reindex();
        self.search.search_query = query;
        self.search.search_selection = edit;
        self.filter(0);
    }

    fn maintain(&mut self, ctx: &HostContext<'_>) -> CommandBatch {
        if self.frame.level != Level::Regions {
            return if self.search.search_pending {
                self.filter_refined(1024);
                if self.consume_unambiguous_selection() {
                    self.update_summary();
                }
                self.finish(ctx, CommandBatch::new())
            } else {
                CommandBatch::new()
            };
        }
        let changed = !self.frozen && self.source.pending_relabel && !self.holds_preview_labels();
        if changed {
            self.source.apply_deferred();
            self.source.pending_relabel = false;
            self.rebuild_source();
        }
        let pending = self.search.search_pending;
        let ready = self.prepare_search_text(if pending { 1024 } else { 256 });
        if ready && pending {
            self.filter(0);
        }
        if changed || pending && ready {
            return self.finish(ctx, self.selection_pointer());
        }
        if !ready {
            CommandBatch::one(Self::timer())
        } else {
            CommandBatch::new()
        }
    }

    fn start(&mut self, ctx: &HostContext<'_>) -> CommandBatch {
        self.held_overlap_keys.clear();
        self.overlap_cycle = 0;
        self.active = true;
        self.scan_id = super::scan_region::next_scan_id();
        self.segment_id = self.segment_id.wrapping_add(1);
        self.pending_segment = false;
        self.preview_actions.clear();
        self.copying = false;
        self.frozen = false;
        self.source.clear_results();
        self.scene_cache.clear();
        self.frame = Frame::default();
        self.history.clear();
        self.window = None;
        self.status = "Recognizing text…".into();
        self.reindex();
        self.filter(0);
        let area = self
            .region
            .area(ctx, self.config.hint.scan_scope, self.bounds);
        let bounds = area.bounds;
        self.bounds = Some(bounds);
        self.source.scan_bounds = self.bounds;
        let mut vision = self.config.hint.vision.clone();
        vision.detect_text = true;
        vision.detect_rectangles = false;
        let mut commands = CommandBatch::two(
            Command::CancelTimer {
                id: MAINTENANCE.into(),
            },
            Command::CancelTextSegmentation,
        );
        if self.region.is_bounded() {
            commands.push(Command::HideOverlay);
        }
        commands.push(Command::scan_ui(UiScanRequest {
            id: self.scan_id,
            scope: area.scope,
            screen_bounds: area.screen_bounds,
            timeout_ms: self.config.hint.scan_timeout_ms,
            bounds: Some(bounds),
            roles: Vec::new(),
            max_depth: self.config.hint.max_depth,
            visible_only: true,
            clickable_only: false,
            strategy: UiScanStrategy::Vision,
            vision,
            app: ctx.focused_app.cloned(),
        }));
        let (bounds, style) =
            ctx.presenter
                .input_panel(&self.config.hint.search_input_ui, self.window, ctx);
        commands.push(Command::OpenTextPrompt(Box::new(TextPrompt {
            point_keys: None,
            edit_keys: self.config.hint.search_edit_keys.clone(),
            live_style: Some(style),
            copy_keys: Arc::from([]),
            bounds,
            id: self.scan_id,
            title: String::new(),
            message: String::new(),
            placeholder: String::new(),
            max_chars: 4096,
        })));
        commands.push(Command::RequestPanelWindowBounds(self.scan_id));
        self.finish(ctx, commands)
    }

    #[inline(always)]
    fn scan_event(&mut self, mut result: UiScanResult, ctx: &HostContext<'_>) -> CommandBatch {
        if self.region.is_bounded()
            && self.active
            && result.id == self.scan_id
            && let Some(bounds) = self.bounds
        {
            super::scan_region::retain_region_results(&mut result, bounds);
        }
        self.scan(result, ctx)
    }

    fn scan(&mut self, mut result: UiScanResult, ctx: &HostContext<'_>) -> CommandBatch {
        if !self.active || result.id != self.scan_id {
            return CommandBatch::new();
        }
        result.targets.retain(|t| {
            preview::is_fast_preview(t)
                || !t.name.trim().is_empty()
                || !t.ocr_text().trim().is_empty()
        });
        self.status = match result.status {
            UiScanStatus::Partial => "Recognizing text…",
            UiScanStatus::PermissionDenied(_) => "Screen recording permission required",
            UiScanStatus::Unsupported(_) => "System OCR unavailable",
            UiScanStatus::ContextChanged => "Screen changed — reopen Text Select",
            UiScanStatus::Failed(_) => "Text recognition failed",
            UiScanStatus::TimedOut => "Text recognition timed out",
            _ => "No text found",
        }
        .into();
        // Reconcile fast geometry before a deferred update can be selected.
        let changed = preview::reconcile(&mut self.frame, &mut self.history, &result);
        if changed {
            self.search.search_text.clear();
            self.search.search_names_initialized = false;
            self.update_summary();
        }
        if self.frozen || !self.source.scanned.is_empty() {
            if !result.targets.is_empty() || !result.retired.is_empty() {
                self.source.defer_update(result.targets, result.retired);
            }
            return if self.frozen {
                if let Some(action) = self.preview_actions.resolve(
                    &result.status,
                    self.frame.selection_waiting_for_preview(),
                    self.frame.selection_incomplete(),
                ) {
                    return match action {
                        Action::Copy => self.copy_selection(ctx, true),
                        Action::Refine => self.refine_selection(ctx),
                        _ => CommandBatch::new(),
                    };
                }
                if changed {
                    self.finish(ctx, CommandBatch::new())
                } else {
                    CommandBatch::new()
                }
            } else if self.holds_preview_labels() {
                if changed {
                    self.finish(ctx, CommandBatch::new())
                } else {
                    CommandBatch::new()
                }
            } else {
                CommandBatch::one(Self::timer())
            };
        }
        self.source.apply_update(result.targets, &result.retired);
        self.rebuild_source();
        self.finish(ctx, CommandBatch::new())
    }

    fn refine(&mut self, ctx: &HostContext<'_>) -> CommandBatch {
        if self.pending_segment || self.frame.level == Level::Characters {
            return CommandBatch::new();
        }
        self.consume_selection_numbers(true);
        // Explicit acceptance may finish the remaining index, as in UIHint.
        self.filter(usize::MAX);
        if self.wait_for_preview(Action::Refine) {
            return self.finish(ctx, CommandBatch::new());
        }
        self.refine_selection(ctx)
    }

    #[inline(always)]
    fn wait_for_preview(&mut self, action: Action) -> bool {
        self.preview_actions.wait(
            action,
            self.frame.selection_waiting_for_preview(),
            self.frame.selection_incomplete(),
            &mut self.frozen,
        )
    }

    #[inline(always)]
    fn refine_selection(&mut self, ctx: &HostContext<'_>) -> CommandBatch {
        if self.output.is_empty() {
            return self.finish(ctx, CommandBatch::new());
        }
        let level = if self.frame.level == Level::Regions {
            Level::Words
        } else {
            Level::Characters
        };
        // Filtering and preview reconciliation already wrote the exact copy
        // selection to output. Snapshot it without joining the selection again.
        let next = Frame::refined(&self.output, level);
        self.history.push(std::mem::replace(&mut self.frame, next));
        self.frozen = true;
        self.pending_segment = level == Level::Words;
        self.reindex();
        self.status.clear();
        if self.pending_segment {
            self.segment_id = self.segment_id.wrapping_add(1);
            self.update_summary();
        } else {
            self.filter(0);
        }
        // Paint pending state first: a backend may complete with its fallback
        // synchronously, and that result must remain the newest scene.
        let mut commands = self.finish(ctx, CommandBatch::new());
        if level == Level::Words {
            commands.push(Command::SegmentText(Request {
                id: self.segment_id,
                text: self.frame.text.clone(),
            }));
        }
        commands
    }

    fn segmented(&mut self, result: Segmented, ctx: &HostContext<'_>) -> CommandBatch {
        if !self.active || !self.pending_segment || result.id != self.segment_id {
            return CommandBatch::new();
        }
        self.pending_segment = false;
        self.frame.apply_words(&result.words);
        let query = std::mem::take(&mut self.search.search_query);
        let edit = self.search.search_selection;
        self.reindex();
        // Committed numbers locate their page in the complete new choices
        // before the saved text query filters that list.
        self.search.hints.clone_from(&self.search.search_hints);
        self.search.search_query = query;
        self.search.search_selection = edit;
        self.consume_selection_numbers(false);
        self.filter_refined(256);
        self.consume_unambiguous_selection();
        self.copying = false;
        self.update_summary();
        self.status.clear();
        self.finish(ctx, CommandBatch::new())
    }

    fn action(&mut self, action: Action, ctx: &HostContext<'_>) -> CommandBatch {
        self.automatic_selection = None;
        self.preview_actions.clear();
        match action {
            Action::ClearInput => {
                if self.search.search_query.is_empty() {
                    return CommandBatch::new();
                }
                self.search.search_query.clear();
                self.search.search_selection = Selection::default();
                return self.query_changed(false, ctx);
            }
            Action::SelectAll | Action::ClearSelection => {
                if !self.search.search_query.is_empty() || self.pending_segment {
                    return CommandBatch::new();
                }
                let selected = action == Action::SelectAll;
                if selected {
                    self.frame.select_all();
                } else {
                    self.frame.clear_selection();
                }
                self.base_selected.fill(selected);
                // Preserve the explicit selection while streaming late OCR results.
                self.frozen |= !self.frame.units.is_empty();
                self.copying = false;
                self.update_summary();
            }
            Action::CyclePage => {
                if self.frame.level == Level::Regions || self.pending_segment {
                    return CommandBatch::new();
                }
                self.page = (self.page + 1) % self.search.hints.len().div_ceil(PAGE_SIZE).max(1);
                if self.consume_unambiguous_selection() {
                    self.update_summary();
                }
            }
            Action::Refine => return self.refine(ctx),
            Action::Copy => {
                if self.pending_segment {
                    return CommandBatch::new();
                }
                let committed = self.consume_selection_numbers(true);
                self.filter(usize::MAX);
                if self.wait_for_preview(Action::Copy) {
                    return self.finish(ctx, CommandBatch::new());
                }
                return self.copy_selection(ctx, committed);
            }
            Action::Search => {
                // Search is always open. The configurable action selects its
                // query for replacement instead of introducing another mode.
                self.search
                    .search_selection
                    .edit(&mut self.search.search_query, EditAction::SelectAll);
            }
        }
        self.finish(ctx, CommandBatch::new())
    }

    #[inline(always)]
    fn copy_selection(&mut self, ctx: &HostContext<'_>, render: bool) -> CommandBatch {
        if self.output.is_empty() {
            return self.finish(ctx, CommandBatch::new());
        }
        self.copying = true;
        let mut commands = CommandBatch::new();
        if render {
            commands.push(self.render(ctx));
        }
        commands.push(Command::CopyText(self.output.clone()));
        commands
    }

    fn navigate(&mut self, reset: bool, ctx: &HostContext<'_>) -> CommandBatch {
        self.segment_id = self.segment_id.wrapping_add(1);
        self.pending_segment = false;
        self.preview_actions.clear();
        self.copying = false;
        if reset || self.history.is_empty() {
            self.history.clear();
            self.frozen = false;
            self.source.apply_deferred();
            self.source.pending_relabel = false;
            self.frame = Frame::from_targets(&self.source.scanned);
        } else if let Some(frame) = self.history.pop() {
            self.frame = frame;
        }
        self.reindex();
        self.filter(0);
        self.status.clear();
        self.finish(ctx, CommandBatch::one(Command::CancelTextSegmentation))
    }

    fn query_changed(&mut self, inserted_separator: bool, ctx: &HostContext<'_>) -> CommandBatch {
        self.preview_actions.clear();
        self.copying = false;
        self.search.search_focus = None;
        if !self.pending_segment {
            if self.frame.level == Level::Regions {
                self.page = 0;
                self.filter(0);
            } else {
                self.filter_refined(256);
                let committed = inserted_separator && self.consume_selection_numbers(false);
                if self.consume_unambiguous_selection() || committed {
                    self.update_summary();
                }
            }
        }
        self.finish(ctx, self.selection_pointer())
    }

    fn consume_selection_numbers(&mut self, finish: bool) -> bool {
        if self.frame.level == Level::Regions || self.pending_segment {
            return false;
        }
        let numbers = selection_input::consume(
            &mut self.search.search_query,
            &mut self.search.search_selection,
            self.frame.units.len(),
            finish,
        );
        let committed = !numbers.is_empty();
        for number in numbers {
            self.apply_selection_number(number);
        }
        committed
    }

    fn apply_selection_number(&mut self, number: selection_input::NumberSelection) {
        if number.range {
            if number.first <= number.last {
                for unit in number.first..=number.last {
                    self.frame.select(unit);
                }
            } else {
                for unit in (number.last..=number.first).rev() {
                    self.frame.select(unit);
                }
            }
        } else {
            self.frame.toggle(number.first);
        }
        self.show_selected_page(number.last);
    }

    fn show_selected_page(&mut self, unit: usize) {
        if let Some(position) = self.search.hints.iter().position(|hint| hint.value == unit) {
            self.page = position / PAGE_SIZE;
        }
    }

    fn consume_unambiguous_selection(&mut self) -> bool {
        if self.frame.level == Level::Regions || self.pending_segment {
            return false;
        }
        if let Some(number) = selection_input::consume_unambiguous(
            &mut self.search.search_query,
            &mut self.search.search_selection,
            self.frame.units.len(),
            panel_hints(&self.search.hints, self.page)
                .iter()
                .map(|hint| (hint.label.as_str(), hint.value)),
        ) {
            if number.range {
                self.apply_selection_number(number);
            } else {
                self.automatic_selection = Some(self.frame.toggle_with_undo(number.first));
                self.show_selected_page(number.first);
            }
            return true;
        }
        false
    }

    fn prepare_search_text(&mut self, budget: usize) -> bool {
        let prepared = self.search.search_text.len();
        let end = prepared.saturating_add(budget).min(self.frame.units.len());
        let ready = end == self.frame.units.len();
        if self.frame.level == Level::Regions {
            self.search
                .search_text
                .reserve(self.frame.units.len() - prepared);
            self.search.search_names_initialized = ready;
        }
        self.search.search_text.extend(
            self.frame.units[prepared..end]
                .iter()
                .map(|range| search::SearchText::content(&self.frame.text[range.clone()])),
        );
        ready
    }

    fn filter_refined(&mut self, budget: usize) {
        let query = self.search.search_query.to_lowercase();
        let terms: smallvec::SmallVec<[&str; 8]> = query
            .split_whitespace()
            .filter(|term| !selection_input::numeric_term(term))
            .collect();
        if terms.is_empty() {
            // Refined hints are an ordered subset of fixed labels until reindex.
            // Equal lengths therefore already mean the complete, unchanged list.
            if self.search.hints.len() != self.search.search_hints.len() {
                self.search.hints.clone_from(&self.search.search_hints);
            }
            self.search.search_pending = false;
        } else {
            self.search.search_pending = !self.prepare_search_text(budget);
            self.search.hints.clear();
            self.search.hints.extend(
                self.search
                    .search_hints
                    .iter()
                    .take(self.search.search_text.len())
                    .filter(|hint| {
                        terms
                            .iter()
                            .all(|term| self.search.search_text[hint.value].matches_text(term))
                    })
                    .cloned(),
            );
        }
        self.page = self
            .page
            .min(self.search.hints.len().saturating_sub(1) / PAGE_SIZE);
    }

    fn selection_pointer(&self) -> CommandBatch {
        let mut commands = CommandBatch::new();
        if self.frame.level == Level::Regions
            && let Some((rect, _)) = self
                .frame
                .region_bounds
                .iter()
                .zip(&self.frame.selected)
                .rfind(|(_, selected)| **selected)
        {
            commands.push(Command::WarpPointer {
                x: rect.right() + f64::from(self.config.ui.pointer_offset[0]),
                y: rect.bottom() + f64::from(self.config.ui.pointer_offset[1]),
            });
        }
        commands
    }

    fn insert(&mut self, value: &str, ctx: &HostContext<'_>) -> CommandBatch {
        if let Some(undo) = self.automatic_selection.take()
            && value.starts_with('-')
            && self.search.search_selection.cursor == self.search.search_query.len()
            && self.search.search_selection.range().is_empty()
        {
            let number = (undo.index + 1).to_string();
            self.frame.undo_toggle(undo);
            self.update_summary();
            self.search
                .search_selection
                .insert(&mut self.search.search_query, &number);
        }
        let start = self.search.search_selection.range().start;
        self.search
            .search_selection
            .insert(&mut self.search.search_query, value);
        let inserted_separator = self.search.search_query
            [start..self.search.search_selection.cursor]
            .chars()
            .any(char::is_whitespace);
        self.query_changed(inserted_separator, ctx)
    }

    fn prepare_overlap(&mut self, ctx: &HostContext<'_>) {
        self.scene_cache.invalidate();
        let config = &self.config.hint;
        ctx.presenter.prepare_hints(
            HintContent {
                hints: &self.search.hints,
                uniform_label_chars: None,
                prefix: "",
                search: Some(&self.search.search_query),
                search_selection: self.search.search_selection,
                scan_bounds: self.bounds,
                label_bounds: self.region.label_bounds(ctx, self.bounds),
                style: HintStyle {
                    ui: &config.ui,
                    placement: config.placement,
                    label_x_offset: config.label_x_offset,
                    label_y_offset: config.label_y_offset,
                    boundary_highlight: &config.boundary_highlight,
                    search_input_ui: &config.search_input_ui,
                    search_highlight: &config.search_highlight,
                },
            },
            &mut self.layers,
            &mut self.wide_placements,
            ctx,
        );
        if self.layers.layer_count() > 1 && !self.held_overlap_keys.is_empty() {
            self.overlap_cycle = self.overlap_cycle.clamp(1, self.layers.layer_count() - 1);
        }
    }

    fn active_overlap_layer(&self) -> Option<usize> {
        let count = self.layers.layer_count();
        (self.layers.is_ready() && count > 0).then(|| {
            if self.held_overlap_keys.is_empty() || count == 1 {
                0
            } else {
                self.overlap_cycle.clamp(1, count - 1)
            }
        })
    }

    fn overlap_key(&mut self, key: &Key, state: KeyState, ctx: &HostContext<'_>) -> CommandBatch {
        if state == KeyState::Down && !self.layers.is_ready() {
            self.prepare_overlap(ctx);
        }
        let before = self.active_overlap_layer();
        match state {
            KeyState::Down if !self.held_overlap_keys.contains(key) => {
                if self.held_overlap_keys.is_empty() && self.layers.layer_count() > 1 {
                    self.overlap_cycle = self.overlap_cycle % (self.layers.layer_count() - 1) + 1;
                }
                self.held_overlap_keys.push(key.clone());
            }
            KeyState::Up => self.held_overlap_keys.retain(|held| held != key),
            _ => {}
        }
        if before != self.active_overlap_layer() {
            CommandBatch::one(self.render(ctx))
        } else {
            CommandBatch::new()
        }
    }

    fn render(&self, ctx: &HostContext<'_>) -> Command {
        let region_hints = self.frame.level == Level::Regions && !self.pending_segment;
        let style = HintStyle {
            ui: &self.config.hint.ui,
            placement: self.config.hint.placement,
            label_x_offset: self.config.hint.label_x_offset,
            label_y_offset: self.config.hint.label_y_offset,
            boundary_highlight: &self.config.hint.boundary_highlight,
            search_input_ui: &self.config.hint.search_input_ui,
            search_highlight: &self.config.hint.search_highlight,
        };
        let hints = HintView {
            cache: Some(&self.scene_cache),
            highlighted_hint: if region_hints {
                let mut selected = self
                    .search
                    .hints
                    .iter()
                    .filter(|hint| self.frame.selected[hint.value]);
                let first = selected.next();
                first.filter(|_| selected.next().is_none())
            } else {
                None
            },
            point: None,
            info: None,
            window_bounds: &self.window,
            content: HintContent {
                hints: if region_hints {
                    &self.search.hints
                } else {
                    &[]
                },
                uniform_label_chars: None,
                prefix: "",
                search: Some(&self.search.search_query),
                search_selection: self.search.search_selection,
                scan_bounds: self.bounds,
                label_bounds: self.region.label_bounds(ctx, self.bounds),
                style,
            },
            layers: &self.layers,
            active_layer: self.active_overlap_layer(),
        };
        // Refinement clears the query but retains the inherited selection.
        // Keep that preview visible; an untouched or reset region scan needs
        // only its hints and search input.
        let preview_status = if self.frame.selection_incomplete() {
            Some("Text recognition incomplete — rescan")
        } else if self.frame.selection_waiting_for_preview() {
            Some("Recognizing text…")
        } else {
            None
        };
        if self.search.search_query.is_empty()
            && self.output.is_empty()
            && self.frame.level == Level::Regions
            && preview_status.is_none()
        {
            return Command::ShowOverlay(Arc::new(ctx.presenter.compose(View::Hints(hints), ctx)));
        }
        let chips = if self.frame.level != Level::Regions && !self.pending_segment {
            panel_hints(&self.search.hints, self.page)
        } else {
            &[]
        };
        let status = if self.pending_segment {
            "Segmenting words…"
        } else if let Some(status) = preview_status {
            status
        } else if self.frame.level != Level::Regions && self.output.is_empty() {
            "No text selected"
        } else if self.search.search_pending {
            "Searching…"
        } else if self.frame.units.is_empty() {
            &self.status
        } else if !self.search.search_query.trim().is_empty() && self.output.is_empty() {
            "No matching text"
        } else {
            "Select text to preview"
        };
        Command::ShowOverlay(Arc::new(ctx.presenter.compose(
            View::TextSelect(TextSelectView {
                hints,
                ui: &self.config.hint.search_info_ui,
                chips,
                unit_text: &self.frame.text,
                units: &self.frame.units,
                grid_min_width: f64::from(if self.frame.level == Level::Characters {
                    self.config.ui.char_min_width
                } else {
                    self.config.ui.word_min_width
                }),
                text: &self.output,
                status,
                summary: &self.summary,
                page: (
                    self.page + 1,
                    self.search.hints.len().div_ceil(PAGE_SIZE).max(1),
                ),
                page_keys: &self.page_keys,
                region_bounds: if region_hints && self.config.ui.border_width > 0.0 {
                    &self.frame.region_bounds
                } else {
                    &[]
                },
                selected: &self.frame.selected,
                selection_ui: &self.config.ui,
            }),
            ctx,
        )))
    }
}

impl Mode for TextSelectMode {
    fn id(&self) -> ModeId {
        ModeId::text_select()
    }
    fn display_name(&self) -> String {
        "Text Select".into()
    }
    fn indicator_color(&self, palette: &Palette) -> Option<Color> {
        Some(palette.accent)
    }
    fn indicator_detail(&self) -> Option<String> {
        Some(self.frame.level.title().into())
    }
    fn keyboard_prompt_supported(&self) -> bool {
        true
    }
    fn keyboard_prompt_active(&self) -> bool {
        self.active
    }
    fn keyboard_prompt_binding(&self, key: &Key, pressed: &[Key]) -> Option<Arc<Binding>> {
        self.config
            .bindings
            .iter()
            .find(|(chord, binding)| {
                if matches!(binding.as_ref(), Binding::Disabled)
                    && chord.keys().len() == 1
                    && chord.keys()[0].as_char().is_some()
                {
                    return false;
                }
                if key.as_str() == "backspace"
                    && chord.keys().len() == 1
                    && !self.search.search_query.is_empty()
                    && matches!(binding.as_ref(), Binding::SelectionParent)
                {
                    return false;
                }
                // Contextual controls only claim an empty prompt. With text,
                // the same keys go through the shared input editor bindings.
                if !self.search.search_query.is_empty()
                    && matches!(
                        binding.as_ref(),
                        Binding::TextSelect(Action::SelectAll | Action::ClearSelection)
                    )
                {
                    return false;
                }
                chord.activation_matches(key)
                    && chord.keys().len() == pressed.len()
                    && chord.matches_pressed(pressed)
            })
            .map(|(_, binding)| binding.clone())
    }
    fn wants_pointer_events(&self) -> bool {
        false
    }
    fn claims_key(&self, key: &Key) -> bool {
        key.as_char().is_some()
            || (self.frame.level == Level::Regions
                && self.config.hint.overlap_cycle_key.activation_matches(key))
    }
    fn available_keys(&self) -> Vec<(String, String)> {
        if self.frame.level == Level::Regions {
            vec![(
                self.config.hint.overlap_cycle_key.canonical(),
                "cycle overlapping hints".into(),
            )]
        } else {
            Vec::new()
        }
    }
    fn handle_owned(&mut self, event: ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        match event {
            ModeEvent::UiScanned(result) => self.scan_event(result, ctx),
            ModeEvent::TextSegmented(result) => self.segmented(result, ctx),
            event => self.handle(&event, ctx),
        }
    }
    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        match event {
            ModeEvent::Activated { previous } => {
                self.region.reset();
                self.return_mode = previous
                    .clone()
                    .filter(|id| *id != self.id())
                    .unwrap_or_else(ModeId::idle);
                self.start(ctx)
            }
            ModeEvent::Pushed { previous } | ModeEvent::PushedInRegion { previous, .. } => {
                self.return_mode = previous.clone();
                let bounds = match event {
                    ModeEvent::PushedInRegion { bounds, .. } => Some(*bounds),
                    _ => None,
                };
                self.region.push(bounds);
                if bounds.is_some() {
                    self.bounds = bounds;
                }
                self.start(ctx)
            }
            ModeEvent::Restarted | ModeEvent::ScreensChanged(_) => self.start(ctx),
            ModeEvent::Deactivated => {
                self.region.reset();
                self.automatic_selection = None;
                self.preview_actions.clear();
                self.active = false;
                self.held_overlap_keys.clear();
                self.overlap_cycle = 0;
                self.layers.release_retained();
                self.wide_placements = None;
                self.source.clear_results();
                self.scene_cache.clear();
                self.search.clear_results();
                self.history.clear();
                self.frame = Frame::default();
                self.output = String::new();
                self.summary = String::new();
                self.base_selected = Vec::new();
                CommandBatch::from_iter([
                    Command::CancelTimer {
                        id: MAINTENANCE.into(),
                    },
                    Command::CancelTextSegmentation,
                    Command::ReleaseTextPrompt,
                    Command::HideOverlay,
                ])
            }
            ModeEvent::Resumed => self.finish(ctx, CommandBatch::new()),
            ModeEvent::Key {
                key,
                state,
                repeat: false,
            } if self.frame.level == Level::Regions
                && self.config.hint.overlap_cycle_key.activation_matches(key) =>
            {
                self.overlap_key(key, *state, ctx)
            }
            ModeEvent::Timer { id, .. } if self.active && id == MAINTENANCE => self.maintain(ctx),
            ModeEvent::UiScanned(result) => self.scan_event(result.clone(), ctx),
            ModeEvent::TextSegmented(result) => self.segmented(result.clone(), ctx),
            ModeEvent::PanelWindowBounds { id, bounds } if *id == self.scan_id => {
                self.window = *bounds;
                self.finish(ctx, CommandBatch::new())
            }
            ModeEvent::Binding {
                binding,
                state: KeyState::Down,
                ..
            } => match binding.as_ref() {
                Binding::TextSelect(action) => self.action(*action, ctx),
                Binding::SelectionReset => self.navigate(true, ctx),
                Binding::SelectionParent => self.navigate(false, ctx),
                Binding::SelectionAccept => self.action(Action::Copy, ctx),
                Binding::SelectionSearch => self.action(Action::Search, ctx),
                Binding::SelectionCancel | Binding::Escape => {
                    CommandBatch::one(self.region.return_command(&self.return_mode))
                }
                Binding::RescanUi => self.start(ctx),
                _ => CommandBatch::one(Command::DispatchActions(vec![(**binding).clone()])),
            },
            ModeEvent::TextInserted(c) => self.insert(c.encode_utf8(&mut [0; 4]), ctx),
            ModeEvent::TextPasted(text) => self.insert(text, ctx),
            ModeEvent::TextCopied if self.copying => {
                self.copying = false;
                self.region
                    .lifecycle_commands(&self.config.lifecycle.after_copy, &self.return_mode)
            }
            ModeEvent::TextSubmitted(None) | ModeEvent::TextEdit(EditAction::Cancel) => {
                CommandBatch::one(self.region.return_command(&self.return_mode))
            }
            ModeEvent::TextEdit(EditAction::Accept) => self.action(Action::Copy, ctx),
            ModeEvent::TextEdit(EditAction::Paste) => CommandBatch::one(Command::ReadClipboard),
            ModeEvent::TextEdit(action) => {
                self.automatic_selection = None;
                let changed = self
                    .search
                    .search_selection
                    .edit(&mut self.search.search_query, *action);
                if changed {
                    self.query_changed(false, ctx)
                } else {
                    self.finish(ctx, CommandBatch::new())
                }
            }
            _ => CommandBatch::new(),
        }
    }
}
