#![cfg(test)]

use super::*;
use crate::api::{Palette, Point, Screen, UiTarget};

struct Env {
    screens: Vec<Screen>,
    palette: Palette,
}
impl Env {
    fn new() -> Self {
        Self {
            screens: vec![Screen {
                bounds: Rect::new(0., 0., 1200., 900.),
                work_area: Rect::new(0., 0., 1200., 900.),
                is_primary: true,
                scale: 1.,
                name: None,
            }],
            palette: Palette::default(),
        }
    }
    fn ctx(&self) -> HostContext<'_> {
        HostContext {
            presenter: &crate::presentation::COMPOSER,
            screens: &self.screens,
            cursor: Point::new(100., 100.),
            focused_app: None,
            palette: &self.palette,
        }
    }
}
fn mode() -> TextSelectMode {
    crate::app::mode_catalog::text_select(&crate::config::Config::default())
}
fn settle(mode: &mut TextSelectMode, env: &Env) {
    for _ in 0..50 {
        if mode.search.search_names_initialized
            && !mode.search.search_pending
            && (!mode.source.pending_relabel || mode.frozen)
        {
            return;
        }
        mode.maintain(&env.ctx());
    }
    panic!("text search preparation did not converge");
}
fn deliver(mode: &mut TextSelectMode, env: &Env, status: UiScanStatus, text: &str) -> CommandBatch {
    let commands = mode.handle_owned(
        ModeEvent::UiScanned(UiScanResult {
            id: mode.scan_id,
            retired: Vec::new(),
            targets: vec![UiTarget::recognized_text(
                Rect::new(10., 10., 300., 20.),
                text.into(),
            )],
            status,
        }),
        &env.ctx(),
    );
    settle(mode, env);
    commands
}
fn words(mode: &mut TextSelectMode, env: &Env, text: &str, ranges: Vec<std::ops::Range<usize>>) {
    mode.start(&env.ctx());
    deliver(mode, env, UiScanStatus::Success, text);
    mode.insert("a", &env.ctx());
    mode.action(Action::Refine, &env.ctx());
    mode.segmented(
        Segmented {
            id: mode.segment_id,
            words: ranges,
        },
        &env.ctx(),
    );
    settle(mode, env);
}

fn fast_preview(rect: Rect) -> UiTarget {
    UiTarget {
        rect,
        role: crate::api::SemanticRole::Control,
        name: String::new(),
        details: None,
    }
}

fn scan_delta(
    mode: &mut TextSelectMode,
    env: &Env,
    targets: Vec<UiTarget>,
    retired: Vec<Rect>,
    status: UiScanStatus,
) -> CommandBatch {
    mode.scan(
        UiScanResult {
            id: mode.scan_id,
            targets,
            retired,
            status,
        },
        &env.ctx(),
    )
}

#[test]
fn region_entry_reuses_ocr_and_fast_previews_only_inside_the_grid() {
    let env = Env::new();
    let mut mode = mode();
    let area = Rect::new(10., 10., 200., 200.);
    let out = mode.handle(
        &ModeEvent::PushedInRegion {
            previous: ModeId::grid(),
            bounds: area,
        },
        &env.ctx(),
    );
    let request = out
        .iter()
        .find_map(|command| match command {
            Command::ScanUi(request) => Some(request),
            _ => None,
        })
        .unwrap();
    assert_eq!(request.scope, crate::api::UiScanScope::Screen);
    assert_eq!(request.bounds, Some(area));
    assert_eq!(request.capture_regions().collect::<Vec<_>>(), vec![area]);
    assert_eq!(request.strategy, UiScanStrategy::Vision);
    assert!(request.vision.detect_text);
    assert!(!request.vision.detect_rectangles);
    assert!(
        out.iter()
            .position(|command| matches!(command, Command::HideOverlay))
            .unwrap()
            < out
                .iter()
                .position(|command| matches!(command, Command::ScanUi(_)))
                .unwrap()
    );
    let inside = Rect::new(20., 20., 40., 10.);
    let outside = Rect::new(400., 20., 40., 10.);
    scan_delta(
        &mut mode,
        &env,
        vec![fast_preview(inside), fast_preview(outside)],
        vec![],
        UiScanStatus::Partial,
    );
    assert_eq!(mode.source.scanned.len(), 1);
    assert_eq!(mode.source.scanned[0].rect, inside);
    scan_delta(
        &mut mode,
        &env,
        vec![
            UiTarget::recognized_text(inside, "hello".into()),
            UiTarget::recognized_text(outside, "outside".into()),
        ],
        vec![inside, outside],
        UiScanStatus::Success,
    );
    settle(&mut mode, &env);
    assert!(
        mode.source
            .scanned
            .iter()
            .all(|target| area.contains(&target.rect.center()))
    );
    assert!(
        mode.source
            .scanned
            .iter()
            .any(|target| target.name == "hello")
    );

    let rescanned = mode.handle(&ModeEvent::Restarted, &env.ctx());
    assert!(rescanned.iter().any(
        |command| matches!(command, Command::ScanUi(request) if request.bounds == Some(area))
    ));
    assert_eq!(
        mode.handle(&ModeEvent::TextEdit(EditAction::Cancel), &env.ctx()),
        CommandBatch::one(Command::PopMode)
    );
    mode.handle(&ModeEvent::Deactivated, &env.ctx());
    let normal = mode.handle(
        &ModeEvent::Activated {
            previous: Some(ModeId::normal()),
        },
        &env.ctx(),
    );
    assert!(normal.iter().any(|command| matches!(command, Command::ScanUi(request)
        if request.scope == crate::api::UiScanScope::All && request.bounds == Some(env.screens[0].bounds))));
}

#[test]
fn fast_geometry_keeps_a_selected_label_until_accurate_text_can_be_copied() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    let rect = Rect::new(10., 10., 300., 20.);
    let first = scan_delta(
        &mut mode,
        &env,
        vec![fast_preview(rect)],
        vec![],
        UiScanStatus::Partial,
    );
    assert!(first.iter().any(|c| matches!(c, Command::ShowOverlay(_))));
    let label = mode.search.search_hints[0].label.as_str().to_owned();
    mode.insert(&format!("@{label}"), &env.ctx());
    assert!(mode.frame.selection_waiting_for_preview());
    assert!(mode.frozen);
    let pending = mode.action(Action::Copy, &env.ctx());
    assert!(!pending.iter().any(|c| matches!(c, Command::CopyText(_))));
    assert_eq!(mode.preview_actions.pending(), Some(Action::Copy));

    let partial = scan_delta(
        &mut mode,
        &env,
        vec![UiTarget::recognized_text(
            Rect::new(12., 10., 290., 20.),
            "设置 Settings".into(),
        )],
        vec![rect],
        UiScanStatus::Partial,
    );
    assert!(!partial.iter().any(|c| matches!(c, Command::CopyText(_))));
    let complete = scan_delta(&mut mode, &env, vec![], vec![], UiScanStatus::Success);
    assert!(
        complete
            .iter()
            .any(|c| matches!(c, Command::CopyText(text) if text == "设置 Settings"))
    );
    assert_eq!(mode.search.search_hints[0].label.as_str(), label);
    assert_eq!(mode.frame.region_bounds, [rect]);
    assert_eq!(mode.frame.selected, [true]);
    assert_eq!(mode.output, "设置 Settings");
    assert!(mode.source.pending_relabel);
}

#[test]
fn selecting_all_fast_regions_shows_the_waiting_status() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    let rect = Rect::new(10., 10., 300., 20.);
    scan_delta(
        &mut mode,
        &env,
        vec![fast_preview(rect)],
        vec![],
        UiScanStatus::Partial,
    );
    let selected = mode.action(Action::SelectAll, &env.ctx());
    assert!(selected.iter().any(|command| {
        match command {
            Command::ShowOverlay(scene) => scene
                .labels
                .iter()
                .any(|label| label.text.as_str() == "Recognizing text…"),
            _ => false,
        }
    }));
}

#[test]
fn fast_paragraph_waits_for_all_selected_rows_before_copying() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    let first = Rect::new(10., 10., 300., 20.);
    let second = Rect::new(10., 35., 300., 20.);
    scan_delta(
        &mut mode,
        &env,
        vec![fast_preview(first), fast_preview(second)],
        vec![],
        UiScanStatus::Partial,
    );
    mode.insert("@a", &env.ctx());
    mode.action(Action::Copy, &env.ctx());
    let partial = scan_delta(
        &mut mode,
        &env,
        vec![UiTarget::recognized_text(first, "第一行".into())],
        vec![first],
        UiScanStatus::Partial,
    );
    assert!(!partial.iter().any(|c| matches!(c, Command::CopyText(_))));
    assert!(mode.output.is_empty());
    assert_eq!(mode.preview_actions.pending(), Some(Action::Copy));
    let partial = scan_delta(
        &mut mode,
        &env,
        vec![UiTarget::recognized_text(second, "第二行".into())],
        vec![second],
        UiScanStatus::Partial,
    );
    assert!(!partial.iter().any(|c| matches!(c, Command::CopyText(_))));
    let complete = scan_delta(&mut mode, &env, vec![], vec![], UiScanStatus::Success);
    assert!(
        complete
            .iter()
            .any(|c| matches!(c, Command::CopyText(text) if text == "第一行第二行"))
    );
}

