// Included in the session tests to share the existing portable native adapter.
fn acknowledge(access: &mut Fake) {
    for (id, maximize) in std::mem::take(&mut access.submitted_states) {
        let window = access.windows.get_mut(&id).unwrap();
        if maximize { window.restored = window.info.bounds; }
        window.info.maximized = maximize;
        window.info.minimized = false;
        window.info.bounds = if maximize { screens()[window.info.screen].work_area } else { window.restored };
    }
    for (id, bounds) in std::mem::take(&mut access.submitted) {
        let window = access.windows.get_mut(&id).unwrap();
        window.info.bounds = bounds;
        if !window.info.maximized { window.restored = bounds; }
    }
}

fn layout_request(count: u64) -> WindowRequest {
    WindowRequest { session: 1, id: 2, scope: None,
        operation: WindowOperation::ApplyLayout { transaction: 7, revision: 1, screen: 0,
            placements: (1..=count).map(|id| (WindowId(id), Rect::new(0.1, 0.1, 0.6, 0.6))).collect(),
            additional_screens: Vec::new(), gap: 0.0, strict: true } }
}

fn begin_async_edit(session: &mut Session, access: &mut Fake) {
    access.deferred = true;
    access.async_states = true;
    let result = run(session, access, WindowOperation::BeginEdit { transaction: 7, targets: access.windows.keys().copied().collect(), screen: None, group: 10 });
    assert!(result.message.is_none(), "{:?}", result.message);
}

fn finish_layout(pending: &mut layout_confirmation::PendingLayout, session: &mut Session, access: &mut Fake, cancelled: bool) -> WindowResult {
    let start = Instant::now();
    for step in 0..30 {
        acknowledge(access);
        if let Some(result) = pending.advance_at(session, access, cancelled, start + Duration::from_millis(step * 20)) { return result; }
    }
    panic!("layout failed to finish");
}

#[test]
fn async_layout_bounds_inflight_and_commits_history_only_at_end_edit() {
    let mut access = Fake::new(20);
    let original = access.windows.clone();
    let mut session = Session::default();
    begin_async_edit(&mut session, &mut access);
    let screens = screens().into();
    let mut pending = layout_confirmation::PendingLayout::begin(&mut session, &mut access, &layout_request(20), &screens).unwrap().unwrap();
    assert!(access.submitted.is_empty());
    assert!(pending.advance(&mut session, &mut access, false).is_none());
    assert_eq!(access.submitted.len(), 16);
    let result = finish_layout(&mut pending, &mut session, &mut access, false);
    assert!(result.message.is_none(), "{:?}", result.message);
    assert_eq!(result.changed, 20);
    assert!(session.history.is_empty());
    run(&mut session, &mut access, WindowOperation::EndEdit { transaction: 7, commit: true });
    assert_eq!(session.history.len(), 1);
    run(&mut session, &mut access, WindowOperation::Undo);
    for (id, snapshot) in original { assert!(same_placement(&snapshot, &access.windows[&id])); }
}

#[test]
fn async_layout_failed_write_rolls_back_all_attempts_including_partial_write() {
    let mut access = Fake::new(3);
    let original = access.windows.clone();
    let mut session = Session::default();
    begin_async_edit(&mut session, &mut access);
    access.reject = Some(WindowId(3));
    let mut pending = layout_confirmation::PendingLayout::begin(&mut session, &mut access, &layout_request(3), &screens().into()).unwrap().unwrap();
    assert!(pending.advance(&mut session, &mut access, false).is_none());
    assert_eq!(access.submitted.len(), 3);
    access.reject = None;
    let result = finish_layout(&mut pending, &mut session, &mut access, false);
    assert!(result.message.is_some());
    assert!(matches!(*result.edit.unwrap(), WindowEditResult::Applied { accepted: false, .. }));
    for (id, snapshot) in original { assert!(same_placement(&snapshot, &access.windows[&id])); }
    assert!(session.history.is_empty());
}

#[test]
fn async_layout_cancel_waits_for_outstanding_writes_then_restores_maximized_entry() {
    let mut access = Fake::new(2);
    let window = access.windows.get_mut(&WindowId(1)).unwrap();
    window.info.maximized = true;
    window.info.bounds = screens()[0].work_area;
    let original = access.windows.clone();
    let mut session = Session::default();
    begin_async_edit(&mut session, &mut access);
    let mut pending = layout_confirmation::PendingLayout::begin(&mut session, &mut access, &layout_request(2), &screens().into()).unwrap().unwrap();
    assert!(pending.advance(&mut session, &mut access, false).is_none());
    assert!(pending.advance(&mut session, &mut access, true).is_none());
    assert!(!access.submitted_states[&WindowId(1)]);
    let result = finish_layout(&mut pending, &mut session, &mut access, true);
    assert!(result.message.is_some());
    for (id, snapshot) in original { assert!(same_placement(&snapshot, &access.windows[&id]), "window {id:?}"); }
}