#[test]
fn selecting_fast_boxes_that_merge_into_one_ocr_row_copies_it_once() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    let left = Rect::new(10., 10., 80., 20.);
    let right = Rect::new(100., 10., 80., 20.);
    scan_delta(
        &mut mode,
        &env,
        vec![fast_preview(left), fast_preview(right)],
        vec![],
        UiScanStatus::Partial,
    );
    assert_eq!(mode.frame.units.len(), 2);
    mode.action(Action::SelectAll, &env.ctx());
    mode.action(Action::Copy, &env.ctx());
    scan_delta(
        &mut mode,
        &env,
        vec![UiTarget::recognized_text(
            Rect::new(10., 10., 170., 20.),
            "hello world".into(),
        )],
        vec![left, right],
        UiScanStatus::Partial,
    );
    let complete = scan_delta(&mut mode, &env, vec![], vec![], UiScanStatus::Success);
    assert!(
        complete
            .iter()
            .any(|c| matches!(c, Command::CopyText(text) if text == "hello world"))
    );
    assert_eq!(mode.output, "hello world");
}

#[test]
fn fast_selection_refines_only_after_accurate_split_rows_arrive() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    let rect = Rect::new(10., 10., 300., 20.);
    scan_delta(
        &mut mode,
        &env,
        vec![fast_preview(rect)],
        vec![],
        UiScanStatus::Partial,
    );
    mode.insert("@a", &env.ctx());
    let pending = mode.action(Action::Refine, &env.ctx());
    assert!(!pending.iter().any(|c| matches!(c, Command::SegmentText(_))));
    let partial = scan_delta(
        &mut mode,
        &env,
        vec![UiTarget::recognized_text(
            Rect::new(10., 10., 100., 20.),
            "hello".into(),
        )],
        vec![rect],
        UiScanStatus::Partial,
    );
    assert!(!partial.iter().any(|c| matches!(c, Command::SegmentText(_))));
    let partial = scan_delta(
        &mut mode,
        &env,
        vec![UiTarget::recognized_text(
            Rect::new(130., 10., 100., 20.),
            "world".into(),
        )],
        vec![],
        UiScanStatus::Partial,
    );
    assert!(!partial.iter().any(|c| matches!(c, Command::SegmentText(_))));
    let complete = scan_delta(&mut mode, &env, vec![], vec![], UiScanStatus::Success);
    assert!(complete.iter().any(
        |c| matches!(c, Command::SegmentText(request) if request.text.as_ref() == "hello world")
    ));
    assert_eq!(mode.frame.level, Level::Words);
}

#[test]
fn missing_preview_rows_never_copy_a_partial_selection() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    let first = Rect::new(10., 10., 300., 20.);
    let second = Rect::new(10., 35., 300., 20.);
    scan_delta(
        &mut mode,
        &env,
        vec![fast_preview(first), fast_preview(second)],
        vec![],
        UiScanStatus::Partial,
    );
    mode.insert("@a", &env.ctx());
    mode.action(Action::Copy, &env.ctx());
    scan_delta(
        &mut mode,
        &env,
        vec![UiTarget::recognized_text(first, "第一行".into())],
        vec![first],
        UiScanStatus::Partial,
    );
    let complete = scan_delta(&mut mode, &env, vec![], vec![second], UiScanStatus::Success);
    assert!(!complete.iter().any(|c| matches!(c, Command::CopyText(_))));
    assert!(mode.frame.selection_incomplete());
    assert!(mode.output.is_empty());
    assert_eq!(mode.preview_actions.pending(), None);
    assert!(
        !mode
            .action(Action::Copy, &env.ctx())
            .iter()
            .any(|c| matches!(c, Command::CopyText(_)))
    );
}

#[test]
fn changing_input_cancels_a_queued_preview_action() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    let rect = Rect::new(10., 10., 300., 20.);
    scan_delta(
        &mut mode,
        &env,
        vec![fast_preview(rect)],
        vec![],
        UiScanStatus::Partial,
    );
    mode.insert("@a", &env.ctx());
    mode.action(Action::Copy, &env.ctx());
    mode.action(Action::ClearInput, &env.ctx());
    let complete = scan_delta(
        &mut mode,
        &env,
        vec![UiTarget::recognized_text(rect, "hello".into())],
        vec![rect],
        UiScanStatus::Partial,
    );
    assert_eq!(mode.preview_actions.pending(), None);
    assert!(!complete.iter().any(|c| matches!(c, Command::CopyText(_))));
}

#[test]
fn preview_replacement_before_deferred_timer_cannot_be_lost_on_selection() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    let rect = Rect::new(10., 10., 300., 20.);
    scan_delta(
        &mut mode,
        &env,
        vec![fast_preview(rect)],
        vec![],
        UiScanStatus::Partial,
    );
    scan_delta(
        &mut mode,
        &env,
        vec![UiTarget::recognized_text(rect, "准确文字".into())],
        vec![rect],
        UiScanStatus::Partial,
    );
    assert!(mode.source.pending_relabel);
    mode.insert("@a", &env.ctx());
    let pending = mode.action(Action::Copy, &env.ctx());
    assert!(!pending.iter().any(|c| matches!(c, Command::CopyText(_))));
    let copy = scan_delta(&mut mode, &env, vec![], vec![], UiScanStatus::Success);
    assert!(
        copy.iter()
            .any(|c| matches!(c, Command::CopyText(text) if text == "准确文字"))
    );
}

#[test]
fn fast_label_prefix_stays_bound_to_the_same_region_while_ocr_streams() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    let rects: Vec<_> = (0..30)
        .map(|i| Rect::new(10., 10. + f64::from(i) * 25., 100., 10.))
        .collect();
    scan_delta(
        &mut mode,
        &env,
        rects.iter().copied().map(fast_preview).collect(),
        vec![],
        UiScanStatus::Partial,
    );
    let hint = mode
        .search
        .search_hints
        .iter()
        .find(|hint| hint.label.as_str().len() > 1)
        .unwrap();
    let index = hint.value;
    let label = hint.label.as_str().to_owned();
    mode.insert(&format!("@{}", &label[..1]), &env.ctx());
    assert!(mode.holds_preview_labels());
    assert!(!mode.frozen);
    scan_delta(
        &mut mode,
        &env,
        vec![UiTarget::recognized_text(rects[index], "正确目标".into())],
        vec![rects[index]],
        UiScanStatus::Partial,
    );
    mode.maintain(&env.ctx());
    assert!(mode.source.pending_relabel);
    let hint = mode
        .search
        .search_hints
        .iter()
        .find(|hint| hint.value == index)
        .unwrap();
    assert_eq!(hint.label.as_str(), label);
    assert_eq!(hint.bounds, rects[index]);
    mode.insert(&label[1..], &env.ctx());
    assert!(mode.frozen);
    mode.action(Action::Copy, &env.ctx());
    let complete = scan_delta(
        &mut mode,
        &env,
        vec![],
        rects
            .iter()
            .enumerate()
            .filter_map(|(i, rect)| (i != index).then_some(*rect))
            .collect(),
        UiScanStatus::Success,
    );
    assert!(
        complete
            .iter()
            .any(|c| matches!(c, Command::CopyText(text) if text == "正确目标"))
    );
}

#[test]
fn failed_and_stale_scans_cannot_complete_a_queued_copy() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    let previous_id = mode.scan_id;
    let rect = Rect::new(10., 10., 300., 20.);
    scan_delta(
        &mut mode,
        &env,
        vec![fast_preview(rect)],
        vec![],
        UiScanStatus::Partial,
    );
    mode.insert("@a", &env.ctx());
    mode.action(Action::Copy, &env.ctx());
    let failed = scan_delta(
        &mut mode,
        &env,
        vec![UiTarget::recognized_text(rect, "partial".into())],
        vec![rect],
        UiScanStatus::TimedOut,
    );
    assert!(!failed.iter().any(|c| matches!(c, Command::CopyText(_))));
    assert!(mode.frame.selection_incomplete());
    assert_eq!(mode.preview_actions.pending(), None);
    mode.start(&env.ctx());
    let stale = mode.scan(
        UiScanResult {
            id: previous_id,
            targets: vec![UiTarget::recognized_text(rect, "stale".into())],
            retired: vec![rect],
            status: UiScanStatus::Success,
        },
        &env.ctx(),
    );
    assert!(stale.is_empty());
    assert!(mode.frame.units.is_empty());
}

#[test]
fn already_recognized_selection_can_refine_while_other_preview_is_pending() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    let preview = Rect::new(10., 10., 300., 20.);
    let known = Rect::new(10., 100., 300., 20.);
    scan_delta(
        &mut mode,
        &env,
        vec![
            fast_preview(preview),
            UiTarget::recognized_text(known, "known".into()),
        ],
        vec![],
        UiScanStatus::Partial,
    );
    let label = mode
        .search
        .search_hints
        .iter()
        .find(|hint| hint.value == 1)
        .unwrap()
        .label
        .as_str()
        .to_owned();
    mode.insert(&format!("@{label}"), &env.ctx());
    let refine = mode.action(Action::Refine, &env.ctx());
    assert!(
        refine.iter().any(
            |c| matches!(c, Command::SegmentText(request) if request.text.as_ref() == "known")
        )
    );
    scan_delta(
        &mut mode,
        &env,
        vec![UiTarget::recognized_text(preview, "晚到文字".into())],
        vec![preview],
        UiScanStatus::Success,
    );
    mode.navigate(false, &env.ctx());
    assert_eq!(&*mode.frame.text, "晚到文字\n\nknown");
    assert_eq!(mode.frame.selected, [false, true]);
}

#[test]
fn first_hints_and_early_input_paint_before_bounded_search_prewarm() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    mode.insert("hello", &env.ctx());
    assert!(!mode.frozen);
    let commands = mode.scan(
        UiScanResult {
            id: mode.scan_id,
            retired: vec![],
            status: UiScanStatus::Partial,
            targets: vec![UiTarget::recognized_text(
                Rect::new(10., 10., 300., 20.),
                "hello world".into(),
            )],
        },
        &env.ctx(),
    );
    assert_eq!(mode.search.search_query, "hello");
    assert!(
        mode.search.scanned.is_empty(),
        "search borrows frame text instead of duplicating targets"
    );
    assert!(mode.search.search_text.is_empty());
    assert!(mode.search.search_pending);
    let paint = commands
        .iter()
        .position(|c| matches!(c, Command::ShowOverlay(_)))
        .unwrap();
    let warm = commands
        .iter()
        .position(|c| matches!(c, Command::SetTimer { .. }))
        .unwrap();
    assert!(paint < warm);
    settle(&mut mode, &env);
    assert_eq!(mode.output, "hello world");
    assert!(mode.frozen);
}

#[test]
fn an_early_unmatched_query_receives_matching_late_ocr() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    mode.insert("hello", &env.ctx());
    deliver(&mut mode, &env, UiScanStatus::Partial, "unrelated text");
    assert!(mode.output.is_empty());
    assert!(!mode.frozen);
    mode.scan(
        UiScanResult {
            id: mode.scan_id,
            retired: vec![],
            status: UiScanStatus::Success,
            targets: vec![UiTarget::recognized_text(
                Rect::new(600., 100., 200., 20.),
                "hello world".into(),
            )],
        },
        &env.ctx(),
    );
    settle(&mut mode, &env);
    assert_eq!(mode.search.search_query, "hello");
    assert_eq!(mode.output, "hello world");
    assert!(mode.frozen);
}

#[test]
fn explicit_labels_work_without_preparing_names_and_plain_c_remains_input() {
    let env = Env::new();
    let config = crate::config::Config::parse("[text_select]\nhint_characters = 'badc'\n[text_select.bindings]\nb = 'key_help'\na = 'none'\n'ctrl+d' = 'key_help'").unwrap();
    let mut mode = crate::app::mode_catalog::text_select(&config);
    assert_eq!(mode.config.alphabet, ['a', 'd', 'c']);
    mode.start(&env.ctx());
    mode.scan(
        UiScanResult {
            id: mode.scan_id,
            retired: vec![],
            status: UiScanStatus::Success,
            targets: vec![UiTarget::recognized_text(
                Rect::new(10., 10., 300., 20.),
                "copy".into(),
            )],
        },
        &env.ctx(),
    );
    mode.insert("@a", &env.ctx());
    assert_eq!(mode.output, "copy");
    assert!(mode.search.search_text.is_empty());
    let c = Key::new("c").unwrap();
    assert!(
        mode.keyboard_prompt_binding(&c, std::slice::from_ref(&c))
            .is_none()
    );
    mode.action(Action::Search, &env.ctx());
    mode.insert("c", &env.ctx());
    assert_eq!(mode.search.search_query, "c");
    assert_eq!(mode.frame.level, Level::Regions);
    settle(&mut mode, &env);
    assert_eq!(mode.output, "copy");
}