#[test]
fn async_end_edit_restores_entry_and_closes_transaction() {
    let mut access = Fake::new(2);
    let original = access.windows.clone();
    let mut session = Session::default();
    begin_async_edit(&mut session, &mut access);
    for window in access.windows.values_mut() { window.info.bounds.x += 100.0; }
    let request = WindowRequest { operation: WindowOperation::EndEdit { transaction: 7, commit: false }, ..layout_request(2) };
    let mut pending = layout_confirmation::PendingLayout::begin(&mut session, &mut access, &request, &screens().into()).unwrap().unwrap();
    let result = finish_layout(&mut pending, &mut session, &mut access, false);
    assert!(session.edit.is_none());
    assert!(matches!(*result.edit.unwrap(), WindowEditResult::Ended { committed: false, .. }));
    for (id, snapshot) in original { assert!(same_placement(&snapshot, &access.windows[&id])); }
}

#[test]
fn async_maximize_and_restore_require_acknowledged_state_and_keep_restore_bounds() {
    let mut access = Fake::new(1);
    access.deferred = true;
    access.async_states = true;
    let original = access.windows[&WindowId(1)].clone();
    let mut session = Session::default();
    let request = WindowRequest { session: 1, id: 1, scope: None, operation: adjust(1, WindowChange::ToggleMaximize) };
    let screens = screens().into();
    for maximize in [true, false] {
        let mut pending = PendingAdjustment::begin(&mut session, &mut access, &request, &screens).unwrap().0.unwrap();
        assert!(pending.poll(&mut session, &mut access, false).is_none());
        let mut result = None;
        for _ in 0..5 { acknowledge(&mut access); result = pending.poll(&mut session, &mut access, false); if result.is_some() { break; } }
        let result = result.unwrap();
        assert!(result.message.is_none(), "{:?}", result.message);
        assert_eq!(result.target.unwrap().maximized, maximize);
    }
    assert!(same_placement(&original, &access.windows[&WindowId(1)]));
}

#[test]
fn independent_confirmations_observe_fast_window_without_committing_ahead_of_slow_window() {
    let mut access = Fake::new(2);
    access.deferred = true;
    let mut session = Session::default();
    let screens = screens().into();
    let mut frames: Vec<_> = (1..=2).map(|id| PendingAdjustment::begin(&mut session, &mut access,
        &WindowRequest { session: 1, id, scope: None, operation: WindowOperation::Adjust { target: WindowId(id), change: WindowChange::Move { dx: 20.0, dy: 0.0 }, group: id } }, &screens).unwrap().0.unwrap()).collect();
    let bounds = access.submitted[&WindowId(2)];
    access.windows.get_mut(&WindowId(2)).unwrap().info.bounds = bounds;
    for frame in &mut frames { frame.advance(&mut access, false); }
    assert!(!frames[0].ready());
    assert!(frames[1].ready());
    assert!(session.history.is_empty());
    // Completed frames must not read a subsequently unavailable native window again.
    access.windows.remove(&WindowId(2));
    frames[1].advance(&mut access, false);
    assert!(frames[1].failure.is_none());
}

#[test]
fn strict_async_layout_learns_constraints_and_rolls_back_before_rejection() {
    let mut access = Fake::new(1);
    let original = access.windows[&WindowId(1)].clone();
    let mut session = Session::default();
    begin_async_edit(&mut session, &mut access);
    let mut pending = layout_confirmation::PendingLayout::begin(&mut session, &mut access, &layout_request(1), &screens().into()).unwrap().unwrap();
    let start = Instant::now();
    let mut completed = None;
    for step in 0..30 {
        for rect in access.submitted.values_mut() {
            if rect.width > 600.0 { rect.width += 50.0; }
        }
        acknowledge(&mut access);
        completed = pending.advance_at(&mut session, &mut access, false, start + Duration::from_millis(step * 20));
        if completed.is_some() { break; }
    }
    let result = completed.unwrap();
    assert!(matches!(*result.edit.unwrap(), WindowEditResult::Applied { accepted: false, .. }));
    assert!(session.minimums[&WindowId(1)].x >= 770.0);
    assert!(same_placement(&original, &access.windows[&WindowId(1)]));
}

#[test]
fn failed_async_rollback_ends_edit_after_entry_recovery_attempt() {
    let mut access = Fake::new(1);
    let original = access.windows[&WindowId(1)].clone();
    let mut session = Session::default();
    begin_async_edit(&mut session, &mut access);
    access.reject = Some(WindowId(1));
    let mut pending = layout_confirmation::PendingLayout::begin(&mut session, &mut access, &layout_request(1), &screens().into()).unwrap().unwrap();
    let result = finish_layout(&mut pending, &mut session, &mut access, false);
    assert!(session.edit.is_none());
    assert!(matches!(*result.edit.unwrap(), WindowEditResult::Ended { committed: false, .. }));
    assert!(result.message.unwrap().contains("entry-layout recovery"));
    assert!(same_placement(&original, &access.windows[&WindowId(1)]));
}