#[test]
fn preview_is_hidden_until_input_and_stays_visible_during_refinement() {
    let env = Env::new();
    let mut mode = mode();
    let preview = |mode: &TextSelectMode| {
        let Command::ShowOverlay(scene) = mode.render(&env.ctx()) else {
            panic!()
        };
        scene
            .labels
            .iter()
            .filter(|label| label.z_index == 10_002 && label.edit.is_none())
            .map(|label| label.text.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    mode.start(&env.ctx());
    assert!(preview(&mode).is_empty());
    deliver(&mut mode, &env, UiScanStatus::Success, "hello world");
    assert!(preview(&mode).is_empty());
    mode.insert("zzzz", &env.ctx());
    assert_eq!(preview(&mode), ["No matching text"]);
    mode.handle(&ModeEvent::TextEdit(EditAction::SelectAll), &env.ctx());
    mode.handle(&ModeEvent::TextEdit(EditAction::Backspace), &env.ctx());
    assert!(preview(&mode).is_empty());
    mode.insert("a", &env.ctx());
    assert!(preview(&mode).iter().any(|text| text == "hello world"));
    mode.action(Action::Refine, &env.ctx());
    assert!(mode.search.search_query.is_empty());
    assert_eq!(preview(&mode), ["hello world", "10 words · 11 chars"]);
    mode.segmented(
        Segmented {
            id: mode.segment_id,
            words: vec![0..5, 6..11],
        },
        &env.ctx(),
    );
    assert!(preview(&mode).iter().any(|text| text == "hello world"));
    mode.navigate(true, &env.ctx());
    assert!(preview(&mode).is_empty());
}

#[test]
fn word_and_character_selection_stays_in_the_panel_and_restores_region_geometry() {
    use crate::api::OverlayShape;
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    deliver(&mut mode, &env, UiScanStatus::Success, "iW");
    mode.insert("a", &env.ctx());
    let original = mode.frame.region_bounds.clone();
    assert_eq!(original, [Rect::new(10., 10., 300., 20.)]);
    let panel_only = |mode: &TextSelectMode| {
        assert!(mode.frame.region_bounds.is_empty());
        assert!(mode.search.scanned.is_empty());
        assert!(mode.selection_pointer().is_empty());
        let Command::ShowOverlay(scene) = mode.render(&env.ctx()) else {
            panic!()
        };
        let scene = scene.materialized();
        assert!(
            !scene
                .shapes
                .iter()
                .any(|shape| matches!(shape, OverlayShape::Rect { z_index: 0, .. }))
        );
        assert!(scene.labels.iter().all(|label| label.rect.y >= 100.));
        scene
    };
    mode.refine(&env.ctx());
    mode.segmented(
        Segmented {
            id: mode.segment_id,
            words: std::iter::once(0..2).collect(),
        },
        &env.ctx(),
    );
    let scene = panel_only(&mode);
    assert!(scene.labels.iter().any(|label| label.text.as_str() == "iW"));
    mode.refine(&env.ctx());
    settle(&mut mode, &env);
    mode.insert("2 ", &env.ctx());
    assert_eq!(mode.output, "i");
    let scene = panel_only(&mode);
    assert!(scene.labels.iter().any(|label| label.text.as_str() == "W"));
    assert!(
        scene
            .labels
            .iter()
            .any(|label| label.text.as_str() == "✓ 1")
    );
    assert_eq!(
        mode.action(Action::Copy, &env.ctx()),
        CommandBatch::one(Command::CopyText("i".into()))
    );
    mode.navigate(false, &env.ctx());
    assert_eq!(mode.frame.level, Level::Words);
    assert_eq!(mode.output, "iW");
    panel_only(&mode);
    mode.navigate(false, &env.ctx());
    assert_eq!(mode.frame.level, Level::Regions);
    assert_eq!(mode.frame.region_bounds, original);
    assert_eq!(mode.frame.selected, [true]);
    assert_eq!(mode.output, "iW");
}

#[test]
fn scan_scope_uses_text_selection_setting_and_its_display_bounds() {
    use crate::api::UiScanScope;
    let mut env = Env::new();
    env.screens.push(Screen {
        bounds: Rect::new(-1400., -100., 1200., 900.),
        work_area: Rect::new(-1400., -100., 1200., 900.),
        is_primary: false,
        scale: 2.,
        name: None,
    });
    for scope in [
        UiScanScope::Window,
        UiScanScope::Active,
        UiScanScope::Screen,
        UiScanScope::All,
    ] {
        let mut config = crate::config::Config::default();
        config.text_select.scan_scope = scope;
        config.ui_hint.scan_scope = if scope == UiScanScope::All {
            UiScanScope::Screen
        } else {
            UiScanScope::All
        };
        let mut mode = crate::app::mode_catalog::text_select(&config);
        let commands = mode.start(&env.ctx());
        let request = commands
            .iter()
            .find_map(|command| match command {
                Command::ScanUi(request) => Some(request),
                _ => None,
            })
            .unwrap();
        assert_eq!(request.scope, scope);
        let screens = if scope == UiScanScope::All {
            &env.screens[..]
        } else {
            &env.screens[..1]
        };
        assert_eq!(request.bounds, Some(Screen::virtual_bounds(screens)));
        assert_eq!(
            request.capture_regions().collect::<Vec<_>>(),
            screens.iter().map(|s| s.bounds).collect::<Vec<_>>()
        );
        assert_eq!(request.strategy, UiScanStrategy::Vision);
        assert!(request.vision.detect_text);
        assert!(!request.vision.detect_rectangles);
    }
}

#[test]
fn ocr_only_entry_uses_english_indicator_and_text_only_panel_without_clicking() {
    let env = Env::new();
    let mut mode = mode();
    let commands = mode.start(&env.ctx());
    let request = commands
        .iter()
        .find_map(|c| {
            if let Command::ScanUi(r) = c {
                Some(r)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(request.strategy, UiScanStrategy::Vision);
    assert!(request.vision.detect_text);
    assert!(!request.vision.detect_rectangles);
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, Command::OpenTextPrompt(_)))
    );
    assert_eq!(mode.display_name(), "Text Select");
    assert_eq!(mode.indicator_detail().as_deref(), Some("Regions"));
    deliver(&mut mode, &env, UiScanStatus::Partial, "键盘选择文字");
    let selected = mode.insert("a", &env.ctx());
    assert_eq!(mode.output, "键盘选择文字");
    assert!(
        selected
            .iter()
            .any(|c| matches!(c, Command::WarpPointer { .. }))
    );
    assert!(
        !selected
            .iter()
            .any(|c| matches!(c, Command::MouseButton { .. } | Command::SwitchMode(_)))
    );
    let scene = selected
        .iter()
        .find_map(|c| {
            if let Command::ShowOverlay(s) = c {
                Some(s)
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        scene
            .labels
            .iter()
            .any(|l| l.text.as_str() == "键盘选择文字")
    );
    assert!(
        !scene
            .labels
            .iter()
            .any(|l| l.text.as_str().contains("文字选择") || l.text.as_str().contains("Ctrl+1"))
    );
    deliver(&mut mode, &env, UiScanStatus::Success, "迟到的识别结果");
    assert_eq!(mode.output, "键盘选择文字");
}

#[test]
fn refined_layers_start_selected_and_numbers_toggle_without_filtering() {
    let env = Env::new();
    let mut mode = mode();
    words(&mut mode, &env, "one two three", vec![0..3, 4..7, 8..13]);
    assert_eq!(mode.output, "one two three");
    let codes: Vec<_> = mode
        .search
        .hints
        .iter()
        .map(|hint| hint.label.as_str().to_owned())
        .collect();
    mode.insert("3 1 ", &env.ctx());
    assert_eq!(mode.output, "two");
    assert!(mode.search.search_query.is_empty());
    mode.insert("   ", &env.ctx());
    assert_eq!(mode.output, "two");
    mode.insert("1 3 ", &env.ctx());
    assert_eq!(mode.output, "two one three");
    mode.insert("1 1 ", &env.ctx());
    assert_eq!(
        mode.output, "two three one",
        "each complete label toggles once; reselected items move to the end"
    );
    assert_eq!(
        mode.search
            .hints
            .iter()
            .map(|hint| hint.label.as_str())
            .collect::<Vec<_>>(),
        codes
    );
    mode.insert("1 2 3 ", &env.ctx());
    assert!(mode.output.is_empty());
    assert_eq!(mode.search.hints.len(), 3);
    assert!(
        mode.action(Action::Copy, &env.ctx())
            .iter()
            .all(|c| !matches!(c, Command::CopyText(_)))
    );
}

#[test]
fn refined_selection_order_survives_copy_refinement_and_select_all() {
    let env = Env::new();
    let mut mode = mode();
    words(&mut mode, &env, "one two three", vec![0..3, 4..7, 8..13]);
    mode.action(Action::ClearSelection, &env.ctx());
    mode.insert("3 1 ", &env.ctx());
    assert_eq!(mode.output, "three one");
    let Command::ShowOverlay(scene) = mode.render(&env.ctx()) else {
        panic!()
    };
    assert!(scene.labels.iter().any(|label| label.text == "three one"));
    assert_eq!(
        mode.action(Action::Copy, &env.ctx()),
        CommandBatch::one(Command::CopyText("three one".into()))
    );
    mode.action(Action::Refine, &env.ctx());
    assert_eq!(mode.frame.level, Level::Characters);
    assert_eq!(mode.output, "three one");
    mode.action(Action::ClearSelection, &env.ctx());
    for unit in [6, 0, 4] {
        let code = mode.search.hints[unit].label.as_str().to_owned();
        mode.insert(&format!("{code} "), &env.ctx());
    }
    assert_eq!(mode.output, "nte");
    mode.navigate(false, &env.ctx());
    assert_eq!(mode.output, "three one");
    mode.action(Action::SelectAll, &env.ctx());
    assert_eq!(mode.output, "three one two", "all appends missing items");
    mode.action(Action::SelectAll, &env.ctx());
    assert_eq!(mode.output, "three one two", "all never duplicates items");
    mode.action(Action::ClearSelection, &env.ctx());
    mode.action(Action::SelectAll, &env.ctx());
    assert_eq!(mode.output, "one two three");
}

#[test]
fn partial_labels_and_paging_do_not_change_checks_or_hide_choices() {
    let env = Env::new();
    let mut mode = mode();
    let text = (0..32)
        .map(|i| format!("word{i:02}"))
        .collect::<Vec<_>>()
        .join(" ");
    let ranges = (0..32).map(|i| i * 7..i * 7 + 6).collect();
    words(&mut mode, &env, &text, ranges);
    let hint = mode
        .search
        .hints
        .iter()
        .enumerate()
        .find(|(_, hint)| hint.label.as_str().len() > 1)
        .unwrap();
    let (index, unit, code) = (hint.0, hint.1.value, hint.1.label.as_str().to_owned());
    mode.insert(&code[..1], &env.ctx());
    assert!(mode.frame.selected.iter().all(|selected| *selected));
    assert_eq!(mode.search.hints.len(), 32);
    mode.handle(&ModeEvent::TextEdit(EditAction::Backspace), &env.ctx());
    assert!(mode.frame.selected.iter().all(|selected| *selected));
    mode.insert(&format!("{code} "), &env.ctx());
    assert!(!mode.frame.selected[unit]);
    assert_eq!(mode.frame.selected.iter().filter(|v| **v).count(), 31);
    assert_eq!(mode.page, index / PAGE_SIZE);
    mode.action(Action::CyclePage, &env.ctx());
    mode.action(Action::CyclePage, &env.ctx());
    assert!(!mode.frame.selected[unit]);
    assert_eq!(mode.search.hints.len(), 32);
    let Command::ShowOverlay(scene) = mode.render(&env.ctx()) else {
        panic!()
    };
    assert_eq!(
        scene
            .shapes
            .iter()
            .filter(|shape| matches!(
                shape,
                crate::api::OverlayShape::Rect {
                    z_index: 10_001,
                    ..
                }
            ))
            .count(),
        PAGE_SIZE
    );
    mode.action(Action::ClearSelection, &env.ctx());
    for unit in [31, 0, 24] {
        while mode.page != unit / PAGE_SIZE {
            mode.action(Action::CyclePage, &env.ctx());
        }
        let code = mode.search.hints[unit].label.as_str().to_owned();
        for character in code.chars() {
            mode.insert(&character.to_string(), &env.ctx());
        }
        mode.insert(" ", &env.ctx());
        mode.action(Action::CyclePage, &env.ctx());
    }
    assert_eq!(mode.output, "word31 word00 word24");
    assert_eq!(
        mode.action(Action::Copy, &env.ctx()),
        CommandBatch::one(Command::CopyText("word31 word00 word24".into()))
    );
}

#[test]
fn decimal_ranges_commit_whole_numbers_and_combine_in_input_order() {
    let env = Env::new();
    let mut mode = mode();
    let items: Vec<_> = (1..=32).map(|i| format!("word{i:02}")).collect();
    words(&mut mode, &env, &items.join(" "), vec![]);
    assert_eq!(
        mode.search
            .hints
            .iter()
            .map(|hint| hint.label.as_str().parse::<usize>().unwrap())
            .collect::<Vec<_>>(),
        (1..=32).collect::<Vec<_>>()
    );
    assert!(
        mode.search.search_text.is_empty(),
        "numeric input needs no text index"
    );
    mode.action(Action::ClearSelection, &env.ctx());
    for character in "1-2".chars() {
        mode.insert(&character.to_string(), &env.ctx());
        assert!(
            mode.output.is_empty(),
            "a partial range must not select a shorter number"
        );
    }
    mode.insert("0", &env.ctx());
    assert_eq!(mode.output, items[..20].join(" "));
    mode.insert(" ", &env.ctx());
    assert_eq!(mode.output, items[..20].join(" "));
    assert!(mode.search.search_query.is_empty());
    mode.action(Action::ClearSelection, &env.ctx());
    mode.insert("24-26 1-3 4-5 ", &env.ctx());
    let expected = items[23..26]
        .iter()
        .chain(&items[..5])
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(mode.output, expected);
    mode.insert("2-5 ", &env.ctx());
    assert_eq!(
        mode.output, expected,
        "overlapping ranges select without toggling or duplicating"
    );
    mode.action(Action::ClearSelection, &env.ctx());
    mode.insert("5-2", &env.ctx());
    let commands = mode.action(Action::Copy, &env.ctx());
    assert_eq!(mode.output, "word05 word04 word03 word02");
    assert!(matches!(
        commands.iter().next(),
        Some(Command::ShowOverlay(_))
    ));
    assert!(
        matches!(commands.iter().last(), Some(Command::CopyText(text)) if text == "word05 word04 word03 word02")
    );
    assert!(mode.search.search_query.is_empty());
    let before = mode.output.clone();
    mode.action(Action::Copy, &env.ctx());
    assert_eq!(
        mode.output, before,
        "copy must not reapply committed numbers"
    );
    mode.action(Action::ClearSelection, &env.ctx());
    mode.insert("12", &env.ctx());
    assert_eq!(mode.output, "word12");
    mode.action(Action::Refine, &env.ctx());
    assert_eq!(mode.frame.level, Level::Characters);
    assert_eq!(mode.output, "word12");
}

#[test]
fn unambiguous_numbers_toggle_immediately_and_can_extend_into_ranges() {
    let env = Env::new();
    let mut mode = mode();
    let items: Vec<_> = (1..=20).map(|i| format!("word{i:02}")).collect();
    words(&mut mode, &env, &items.join(" "), vec![]);
    mode.action(Action::ClearSelection, &env.ctx());
    for digit in ["8", "9"] {
        mode.insert(digit, &env.ctx());
        assert!(mode.search.search_query.is_empty());
    }
    assert_eq!(mode.output, "word08 word09");
    mode.insert("8", &env.ctx());
    assert_eq!(mode.output, "word09");
    mode.action(Action::ClearSelection, &env.ctx());
    mode.insert("1", &env.ctx());
    assert!(mode.output.is_empty());
    mode.insert("2", &env.ctx());
    assert_eq!(mode.output, "word12");
    assert!(mode.search.search_query.is_empty());
    mode.insert("-", &env.ctx());
    assert!(mode.output.is_empty());
    assert_eq!(mode.search.search_query, "12-");
    mode.insert("1", &env.ctx());
    assert!(mode.output.is_empty());
    mode.insert("5", &env.ctx());
    assert_eq!(mode.output, "word12 word13 word14 word15");
    assert!(mode.search.search_query.is_empty());

    mode.action(Action::ClearSelection, &env.ctx());
    mode.insert("3 8 1 ", &env.ctx());
    assert_eq!(mode.output, "word03 word08 word01");
    mode.insert("8", &env.ctx());
    assert_eq!(mode.output, "word03 word01");
    mode.insert("-", &env.ctx());
    assert_eq!(
        mode.output, "word03 word08 word01",
        "starting a range restores the original selection position"
    );
    mode.insert("9", &env.ctx());
    assert_eq!(mode.output, "word03 word08 word01 word09");
    assert!(mode.search.search_query.is_empty());
    mode.insert("4", &env.ctx());
    let expected = mode.output.clone();
    mode.action(Action::ClearInput, &env.ctx());
    mode.insert("-", &env.ctx());
    assert_eq!(
        mode.output, expected,
        "an intervening action ends range continuation"
    );
    assert_eq!(mode.search.search_query, "-");
}

#[test]
fn automatic_number_resolution_uses_displayed_candidates_and_preserves_text_search() {
    let env = Env::new();
    let mut mode = mode();
    let items: Vec<_> = (1..=120).map(|i| format!("word{i:03}")).collect();
    words(&mut mode, &env, &items.join(" "), vec![]);
    mode.action(Action::ClearSelection, &env.ctx());
    mode.insert("9", &env.ctx());
    assert!(mode.search.search_query.is_empty());
    assert_eq!(
        mode.output, "word009",
        "90 on another page does not delay 9"
    );
    mode.action(Action::ClearSelection, &env.ctx());
    mode.insert("word009 ", &env.ctx());
    assert_eq!(mode.search.hints.len(), 1);
    assert_eq!(mode.search.hints[0].label.as_str(), "9");
    mode.insert("9", &env.ctx());
    assert_eq!(mode.search.search_query, "word009 ");
    assert_eq!(mode.output, "word009");
    mode.action(Action::ClearInput, &env.ctx());
    for _ in 0..3 {
        mode.action(Action::CyclePage, &env.ctx());
    }
    mode.insert("81", &env.ctx());
    assert_eq!(mode.page, 80 / PAGE_SIZE);
    assert_eq!(mode.output, "word009 word081");
}

#[test]
fn edited_or_waiting_numbers_resolve_when_the_panel_has_a_unique_candidate() {
    let env = Env::new();
    let mut mode = mode();
    let text = (1..=120)
        .map(|i| format!("word{i:03}"))
        .collect::<Vec<_>>()
        .join(" ");
    words(&mut mode, &env, &text, vec![]);
    mode.action(Action::ClearSelection, &env.ctx());
    mode.insert("90", &env.ctx());
    assert!(mode.output.is_empty());
    mode.handle(&ModeEvent::TextEdit(EditAction::Backspace), &env.ctx());
    assert_eq!(mode.output, "word009");
    assert!(mode.search.search_query.is_empty());
    mode.action(Action::ClearSelection, &env.ctx());
    mode.action(Action::CyclePage, &env.ctx());
    mode.insert("9", &env.ctx());
    assert_eq!(mode.search.search_query, "9");
    while mode.page != 0 {
        mode.action(Action::CyclePage, &env.ctx());
    }
    assert_eq!(mode.output, "word009");
    assert!(mode.search.search_query.is_empty());

    words(
        &mut mode,
        &env,
        &format!("{} needle", "word ".repeat(2200)),
        vec![],
    );
    mode.action(Action::ClearSelection, &env.ctx());
    mode.insert("needle 2201", &env.ctx());
    assert!(mode.search.search_pending);
    assert!(mode.output.is_empty());
    settle(&mut mode, &env);
    assert_eq!(mode.output, "needle");
    assert_eq!(mode.search.search_query, "needle ");
}

#[test]
fn numeric_input_waits_for_word_segmentation_before_resolving_ids() {
    let env = Env::new();
    for (input, remaining) in [
        ("2 ", ""),
        ("2", ""),
        ("world 2 ", "world "),
        ("world 2", "world "),
    ] {
        let mut mode = mode();
        mode.start(&env.ctx());
        deliver(&mut mode, &env, UiScanStatus::Success, "hello world");
        mode.insert("a", &env.ctx());
        mode.action(Action::Refine, &env.ctx());
        assert!(mode.pending_segment);
        assert!(mode.frame.units.is_empty());
        assert!(mode.frame.selected.is_empty());
        assert!(mode.search.search_hints.is_empty());
        assert!(mode.search.hints.is_empty());
        assert!(mode.base_selected.is_empty());
        mode.insert(input, &env.ctx());
        assert_eq!(mode.search.search_query, input);
        assert!(
            !mode
                .action(Action::Copy, &env.ctx())
                .iter()
                .any(|command| matches!(command, Command::CopyText(_)))
        );
        assert!(mode.search.search_text.is_empty());
        assert_eq!(mode.output, "hello world");
        mode.segmented(
            Segmented {
                id: mode.segment_id,
                words: vec![0..5, 6..11],
            },
            &env.ctx(),
        );
        let commands = mode.action(Action::Copy, &env.ctx());
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::CopyText(text) if text == "hello"))
        );
        assert_eq!(mode.frame.selected, [true, false]);
        assert_eq!(mode.search.search_query, remaining);
    }
}

#[test]
fn refined_search_filters_all_pages_without_renumbering_or_changing_selection() {
    let env = Env::new();
    let mut mode = mode();
    let mut items: Vec<_> = (1..=32).map(|i| format!("word{i:02}")).collect();
    items[30] = "Needle".into();
    words(&mut mode, &env, &items.join(" "), vec![]);
    mode.action(Action::ClearSelection, &env.ctx());
    mode.insert("2 1 ", &env.ctx());
    let before = mode.output.clone();
    mode.insert("NEED", &env.ctx());
    settle(&mut mode, &env);
    assert_eq!(mode.output, before);
    assert_eq!(mode.search.hints.len(), 1);
    assert_eq!(mode.search.hints[0].label.as_str(), "31");
    assert_eq!(mode.search.hints[0].value, 30);
    mode.insert(" 31 ", &env.ctx());
    assert_eq!(mode.output, "word02 word01 Needle");
    assert_eq!(mode.search.search_query, "NEED ");
    let Command::ShowOverlay(scene) = mode.render(&env.ctx()) else {
        panic!()
    };
    assert!(scene.labels.iter().any(|label| label.text == "✓ 31"));
    mode.action(Action::ClearInput, &env.ctx());
    assert_eq!(mode.search.hints.len(), 32);
    assert_eq!(mode.output, "word02 word01 Needle");
    mode.insert("unmatched", &env.ctx());
    assert!(mode.search.hints.is_empty());
    assert_eq!(mode.output, "word02 word01 Needle");
    assert_eq!(
        mode.action(Action::Copy, &env.ctx()),
        CommandBatch::one(Command::CopyText("word02 word01 Needle".into()))
    );

    words(
        &mut mode,
        &env,
        "使用键盘选择文字",
        vec![0..6, 6..12, 12..18, 18..24],
    );
    mode.insert("jp", &env.ctx());
    assert_eq!(mode.search.hints.len(), 1);
    assert_eq!(mode.search.hints[0].label.as_str(), "2");
    assert_eq!(mode.output, "使用键盘选择文字");
    mode.action(Action::ClearInput, &env.ctx());
    mode.action(Action::Refine, &env.ctx());
    mode.insert("盘", &env.ctx());
    assert_eq!(mode.search.hints.len(), 1);
    assert_eq!(mode.search.hints[0].label.as_str(), "4");
    assert_eq!(mode.output, "使用键盘选择文字");
}

#[test]
fn refined_search_prepares_large_levels_cooperatively() {
    let env = Env::new();
    let mut mode = mode();
    let text = format!("{} needle", "word ".repeat(2200));
    words(&mut mode, &env, &text, vec![]);
    let before = mode.output.clone();
    let commands = mode.insert("needle", &env.ctx());
    assert_eq!(mode.search.search_text.len(), 256);
    assert!(mode.search.search_pending);
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, Command::SetTimer { id, .. } if id == MAINTENANCE))
    );
    assert!(mode.search.hints.is_empty());
    settle(&mut mode, &env);
    assert!(!mode.search.search_pending);
    assert_eq!(mode.search.hints.len(), 1);
    assert_eq!(mode.search.hints[0].label.as_str(), "2201");
    assert_eq!(mode.output, before);
    mode.action(Action::ClearInput, &env.ctx());
    assert_eq!(mode.search.hints.len(), 2201);
    mode.insert("absent", &env.ctx());
    assert!(
        !mode.search.search_pending,
        "the prepared text index is reused"
    );
    assert!(mode.search.hints.is_empty());
}

#[test]
fn summary_counts_ascii_line_breaks_and_unicode_graphemes() {
    let mut mode = mode();
    for text in [
        "abc",
        "\r",
        "\r\n",
        "\n\r",
        "\r\r\n\n",
        "e\u{301}中👨‍👩‍👧‍👦",
        "🇨🇳\r\n🇺🇸",
    ] {
        mode.frame.text = text.into();
        mode.frame.units.clear();
        mode.frame.units.push(0..text.len());
        mode.frame.selected = vec![true];
        mode.update_summary();
        assert_eq!(mode.output, text);
        assert_eq!(
            mode.summary,
            format!("1 regions · {} chars", text.graphemes(true).count())
        );
    }
}

#[test]
fn editing_numbers_in_search_text_waits_for_explicit_commit() {
    let env = Env::new();
    let mut mode = mode();
    words(&mut mode, &env, "one two three", vec![]);
    mode.action(Action::ClearSelection, &env.ctx());
    mode.insert(" one", &env.ctx());
    mode.handle(&ModeEvent::TextEdit(EditAction::Home), &env.ctx());
    mode.insert("1", &env.ctx());
    assert_eq!(mode.search.search_query, "1 one");
    assert!(
        mode.output.is_empty(),
        "existing whitespace must not prematurely commit an edited number"
    );
    mode.insert("2", &env.ctx());
    assert_eq!(mode.search.search_query, "12 one");
    mode.handle(&ModeEvent::TextEdit(EditAction::Backspace), &env.ctx());
    assert!(mode.output.is_empty());
    mode.action(Action::Copy, &env.ctx());
    assert_eq!(mode.output, "one");
    assert_eq!(mode.search.search_query, "one");
    assert_eq!(mode.search.search_selection, Selection::default());
    mode.insert("0 1-4 1- ", &env.ctx());
    assert_eq!(mode.output, "one");

    mode.action(Action::ClearInput, &env.ctx());
    mode.action(Action::ClearSelection, &env.ctx());
    mode.insert(&format!(" one{}", "界".repeat(4090)), &env.ctx());
    mode.handle(&ModeEvent::TextEdit(EditAction::Home), &env.ctx());
    mode.insert("1", &env.ctx());
    mode.insert("2 ", &env.ctx());
    assert_eq!(mode.search.search_query.chars().count(), 4096);
    assert!(mode.search.search_query.starts_with("12 one"));
    assert!(
        mode.output.is_empty(),
        "discarded whitespace must not commit numbers"
    );
    mode.handle(&ModeEvent::TextEdit(EditAction::SelectAll), &env.ctx());
    mode.insert("2 ", &env.ctx());
    assert_eq!(mode.output, "two");
    assert!(mode.search.search_query.is_empty());
}

#[test]
fn clearing_refinement_input_preserves_checked_items_and_pending_segmentation() {
    let env = Env::new();
    let text = (0..32)
        .map(|i| format!("word{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let ranges = text
        .match_indices("word")
        .map(|(start, _)| {
            start
                ..text[start..]
                    .find(' ')
                    .map_or(text.len(), |end| start + end)
        })
        .collect();
    let mut mode = mode();
    words(&mut mode, &env, &text, ranges);
    mode.frame.toggle(3);
    mode.update_summary();
    for level in [Level::Words, Level::Characters] {
        if level == Level::Characters {
            mode.action(Action::Refine, &env.ctx());
        }
        let selected = mode.frame.selected.clone();
        let output = mode.output.clone();
        let prefix = mode
            .search
            .hints
            .iter()
            .find(|hint| hint.label.as_str().len() > 1)
            .unwrap()
            .label
            .as_str()[..1]
            .to_string();
        mode.insert(&prefix, &env.ctx());
        assert!(!mode.search.search_query.is_empty());
        let commands = mode.action(Action::ClearInput, &env.ctx());
        assert!(mode.search.search_query.is_empty());
        assert_eq!(mode.search.search_selection, Selection::default());
        assert_eq!(mode.frame.level, level);
        assert_eq!(mode.frame.selected, selected);
        assert_eq!(mode.output, output);
        assert!(!commands.iter().any(|c| matches!(
            c,
            Command::CopyText(_) | Command::CopyInputText { .. } | Command::SwitchMode(_)
        )));
    }
    mode.start(&env.ctx());
    deliver(&mut mode, &env, UiScanStatus::Success, "hello world");
    mode.insert("a", &env.ctx());
    mode.action(Action::Refine, &env.ctx());
    assert!(mode.pending_segment);
    mode.insert("a", &env.ctx());
    let segment_id = mode.segment_id;
    mode.action(Action::ClearInput, &env.ctx());
    assert!(mode.pending_segment);
    assert_eq!(mode.segment_id, segment_id);
    mode.segmented(
        Segmented {
            id: segment_id,
            words: vec![0..5, 6..11],
        },
        &env.ctx(),
    );
    assert_eq!(mode.frame.selected, [true, true]);
    assert!(mode.search.search_query.is_empty());
}

#[test]
fn region_outline_style_and_pointer_offset_are_independent() {
    use crate::api::{Appearance, OverlayShape};
    let mut env = Env::new();
    let config = crate::config::Config::parse(
        r##"
[text_select.ui]
pointer_offset = [20, 18]
border_color = {light = "#112233FF", dark = "#AABBCCFF"}
border_width = 2.5
padding = [5, 3]
"##,
    )
    .unwrap();
    let mut mode = crate::app::mode_catalog::text_select(&config);
    mode.start(&env.ctx());
    deliver(&mut mode, &env, UiScanStatus::Success, "hello");
    let commands = mode.insert("a", &env.ctx());
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, Command::WarpPointer { x: 330.0, y: 48.0 }))
    );
    for (appearance, color) in [
        (Appearance::Light, Color::rgb(0x11, 0x22, 0x33)),
        (Appearance::Dark, Color::rgb(0xaa, 0xbb, 0xcc)),
    ] {
        env.palette.appearance = appearance;
        let Command::ShowOverlay(scene) = mode.render(&env.ctx()) else {
            panic!()
        };
        let scale = crate::presentation::label_scale(env.ctx().scale());
        assert!(scene.shapes.iter().any(|shape| matches!(shape, OverlayShape::Rect {rect, stroke, stroke_width, z_index: 0, ..}
            if *rect == Rect::new(10. - 5. * scale, 10. - 3. * scale, 300. + 10. * scale, 20. + 6. * scale)
                && *stroke == color && *stroke_width == 2.5 * scale)));
    }
    let bounds = mode.frame.region_bounds[0];
    for (width, selected, bounds) in [
        (0., true, bounds),
        (2.5, false, bounds),
        (2.5, true, Rect::default()),
    ] {
        mode.config.ui.border_width = width;
        mode.frame.selected[0] = selected;
        mode.frame.region_bounds[0] = bounds;
        let Command::ShowOverlay(scene) = mode.render(&env.ctx()) else {
            panic!()
        };
        assert!(
            scene
                .shapes
                .iter()
                .all(|shape| !matches!(shape, OverlayShape::Rect { z_index: 0, .. }))
        );
    }
}

#[test]
fn words_and_characters_use_panel_choices_and_preserve_parent_checks() {
    let env = Env::new();
    let mut mode = mode();
    words(
        &mut mode,
        &env,
        "使用键盘选择文字",
        vec![0..6, 6..12, 12..18, 18..24],
    );
    assert_eq!(mode.search.hints.len(), 4);
    assert!(mode.search.hints.iter().all(|hint| hint.bounds.is_empty()));
    let Command::ShowOverlay(scene) = mode.render(&env.ctx()) else {
        panic!()
    };
    let scene = scene.materialized();
    assert!(scene.labels.iter().all(|label| label.rect.y >= 100.));
    for word in ["使用", "键盘", "选择", "文字"] {
        assert!(scene.labels.iter().any(|label| label.text.as_str() == word));
    }
    mode.insert("2 4 ", &env.ctx());
    assert_eq!(mode.output, "使用选择");
    mode.action(Action::Refine, &env.ctx());
    settle(&mut mode, &env);
    assert_eq!(mode.frame.level, Level::Characters);
    let Command::ShowOverlay(scene) = mode.render(&env.ctx()) else {
        panic!()
    };
    let scene = scene.materialized();
    assert!(
        !scene
            .labels
            .iter()
            .any(|label| label.text.as_str().eq_ignore_ascii_case("a") && label.rect.y < 100.)
    );
    mode.insert("1 3 ", &env.ctx());
    assert_eq!(mode.output, "用择");
    mode.navigate(false, &env.ctx());
    assert_eq!(mode.output, "使用选择");
    mode.navigate(false, &env.ctx());
    assert_eq!(mode.output, "使用键盘选择文字");
    mode.action(Action::Refine, &env.ctx());
    let stale = mode.segment_id;
    mode.navigate(true, &env.ctx());
    assert!(
        mode.segmented(
            Segmented {
                id: stale,
                words: std::iter::once(0..24).collect()
            },
            &env.ctx()
        )
        .is_empty()
    );
    assert_eq!(mode.frame.level, Level::Regions);
    assert!(mode.output.is_empty());
}

#[test]
fn input_caret_reuses_hint_scene_and_changed_results_replace_it() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    mode.scan(
        UiScanResult {
            id: mode.scan_id,
            retired: Vec::new(),
            status: UiScanStatus::Success,
            targets: ["one", "two", "three"]
                .into_iter()
                .enumerate()
                .map(|(i, text)| {
                    UiTarget::recognized_text(
                        Rect::new(10. + i as f64 * 200., 10., 100., 20.),
                        text.into(),
                    )
                })
                .collect(),
        },
        &env.ctx(),
    );
    settle(&mut mode, &env);
    let Command::ShowOverlay(first) = mode.render(&env.ctx()) else {
        panic!()
    };
    mode.handle(&ModeEvent::TextEdit(EditAction::Home), &env.ctx());
    let Command::ShowOverlay(moved) = mode.render(&env.ctx()) else {
        panic!()
    };
    assert!(Arc::ptr_eq(
        &first.retained.as_ref().unwrap().scene,
        &moved.retained.as_ref().unwrap().scene
    ));
    mode.insert("two", &env.ctx());
    let Command::ShowOverlay(filtered) = mode.render(&env.ctx()) else {
        panic!()
    };
    assert!(!Arc::ptr_eq(
        &first.retained.as_ref().unwrap().scene,
        &filtered.retained.as_ref().unwrap().scene
    ));
    let labels = &filtered.retained.as_ref().unwrap().scene.labels;
    assert_eq!(labels.len(), 1);
    assert!(labels[0].text.as_str().eq_ignore_ascii_case("s"));
    assert_eq!(filtered.retained.as_ref().unwrap().omitted_label, Some(0));
    let current = filtered
        .labels
        .iter()
        .find(|label| label.text.as_str() == "s")
        .unwrap();
    let mut expected = (*labels[0].style).clone();
    mode.config
        .hint
        .search_highlight
        .apply(&mut expected, &env.palette);
    assert_eq!(*current.style, expected);
    assert!(current.z_index > labels[0].z_index);
    mode.handle(&ModeEvent::TextEdit(EditAction::Home), &env.ctx());
    let Command::ShowOverlay(moved) = mode.render(&env.ctx()) else {
        panic!()
    };
    assert!(Arc::ptr_eq(
        &filtered.retained.as_ref().unwrap().scene,
        &moved.retained.as_ref().unwrap().scene
    ));
    assert_eq!(moved.retained.as_ref().unwrap().omitted_label, Some(0));
    mode.handle(&ModeEvent::TextEdit(EditAction::SelectAll), &env.ctx());
    mode.handle(&ModeEvent::TextEdit(EditAction::Backspace), &env.ctx());
    let Command::ShowOverlay(cleared) = mode.render(&env.ctx()) else {
        panic!()
    };
    assert_eq!(cleared.retained.as_ref().unwrap().omitted_label, None);
    assert_eq!(cleared.retained.as_ref().unwrap().scene.labels.len(), 3);
}

#[test]
fn pinyin_ranking_and_copy_preview_match_uihint_and_wait_for_copy_acknowledgement() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    mode.scan(
        UiScanResult {
            id: mode.scan_id,
            retired: Vec::new(),
            status: UiScanStatus::Success,
            targets: ["设置", "搜索"]
                .into_iter()
                .enumerate()
                .map(|(i, text)| {
                    UiTarget::recognized_text(
                        Rect::new(10. + i as f64 * 200., 10., 100., 20.),
                        text.into(),
                    )
                })
                .collect(),
        },
        &env.ctx(),
    );
    settle(&mut mode, &env);
    mode.insert("static_text", &env.ctx());
    assert!(
        mode.output.is_empty(),
        "text selection must not search synthetic role metadata"
    );
    mode.action(Action::Search, &env.ctx());
    mode.insert("sz", &env.ctx());
    assert_eq!(mode.output, "设置");
    let commands = mode.action(Action::Copy, &env.ctx());
    assert_eq!(
        commands,
        CommandBatch::one(Command::CopyText("设置".into()))
    );
    assert!(mode.active);
    assert!(
        mode.handle(&ModeEvent::TextCopied, &env.ctx())
            .iter()
            .any(|c| matches!(c, Command::SwitchMode(_)))
    );
}

#[test]
fn empty_scan_updates_preserve_results_and_search_warmup() {
    let env = Env::new();
    let mut mode = mode();
    let empty = |mode: &mut TextSelectMode, status| {
        mode.scan(
            UiScanResult {
                id: mode.scan_id,
                targets: Vec::new(),
                retired: Vec::new(),
                status,
            },
            &env.ctx(),
        )
    };
    mode.start(&env.ctx());
    assert!(
        empty(&mut mode, UiScanStatus::Success)
            .iter()
            .any(|c| matches!(c, Command::ShowOverlay(_)))
    );
    assert_eq!(mode.status, "No text found");
    assert!(mode.frame.units.is_empty());

    mode.start(&env.ctx());
    let bounds = Rect::new(10., 10., 300., 20.);
    mode.scan(
        UiScanResult {
            id: mode.scan_id,
            targets: vec![UiTarget::recognized_text(bounds, "first".into())],
            retired: Vec::new(),
            status: UiScanStatus::Partial,
        },
        &env.ctx(),
    );
    let text = mode.frame.text.clone();
    assert!(!mode.search.search_names_initialized);
    for (status, expected) in [
        (UiScanStatus::Success, "No text found"),
        (UiScanStatus::TimedOut, "Text recognition timed out"),
    ] {
        assert!(
            empty(&mut mode, status)
                .iter()
                .any(|c| matches!(c, Command::SetTimer { .. }))
        );
        assert_eq!(mode.status, expected);
        assert!(!mode.source.pending_relabel);
        mode.maintain(&env.ctx());
        assert!(mode.search.search_names_initialized);
        assert!(Arc::ptr_eq(&mode.frame.text, &text));
    }

    mode.scan(
        UiScanResult {
            id: mode.scan_id,
            targets: Vec::new(),
            retired: vec![bounds],
            status: UiScanStatus::Success,
        },
        &env.ctx(),
    );
    assert!(mode.source.pending_relabel);
    settle(&mut mode, &env);
    assert!(mode.frame.units.is_empty());
}

#[test]
fn late_batches_coalesce_and_deactivation_cancels_maintenance() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    deliver(&mut mode, &env, UiScanStatus::Partial, "first");
    let commands = mode.scan(
        UiScanResult {
            id: mode.scan_id,
            retired: vec![],
            status: UiScanStatus::Success,
            targets: vec![UiTarget::recognized_text(
                Rect::new(600., 100., 200., 20.),
                "second".into(),
            )],
        },
        &env.ctx(),
    );
    assert_eq!(mode.frame.units.len(), 1);
    assert!(
        commands
            .iter()
            .all(|c| matches!(c, Command::SetTimer { .. }))
    );
    mode.scan(
        UiScanResult {
            id: mode.scan_id,
            targets: Vec::new(),
            retired: Vec::new(),
            status: UiScanStatus::Success,
        },
        &env.ctx(),
    );
    assert!(mode.source.pending_relabel);
    settle(&mut mode, &env);
    assert_eq!(mode.frame.units.len(), 2);
    mode.action(Action::SelectAll, &env.ctx());
    assert!(!mode.output.is_empty());
    let snapshot = mode.frame.clone();
    let commands = mode.scan(
        UiScanResult {
            id: mode.scan_id,
            retired: vec![],
            status: UiScanStatus::Success,
            targets: vec![UiTarget::recognized_text(
                Rect::new(10., 35., 300., 50.),
                "late row".into(),
            )],
        },
        &env.ctx(),
    );
    assert!(commands.is_empty());
    settle(&mut mode, &env);
    assert_eq!(mode.frame.text, snapshot.text);
    assert_eq!(mode.frame.selected, snapshot.selected);
    assert_eq!(mode.frame.region_bounds, snapshot.region_bounds);
    mode.navigate(true, &env.ctx());
    settle(&mut mode, &env);
    assert_eq!(&*mode.frame.text, "first late row\n\nsecond");
    assert_eq!(mode.frame.selected, [false, false]);
    mode.action(Action::SelectAll, &env.ctx());
    let commands = mode.handle(&ModeEvent::Deactivated, &env.ctx());
    assert_eq!(mode.output.capacity(), 0);
    assert_eq!(mode.summary.capacity(), 0);
    assert_eq!(mode.base_selected.capacity(), 0);
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, Command::CancelTimer { id } if id == MAINTENANCE))
    );
    assert!(
        mode.handle(
            &ModeEvent::Timer {
                id: MAINTENANCE.into(),
                elapsed: Duration::ZERO
            },
            &env.ctx()
        )
        .is_empty()
    );
}

#[test]
fn held_overlap_rebuilds_for_streamed_results_and_clears_on_restart() {
    let env = Env::new();
    let mut mode = mode();
    mode.start(&env.ctx());
    let send = |mode: &mut TextSelectMode, key: &str, state| {
        mode.handle(
            &ModeEvent::Key {
                key: Key::new(key).unwrap(),
                state,
                repeat: false,
            },
            &env.ctx(),
        )
    };
    send(&mut mode, "left_shift", KeyState::Down);
    assert_eq!(mode.held_overlap_keys.len(), 1);
    mode.scan(
        UiScanResult {
            id: mode.scan_id,
            retired: vec![],
            status: UiScanStatus::Partial,
            targets: (0..3)
                .map(|i| {
                    UiTarget::recognized_text(
                        Rect::new(10. + i as f64, 10., 80., 20.),
                        format!("word {i}"),
                    )
                })
                .collect(),
        },
        &env.ctx(),
    );
    settle(&mut mode, &env);
    assert_eq!(mode.layers.layer_count(), 3);
    assert_eq!(mode.active_overlap_layer(), Some(1));
    send(&mut mode, "left_shift", KeyState::Up);
    assert_eq!(mode.active_overlap_layer(), Some(0));
    send(&mut mode, "right_shift", KeyState::Down);
    assert_eq!(mode.active_overlap_layer(), Some(2));
    send(&mut mode, "right_shift", KeyState::Up);
    send(&mut mode, "left_shift", KeyState::Down);
    assert_eq!(mode.active_overlap_layer(), Some(1));
    mode.start(&env.ctx());
    assert!(mode.held_overlap_keys.is_empty());
    assert!(!mode.layers.is_ready());
}

#[test]
fn wider_panel_separates_the_summary_and_keeps_card_text_compact() {
    use crate::api::OverlayShape;
    let mut env = Env::new();
    for scale in [1., 1.5] {
        env.screens[0].scale = scale;
        let config =
            crate::config::Config::parse("[ui_hint.search_highlight]\ntext_color = '#FF1122FF'")
                .unwrap();
        let mut mode = crate::app::mode_catalog::text_select(&config);
        words(
            &mut mode,
            &env,
            "KeySteer Omitted fields",
            vec![0..8, 9..16, 17..23],
        );
        let Command::ShowOverlay(scene) = mode.render(&env.ctx()) else {
            panic!()
        };
        let scaled = crate::presentation::label_scale(scale);
        let panel = scene
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                OverlayShape::Rect {
                    rect,
                    z_index: 10_000,
                    ..
                } => Some(*rect),
                _ => None,
            })
            .max_by(|a, b| a.width.total_cmp(&b.width))
            .unwrap();
        assert_eq!(
            panel.width,
            (800. * scaled).min(env.screens[0].bounds.width)
        );
        let summary = scene
            .labels
            .iter()
            .find(|label| label.text.contains("3 words"))
            .unwrap();
        let code = scene
            .labels
            .iter()
            .find(|label| label.text.as_str() == "✓ 1")
            .unwrap();
        let word = scene
            .labels
            .iter()
            .find(|label| label.text.as_str() == "KeySteer")
            .unwrap();
        let first_card = scene
            .shapes
            .iter()
            .find_map(|shape| match shape {
                OverlayShape::Rect {
                    rect,
                    z_index: 10_001,
                    ..
                } => Some(*rect),
                _ => None,
            })
            .unwrap();
        assert!(first_card.y - summary.rect.bottom() >= 12. * scaled);
        assert!(scene.shapes.iter().any(|shape| matches!(shape,
            OverlayShape::Line {from, to, ..} if from.y > summary.rect.bottom() && to.y < first_card.y)));
        assert_eq!(word.rect.y, code.rect.bottom());
        assert!(word.rect.bottom() <= first_card.bottom());
        assert!(first_card.height < 3.0 * word.style.font_size * scaled);
        assert!(scene.shapes.iter().all(|shape| match shape {
            OverlayShape::Rect {
                rect,
                z_index: 10_001,
                ..
            } =>
                rect.x >= panel.x
                    && rect.right() <= panel.right()
                    && rect.bottom() <= panel.bottom(),
            _ => true,
        }));
        mode.frame.toggle(1);
        mode.update_summary();
        let Command::ShowOverlay(mixed) = mode.render(&env.ctx()) else {
            panic!()
        };
        let plain = mode
            .config
            .hint
            .search_info_ui
            .input_style(env.palette.appearance);
        for text in ["2", "Omitted"] {
            let label = mixed
                .labels
                .iter()
                .find(|label| label.text.as_str() == text)
                .unwrap();
            assert_eq!(label.style.text_color, plain.text_color);
            assert_eq!(label.style.background, Color::TRANSPARENT);
        }
        let checked = mixed
            .labels
            .iter()
            .find(|label| label.text.as_str() == "KeySteer")
            .unwrap();
        assert_eq!(checked.style.text_color, Color::rgb(255, 17, 34));
        assert_eq!(checked.style, word.style);
        mode.frame.toggle(1);
        mode.update_summary();
        let Command::ShowOverlay(restored) = mode.render(&env.ctx()) else {
            panic!()
        };
        let restored = restored
            .labels
            .iter()
            .find(|label| label.text.as_str() == "Omitted")
            .unwrap();
        assert_eq!(restored.style, word.style);
    }
    let mut config = crate::config::Config::default();
    config.text_select.ui.panel_width = 960;
    let mode = crate::app::mode_catalog::text_select(&config);
    assert_eq!(mode.config.hint.search_info_ui.width, 960.);
    assert_eq!(
        crate::app::mode_catalog::hint_settings(&config)
            .search_info_ui
            .width,
        520.
    );
}

#[test]
fn preview_reflows_ocr_rows_to_panel_width_and_copies_the_same_clean_text() {
    let mut env = Env::new();
    let row = "alpha  beta gamma delta epsilon";
    let expected = ["alpha beta gamma delta epsilon"; 8].join(" ");
    for scale in [1., 1.5] {
        env.screens[0].scale = scale;
        let mut previews = Vec::new();
        for width in [400, 1000] {
            let mut config = crate::config::Config::default();
            config.text_select.ui.panel_width = width;
            let mut mode = crate::app::mode_catalog::text_select(&config);
            mode.start(&env.ctx());
            mode.scan(
                UiScanResult {
                    id: mode.scan_id,
                    retired: Vec::new(),
                    status: UiScanStatus::Success,
                    targets: (0..8)
                        .map(|index| {
                            UiTarget::recognized_text(
                                Rect::new(10., 10. + index as f64 * 25., 300., 20.),
                                row.into(),
                            )
                        })
                        .collect(),
                },
                &env.ctx(),
            );
            settle(&mut mode, &env);
            mode.insert("a", &env.ctx());
            assert_eq!(mode.output, expected);
            let Command::ShowOverlay(scene) = mode.render(&env.ctx()) else {
                panic!()
            };
            let labels: Vec<_> = scene
                .labels
                .iter()
                .filter(|label| {
                    label.z_index == 10_002
                        && label.fixed_bounds
                        && label.edit.is_none()
                        && label.style.wrap
                })
                .cloned()
                .collect();
            assert_eq!(labels.len(), 1);
            assert_eq!(labels[0].text.as_str(), expected);
            assert!(!labels[0].text.contains('\n'));
            assert!(labels[0].rect.height <= labels[0].style.font_size * scale * 1.4 * 4.0 + 0.01);
            assert_eq!(
                mode.action(Action::Copy, &env.ctx()),
                CommandBatch::one(Command::CopyText(expected.clone()))
            );
            previews.push(labels);
        }
        let narrow = &previews[0];
        let wide = &previews[1];
        assert!(
            wide[0].rect.width > narrow[0].rect.width,
            "scale={scale}: narrow={narrow:?}, wide={wide:?}"
        );
        assert_eq!(wide[0].text, narrow[0].text);
    }
}

#[test]
fn paging_caption_follows_the_configured_cycle_key() {
    let env = Env::new();
    let mut config = crate::config::Config::default();
    config.text_select.bindings.remove("tab");
    config
        .text_select
        .bindings
        .insert("ctrl+n".into(), Binding::TextSelect(Action::CyclePage));
    let mut mode = crate::app::mode_catalog::text_select(&config);
    let text = "字".repeat(25);
    words(
        &mut mode,
        &env,
        &text,
        (0..25).map(|i| i * 3..i * 3 + 3).collect(),
    );
    let Command::ShowOverlay(scene) = mode.render(&env.ctx()) else {
        panic!()
    };
    assert!(
        scene
            .labels
            .iter()
            .any(|label| label.text.as_str() == "Page 1 / 2  ·  CTRL+N: next page")
    );
    mode.action(Action::CyclePage, &env.ctx());
    assert_eq!(mode.page, 1);
    mode.action(Action::CyclePage, &env.ctx());
    assert_eq!(mode.page, 0);
}
