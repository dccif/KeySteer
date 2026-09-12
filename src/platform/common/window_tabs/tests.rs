use super::*;
use crate::api::window_tabs::TabGroupId;
use std::cell::Cell;

struct Fake {
    windows: BTreeMap<WindowId, Snapshot>,
    events: Vec<TabNativeEvent>,
    closed: Vec<WindowId>,
    close_requests: std::cell::RefCell<Vec<WindowId>>,
    focus: Cell<Option<WindowId>>,
    deny_focus: bool,
    fail_visibility: Option<(WindowId, bool)>,
    fail: Option<WindowId>,
    bars: Vec<TabBar>,
    updates: Vec<TabGroupId>,
    fail_bar: bool,
    writes: usize,
    snapshot_reads: Cell<usize>,
    hidden: BTreeSet<WindowId>,
    hidden_foreground: usize,
    header: f64,
}
fn screens() -> Vec<Screen> {
    vec![Screen {
        name: Some("Test".into()),
        bounds: Rect::new(0.0, 0.0, 1200.0, 800.0),
        work_area: Rect::new(0.0, 0.0, 1200.0, 800.0),
        scale: 1.0,
        is_primary: true,
    }]
}

#[test]
fn scope_filters_inventory_and_reentry_reclaims_numbers_without_dissolving_groups() {
    use crate::api::window::WindowScope;
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    let membership = access.groups.state.groups.clone();
    access
        .native
        .windows
        .get_mut(&WindowId(3))
        .unwrap()
        .info
        .minimized = true;
    access
        .native
        .windows
        .get_mut(&WindowId(4))
        .unwrap()
        .info
        .screen = 1;
    let current = Some(WindowScope {
        screen: Some(0),
        include_minimized: false,
    });
    access.set_scope(current, true);
    let windows = access.enumerate(&screens(), &|| false).unwrap();
    assert_eq!(
        windows.iter().map(|w| w.id).collect::<Vec<_>>(),
        [WindowId(1), WindowId(2)]
    );
    assert_eq!(
        access
            .groups
            .state
            .numbers
            .iter()
            .map(|(_, n)| *n)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(access.groups.state.groups, membership);
    access
        .native
        .windows
        .get_mut(&WindowId(2))
        .unwrap()
        .info
        .minimized = true;
    access.set_scope(current, true);
    assert!(access.enumerate(&screens(), &|| false).unwrap().is_empty());
    assert!(access.groups.state.numbers.is_empty());
    access.set_scope(
        Some(WindowScope {
            screen: None,
            include_minimized: true,
        }),
        false,
    );
    assert_eq!(access.enumerate(&screens(), &|| false).unwrap().len(), 4);
    assert_eq!(
        access
            .groups
            .state
            .numbers
            .iter()
            .map(|(_, n)| *n)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    access.native.windows.remove(&WindowId(3));
    access.set_scope(
        Some(WindowScope {
            screen: None,
            include_minimized: true,
        }),
        true,
    );
    access.enumerate(&screens(), &|| false).unwrap();
    assert_eq!(
        access
            .groups
            .state
            .numbers
            .iter()
            .map(|(_, n)| *n)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

#[test]
fn window_tab_cycle_stays_in_current_group_and_digits_can_leave() {
    use crate::api::window::WindowOperation as O;
    use crate::platform::common::window_session::WindowSessionProbe;
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 3);
    let mut session = WindowSessionProbe::new(WindowId(1));
    for (operation, expected) in [
        (O::Cycle, 1),
        (O::Cycle, 3),
        (O::Cycle, 1),
        (O::CyclePrevious, 3),
        (O::Select(WindowId(2)), 2),
    ] {
        let result = session.execute(&mut access, operation, &screens());
        assert_eq!(result.target.unwrap().id, WindowId(expected));
    }
    assert_eq!(
        session
            .execute(&mut access, O::Cycle, &screens())
            .target
            .unwrap()
            .id,
        WindowId(3)
    );
}

#[test]
fn first_numbers_batch_applications_without_renumbering_existing_windows() {
    let mut access = setup();
    access.groups = Groups::default();
    for (id, app) in [(1, "Explorer"), (2, "Zed"), (3, "explorer"), (4, "Browser")] {
        access
            .native
            .windows
            .get_mut(&WindowId(id))
            .unwrap()
            .info
            .app = app.into();
    }
    access.acquire(Point::default(), &screens()).unwrap();
    access.enumerate(&screens(), &|| false).unwrap();
    let numbers: BTreeMap<_, _> = access.groups.state.numbers.iter().copied().collect();
    assert_eq!(numbers[&WindowId(1)], 1);
    assert_eq!(numbers[&WindowId(3)], 2);
    let original = access.groups.state.numbers.clone();
    access.enumerate(&screens(), &|| false).unwrap();
    assert_eq!(access.groups.state.numbers, original);
}

#[test]
fn native_minimize_synchronizes_every_member_and_stale_focus_cannot_reopen_group() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    let hidden_geometry = access.native.windows[&WindowId(1)].info.bounds;
    access
        .native
        .windows
        .get_mut(&WindowId(2))
        .unwrap()
        .info
        .minimized = true;
    access.native.focus.set(Some(WindowId(1)));
    access.native.events.extend([
        TabNativeEvent::Changed(WindowId(2)),
        TabNativeEvent::Focused(WindowId(1)),
    ]);
    access.reset();
    access.pump(&screens(), &|| false).unwrap();
    assert!(access.native.windows[&WindowId(1)].info.minimized);
    assert!(access.native.windows[&WindowId(2)].info.minimized);
    assert!(!access.native.windows[&WindowId(3)].info.minimized);
    assert_eq!(
        access.native.windows[&WindowId(1)].info.bounds,
        hidden_geometry
    );
    assert_eq!(access.groups.state.groups[0].active, WindowId(2));
    assert!(!access.bars[0].visible);
    access
        .activate_window(WindowId(1), &screens(), &|| false)
        .unwrap();
    assert!(
        !access
            .snapshot(WindowId(1), &screens())
            .unwrap()
            .info
            .minimized
    );
    assert!(access.bars[0].visible);
    assert!(access.native.hidden.contains(&WindowId(2)));
}

#[test]
fn tab_headers_are_included_in_layout_inventory_minimums_and_restore() {
    for scale in [1.0, 1.5, 2.0] {
        let mut displays = screens();
        displays[0].scale = scale;
        let header = 30.0 * scale;
        let mut access = setup();
        access.native.header = 30.0;
        access
            .native
            .windows
            .get_mut(&WindowId(1))
            .unwrap()
            .info
            .bounds = displays[0].work_area;
        for id in [1, 2] {
            access
                .tab_operation(
                    TabOperation::Choose(WindowTarget::Window(WindowId(id))),
                    &displays,
                    &|| false,
                )
                .unwrap();
        }
        assert_eq!(
            access.snapshot(WindowId(2), &displays).unwrap().info.bounds,
            displays[0].work_area
        );
        assert_eq!(access.native.windows[&WindowId(2)].info.bounds.y, header);
        assert_eq!(access.minimum_size(WindowId(2)).y, 80.0 + header);
        let inactive = access.native.windows[&WindowId(1)].info.bounds;
        let outer = Rect::new(600.0, 0.0, 600.0, 400.0);
        access
            .set_frame(WindowId(1), outer, &displays, &|| false)
            .unwrap();
        assert_eq!(
            access.snapshot(WindowId(1), &displays).unwrap().info.bounds,
            outer
        );
        assert_eq!(
            access.native.windows[&WindowId(2)].info.bounds,
            Rect::new(600.0, header, 600.0, 400.0 - header)
        );
        assert_eq!(access.native.windows[&WindowId(1)].info.bounds, inactive);
        let saved = access.snapshot(WindowId(1), &displays).unwrap();
        access
            .set_frame(
                WindowId(1),
                Rect::new(0.0, 0.0, 600.0, 500.0),
                &displays,
                &|| false,
            )
            .unwrap();
        access.restore(&saved, &displays, &|| false).unwrap();
        assert_eq!(
            access.snapshot(WindowId(1), &displays).unwrap().info.bounds,
            outer
        );
        access
            .activate_tab(WindowId(1), &displays, &|| false)
            .unwrap();
        assert_eq!(
            access.snapshot(WindowId(1), &displays).unwrap().info.bounds,
            outer
        );
        access.reset();
        access
            .native
            .windows
            .get_mut(&WindowId(1))
            .unwrap()
            .info
            .bounds = displays[0].work_area;
        access
            .native
            .events
            .push(TabNativeEvent::Changed(WindowId(1)));
        access.pump(&displays, &|| false).unwrap();
        assert_eq!(
            access.snapshot(WindowId(1), &displays).unwrap().info.bounds,
            displays[0].work_area
        );
        assert_eq!(access.bars[0].bounds.y, header);
    }
}
fn setup() -> Grouped<Fake> {
    let windows = (1..=4)
        .map(|id| {
            let bounds = Rect::new(id as f64 * 20.0, 100.0, 400.0, 300.0);
            (
                WindowId(id),
                Snapshot {
                    info: WindowInfo {
                        id: WindowId(id),
                        title: format!("Window {id}"),
                        app: format!("app{id}"),
                        screen: 0,
                        bounds,
                        resizable: true,
                        maximized: false,
                        minimized: false,
                        fullscreen: false,
                    },
                    restored: bounds,
                },
            )
        })
        .collect();
    let mut grouped = Grouped::new(Fake {
        windows,
        events: Vec::new(),
        closed: Vec::new(),
        close_requests: Default::default(),
        focus: Cell::new(None),
        deny_focus: false,
        fail_visibility: None,
        fail: None,
        bars: Vec::new(),
        updates: Vec::new(),
        fail_bar: false,
        writes: 0,
        snapshot_reads: Cell::new(0),
        hidden: BTreeSet::new(),
        hidden_foreground: 0,
        header: 0.0,
    });
    grouped.enumerate(&screens(), &|| false).unwrap();
    grouped
}
impl WindowAccess for Fake {
    fn tab_bar_height(&self, screen: &Screen) -> f64 {
        self.header * screen.scale
    }
    fn tab_selected(&self, id: WindowId) -> bool {
        self.focus.get() == Some(id)
    }
    fn tab_set_hidden(&mut self, id: WindowId, hidden: bool) -> Result<(), String> {
        if self.fail_visibility == Some((id, hidden)) {
            self.fail_visibility = None;
            return Err("injected visibility failure".into());
        }
        if hidden {
            if self.focus.get() == Some(id) && !self.hidden.contains(&id) {
                self.hidden_foreground += 1;
            }
            self.hidden.insert(id);
        } else {
            self.hidden.remove(&id);
        }
        Ok(())
    }
    fn acquire(&mut self, _: Point, _: &[Screen]) -> Result<Option<WindowInfo>, String> {
        Ok(Some(self.windows[&WindowId(1)].info.clone()))
    }
    fn enumerate(&mut self, _: &[Screen], _: &dyn Fn() -> bool) -> Result<Vec<WindowInfo>, String> {
        Ok(self.windows.values().map(|s| s.info.clone()).collect())
    }
    fn snapshot(&self, id: WindowId, screens: &[Screen]) -> Result<Snapshot, String> {
        self.snapshot_reads.set(self.snapshot_reads.get() + 1);
        if screens.is_empty() {
            return Err("no displays".into());
        }
        self.windows.get(&id).cloned().ok_or("closed".into())
    }
    fn set_frame(
        &mut self,
        id: WindowId,
        rect: Rect,
        screens: &[Screen],
        _: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        if self.fail == Some(id) {
            self.fail = None;
            return Err("injected placement failure".into());
        }
        self.writes += 1;
        let mut snapshot = self.snapshot(id, screens)?;
        snapshot.info.bounds = rect;
        snapshot.info.minimized = false;
        snapshot.info.maximized = false;
        snapshot.restored = rect;
        self.windows.insert(id, snapshot.clone());
        Ok(snapshot.info)
    }
    fn restore(
        &mut self,
        snapshot: &Snapshot,
        _: &[Screen],
        _: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        if !self.windows.contains_key(&snapshot.info.id) {
            return Err("closed".into());
        }
        self.writes += 1;
        self.windows.insert(snapshot.info.id, snapshot.clone());
        Ok(snapshot.info.clone())
    }
    fn cycle_state(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        let mut before = self.snapshot(id, screens)?;
        before.info.minimized = !before.info.minimized;
        self.restore(&before, screens, cancelled)
    }
    fn select(&self, id: WindowId) -> Result<(), String> {
        if self.deny_focus {
            return Err("system denied focus".into());
        }
        self.focus.set(Some(id));
        Ok(())
    }
    fn close(&self, id: WindowId) -> Result<(), String> {
        self.close_requests.borrow_mut().push(id);
        Ok(())
    }
    fn pointer(&self) -> Result<Point, String> {
        Ok(Point::default())
    }
    fn reset(&mut self) {
        self.windows.clear();
    }
    fn take_closed(&mut self) -> Vec<WindowId> {
        std::mem::take(&mut self.closed)
    }
    fn tab_events(&mut self) -> Vec<TabNativeEvent> {
        std::mem::take(&mut self.events)
    }
    fn tab_bars(&mut self, bars: &[TabBar]) -> Result<(), String> {
        self.bars = bars.to_vec();
        Ok(())
    }
    fn tab_bar_update(&mut self, bar: &TabBar) -> Result<bool, String> {
        if std::mem::take(&mut self.fail_bar) {
            return Err("injected bar failure".into());
        }
        self.updates.push(bar.group);
        let current = self.bars.iter_mut().find(|b| b.group == bar.group).unwrap();
        *current = bar.clone();
        Ok(true)
    }
    fn tab_visible(&self, id: WindowId) -> bool {
        self.windows.contains_key(&id) && !self.hidden.contains(&id)
    }
}

#[test]
fn switching_transfers_focus_before_hiding_the_previous_member() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    let before = access.native.hidden_foreground;
    for id in [1, 2, 1, 2] {
        op(&mut access, TabOperation::Activate(WindowId(id)));
        assert_eq!(access.native.focus.get(), Some(WindowId(id)));
        assert_eq!(access.native.hidden_foreground, before);
        assert_eq!(access.native.hidden.len(), 1);
    }
    access
        .activate_window(WindowId(1), &screens(), &|| false)
        .unwrap();
    assert_eq!(access.native.hidden_foreground, before);
}

#[test]
fn inactive_group_keeps_its_bar_and_reentry_does_not_duplicate_it() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    access.native.focus.set(Some(WindowId(4)));
    access.native.events.push(TabNativeEvent::VisibilityChanged);
    access.pump(&screens(), &|| false).unwrap();
    assert!(access.native.bars[0].visible);
    for _ in 0..3 {
        op(&mut access, TabOperation::Enter { screen: 0 });
        assert_eq!(access.groups.state.groups.len(), 1);
        assert_eq!(access.groups.state.groups[0].id, TabGroupId(1));
    }
    access
        .native
        .windows
        .get_mut(&WindowId(2))
        .unwrap()
        .info
        .minimized = true;
    access
        .native
        .events
        .push(TabNativeEvent::Changed(WindowId(2)));
    access.pump(&screens(), &|| false).unwrap();
    assert!(!access.native.bars[0].visible);
}

#[test]
fn worker_exit_keeps_bar_tracking_without_moving_hidden_members() {
    use crate::api::{
        BackendEvent,
        window::{WindowOperation, WindowRequest},
    };
    use crate::platform::common::window_session::WindowWorker;
    use std::sync::{Arc, Mutex, mpsc};

    struct Shared(Arc<Mutex<Fake>>);
    impl WindowAccess for Shared {
        fn acquire(&mut self, p: Point, s: &[Screen]) -> Result<Option<WindowInfo>, String> {
            self.0.lock().unwrap().acquire(p, s)
        }
        fn enumerate(
            &mut self,
            s: &[Screen],
            c: &dyn Fn() -> bool,
        ) -> Result<Vec<WindowInfo>, String> {
            self.0.lock().unwrap().enumerate(s, c)
        }
        fn snapshot(&self, id: WindowId, s: &[Screen]) -> Result<Snapshot, String> {
            self.0.lock().unwrap().snapshot(id, s)
        }
        fn set_frame(
            &mut self,
            id: WindowId,
            r: Rect,
            s: &[Screen],
            c: &dyn Fn() -> bool,
        ) -> Result<WindowInfo, String> {
            self.0.lock().unwrap().set_frame(id, r, s, c)
        }
        fn restore(
            &mut self,
            b: &Snapshot,
            s: &[Screen],
            c: &dyn Fn() -> bool,
        ) -> Result<WindowInfo, String> {
            self.0.lock().unwrap().restore(b, s, c)
        }
        fn cycle_state(
            &mut self,
            id: WindowId,
            s: &[Screen],
            c: &dyn Fn() -> bool,
        ) -> Result<WindowInfo, String> {
            self.0.lock().unwrap().cycle_state(id, s, c)
        }
        fn select(&self, id: WindowId) -> Result<(), String> {
            self.0.lock().unwrap().select(id)
        }
        fn pointer(&self) -> Result<Point, String> {
            Ok(Point::default())
        }
        fn reset(&mut self) {
            self.0.lock().unwrap().reset();
        }
        fn tab_events(&mut self) -> Vec<TabNativeEvent> {
            self.0.lock().unwrap().tab_events()
        }
        fn tab_bars(&mut self, bars: &[TabBar]) -> Result<(), String> {
            self.0.lock().unwrap().tab_bars(bars)
        }
    }
    let fake = Arc::new(Mutex::new(setup().native));
    let owner = fake.clone();
    let (tx, rx) = mpsc::channel();
    let mut worker = WindowWorker::start(
        move || Shared(owner),
        move |e| {
            let _ = tx.send(e);
        },
    )
    .unwrap();
    for (index, operation) in [
        WindowOperation::Acquire(Point::default()),
        WindowOperation::Tabs(TabOperation::Choose(WindowTarget::Window(WindowId(1)))),
        WindowOperation::Tabs(TabOperation::Choose(WindowTarget::Window(WindowId(2)))),
    ]
    .into_iter()
    .enumerate()
    {
        worker
            .submit(
                WindowRequest {
                    scope: None,
                    session: 7,
                    id: index as u64 + 1,
                    operation,
                },
                &screens(),
            )
            .unwrap();
        let BackendEvent::WindowResult(result) = rx.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("unexpected event")
        };
        assert!(result.message.is_none(), "{result:?}");
    }
    worker.cancel(7);
    // Deliver several external movements after cancellation, including after
    // the worker has consumed its cleanup wakeup. No new mode request supplies screens.
    for step in 0..3 {
        std::thread::sleep(Duration::from_millis(60));
        let rect = Rect::new(150.0 + step as f64 * 30.0, 200.0, 450.0, 320.0);
        {
            let mut native = fake.lock().unwrap();
            let moved = native.windows.get_mut(&WindowId(2)).unwrap();
            moved.info.bounds = rect;
            moved.restored = rect;
            native.events.push(TabNativeEvent::Changed(WindowId(2)));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let synchronized = {
                let native = fake.lock().unwrap();
                native.windows[&WindowId(1)].info.bounds != rect
                    && native.bars.len() == 1
                    && native.bars[0].visible
                    && native.bars[0].bounds == rect
            };
            if synchronized {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "group stopped following after leaving the mode"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    worker
        .stop_until(Instant::now() + Duration::from_secs(2))
        .unwrap();
}
fn choose(access: &mut Grouped<Fake>, id: u64) {
    access
        .tab_operation(
            TabOperation::Choose(WindowTarget::Window(WindowId(id))),
            &screens(),
            &|| false,
        )
        .unwrap();
}
fn op(access: &mut Grouped<Fake>, op: TabOperation) {
    access.tab_operation(op, &screens(), &|| false).unwrap();
}

fn two_groups() -> Grouped<Fake> {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    op(&mut access, TabOperation::EndGroup);
    choose(&mut access, 3);
    choose(&mut access, 4);
    access.reset();
    access
}
fn drop_event(
    access: &mut Grouped<Fake>,
    source: WindowTarget,
    target: u32,
    before: Option<u64>,
) -> Result<(), String> {
    access.native.events.push(TabNativeEvent::Drop(TabDrop {
        source,
        target: TabGroupId(target),
        before: before.map(WindowId),
    }));
    access.pump(&screens(), &|| false)
}

#[test]
fn drag_member_between_groups_outside_mode_preserves_both_group_positions_and_undo() {
    let mut access = two_groups();
    let source_frame = Rect::new(100.0, 130.0, 400.0, 300.0);
    let target_frame = Rect::new(700.0, 260.0, 400.0, 300.0);
    access
        .native
        .windows
        .get_mut(&WindowId(2))
        .unwrap()
        .info
        .bounds = source_frame;
    access
        .native
        .windows
        .get_mut(&WindowId(4))
        .unwrap()
        .info
        .bounds = target_frame;
    drop_event(&mut access, WindowTarget::Window(WindowId(2)), 2, Some(4)).unwrap();
    let state = access.tab_state().unwrap();
    assert_eq!(state.groups.len(), 1);
    assert_eq!(state.groups[0].members, [3, 2, 4].map(WindowId));
    assert_eq!(state.groups[0].active, WindowId(2));
    assert_eq!(
        access.native.windows[&WindowId(2)].info.bounds,
        target_frame
    );
    assert_eq!(
        access.native.windows[&WindowId(1)].info.bounds,
        source_frame
    );
    assert!(!access.native.hidden.contains(&WindowId(1)));
    assert_eq!(state.target, None);
    op(&mut access, TabOperation::Undo);
    assert_eq!(access.groups.state.groups.len(), 2);
    assert_eq!(
        access.native.windows[&WindowId(2)].info.bounds,
        source_frame
    );
    op(&mut access, TabOperation::Redo);
    assert_eq!(
        access.groups.state.groups[0].members,
        [3, 2, 4].map(WindowId)
    );
}

#[test]
fn drag_whole_group_keeps_order_and_only_moves_its_active_member() {
    let mut access = two_groups();
    let hidden_frame = access.native.windows[&WindowId(1)].info.bounds;
    let target_frame = access.native.windows[&WindowId(4)].info.bounds;
    drop_event(&mut access, WindowTarget::Group(TabGroupId(1)), 2, Some(3)).unwrap();
    assert_eq!(access.groups.state.groups.len(), 1);
    assert_eq!(
        access.groups.state.groups[0].members,
        [1, 2, 3, 4].map(WindowId)
    );
    assert_eq!(
        access.native.windows[&WindowId(1)].info.bounds,
        hidden_frame
    );
    assert_eq!(
        access.native.windows[&WindowId(2)].info.bounds,
        target_frame
    );
    assert_eq!(access.native.hidden.len(), 3);
}

#[test]
fn drag_reorders_within_a_group_and_stale_or_failed_drop_keeps_members() {
    let mut access = two_groups();
    drop_event(&mut access, WindowTarget::Window(WindowId(2)), 1, Some(1)).unwrap();
    assert_eq!(access.groups.state.groups[0].members, [2, 1].map(WindowId));
    let before = access.groups.state.clone();
    assert!(drop_event(&mut access, WindowTarget::Window(WindowId(2)), 9, None).is_err());
    assert_eq!(access.groups.state, before);
    access.native.fail = Some(WindowId(2));
    assert!(drop_event(&mut access, WindowTarget::Window(WindowId(2)), 2, None).is_err());
    assert_eq!(access.groups.state, before);
    assert_eq!(
        access.native.hidden,
        [WindowId(1), WindowId(3)].into_iter().collect()
    );
}

#[test]
fn native_selection_survives_denied_focus_and_stale_member_notifications() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    access.native.focus.set(None);
    access.native.deny_focus = true;
    access.native.events.extend([
        TabNativeEvent::Focused(WindowId(2)),
        TabNativeEvent::Changed(WindowId(2)),
    ]);
    assert!(
        access
            .activate_window(WindowId(1), &screens(), &|| false)
            .is_err()
    );
    access.pump(&screens(), &|| false).unwrap();
    assert_eq!(access.tab_state().unwrap().groups[0].active, WindowId(1));
    assert_eq!(access.native.bars[0].active, WindowId(1));
}

#[test]
fn closing_strip_dissolves_even_when_foreground_activation_is_denied() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    let group = access.tab_state().unwrap().groups[0].id;
    access.reset();
    access.native.deny_focus = true;
    access.native.events.push(TabNativeEvent::Dissolve(group));
    access.pump(&screens(), &|| false).unwrap();
    assert!(!access.persistent());
    assert!(access.native.bars.is_empty());
    assert_eq!(access.native.windows.len(), 4);
}

#[test]
fn first_group_undo_restores_both_original_frames_after_moving() {
    let mut access = setup();
    let original = [WindowId(1), WindowId(2)].map(|id| access.native.windows[&id].info.bounds);
    choose(&mut access, 1);
    choose(&mut access, 2);
    let moved = Rect::new(110.0, 120.0, 500.0, 350.0);
    access
        .set_frame(WindowId(2), moved, &screens(), &|| false)
        .unwrap();
    op(&mut access, TabOperation::Undo);
    assert!(!access.persistent());
    for (id, bounds) in [WindowId(1), WindowId(2)].into_iter().zip(original) {
        assert_eq!(access.native.windows[&id].info.bounds, bounds);
    }
    op(&mut access, TabOperation::Redo);
    assert!(access.persistent());
    assert_eq!(access.native.windows[&WindowId(1)].info.bounds, original[0]);
    assert_eq!(access.native.windows[&WindowId(2)].info.bounds, moved);
}

#[test]
fn completed_groups_survive_session_reset_and_move_only_active_member() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    op(&mut access, TabOperation::EndGroup);
    choose(&mut access, 3);
    choose(&mut access, 4);
    op(&mut access, TabOperation::EndGroup);
    let numbers = access.tab_state().unwrap().numbers;
    access.reset();
    assert_eq!(access.tab_state().unwrap().groups.len(), 2);
    assert_eq!(access.native.windows.len(), 4);
    assert_eq!(access.tab_state().unwrap().numbers, numbers);
    let rect = Rect::new(100.0, 150.0, 500.0, 350.0);
    access
        .set_frame(WindowId(2), rect, &screens(), &|| false)
        .unwrap();
    assert_ne!(access.native.windows[&WindowId(1)].info.bounds, rect);
    assert_eq!(access.native.windows[&WindowId(2)].info.bounds, rect);
    assert_ne!(access.native.windows[&WindowId(3)].info.bounds, rect);
    assert_eq!(access.layout_representative(WindowId(2)), WindowId(1));
}

#[test]
fn active_window_movements_never_emit_follower_writes() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    access.native.focus.set(None);
    access.reset();
    let writes = access.native.writes;
    for step in 0..120 {
        access
            .native
            .windows
            .get_mut(&WindowId(2))
            .unwrap()
            .info
            .bounds
            .x += 1.0;
        access
            .native
            .events
            .push(TabNativeEvent::Changed(WindowId(2)));
        access.pump(&screens(), &|| false).unwrap();
        assert_eq!(
            access.native.writes, writes,
            "follower write on movement {step}"
        );
    }
    assert_eq!(access.groups.state.groups.len(), 1);
    let frame = access.native.windows[&WindowId(2)].info.bounds;
    assert_ne!(access.native.windows[&WindowId(1)].info.bounds, frame);
    access
        .activate_window(WindowId(1), &screens(), &|| false)
        .unwrap();
    assert_eq!(access.native.writes, writes + 1);
    assert_eq!(access.native.windows[&WindowId(1)].info.bounds, frame);
    assert!(!access.native.hidden.contains(&WindowId(1)));
    assert!(access.native.hidden.contains(&WindowId(2)));
    access
        .native
        .events
        .push(TabNativeEvent::Dissolve(TabGroupId(1)));
    access.pump(&screens(), &|| false).unwrap();
    assert!(access.groups.state.groups.is_empty());
    assert_eq!(access.native.windows.len(), 4);
}
#[test]
fn auto_grouping_is_one_undo_step_and_redo_keeps_numbers() {
    let mut access = setup();
    for snapshot in access.native.windows.values_mut() {
        snapshot.info.app = if snapshot.info.id.0 <= 2 {
            "one"
        } else {
            "two"
        }
        .into();
    }
    op(&mut access, TabOperation::Enter { screen: 0 });
    assert_eq!(access.groups.state.groups.len(), 2);
    assert_eq!(access.groups.state.target, None);
    assert_eq!(access.history.len(), 1);
    let groups = access.groups.state.groups.clone();
    op(&mut access, TabOperation::Undo);
    assert!(access.groups.state.groups.is_empty());
    op(&mut access, TabOperation::Redo);
    assert_eq!(access.groups.state.groups, groups);
}
#[test]
fn switching_existing_member_does_not_change_geometry_or_history() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    let writes = access.native.writes;
    let history = access.history.len();
    choose(&mut access, 1);
    assert_eq!(access.native.writes, writes);
    assert_eq!(access.history.len(), history);
    assert_eq!(access.groups.state.groups[0].active, WindowId(1));
}

#[test]
fn tab_click_supersedes_older_foreground_notification() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    access.native.events = vec![
        TabNativeEvent::Focused(WindowId(2)),
        TabNativeEvent::Activate(WindowId(1)),
    ];
    access.pump(&screens(), &|| false).unwrap();
    assert_eq!(access.groups.state.groups[0].active, WindowId(1));
    assert_eq!(access.native.focus.get(), Some(WindowId(1)));
}
#[test]
fn failed_join_restores_windows_and_membership() {
    let mut access = setup();
    choose(&mut access, 1);
    let before = access.checkpoint(&[WindowId(1), WindowId(2)], &screens());
    access.native.fail = Some(WindowId(2));
    assert!(
        access
            .tab_operation(
                TabOperation::Choose(WindowTarget::Window(WindowId(2))),
                &screens(),
                &|| false
            )
            .is_err()
    );
    assert!(access.groups.state.groups.is_empty());
    assert!(access.history.is_empty());
    for snapshot in before.windows {
        assert!(same_state(
            &snapshot,
            &access.native.windows[&snapshot.info.id]
        ));
    }
}
#[test]
fn external_minimize_restore_close_and_dissolve_never_close_other_windows() {
    let mut access = setup();
    for id in [1, 2, 3] {
        choose(&mut access, id);
    }
    access
        .native
        .windows
        .get_mut(&WindowId(3))
        .unwrap()
        .info
        .minimized = true;
    access
        .native
        .events
        .push(TabNativeEvent::Changed(WindowId(3)));
    access.pump(&screens(), &|| false).unwrap();
    assert!(access.native.windows[&WindowId(3)].info.minimized);
    assert!(access.native.hidden.contains(&WindowId(1)));
    assert!(access.native.hidden.contains(&WindowId(2)));
    access
        .native
        .windows
        .get_mut(&WindowId(1))
        .unwrap()
        .info
        .minimized = false;
    access
        .native
        .events
        .push(TabNativeEvent::Changed(WindowId(1)));
    access.pump(&screens(), &|| false).unwrap();
    assert_eq!(access.groups.state.groups[0].active, WindowId(3));
    op(&mut access, TabOperation::Activate(WindowId(1)));
    assert!(!access.native.windows[&WindowId(1)].info.minimized);
    access.native.windows.remove(&WindowId(2));
    access
        .native
        .events
        .push(TabNativeEvent::Closed(WindowId(2)));
    access.pump(&screens(), &|| false).unwrap();
    assert_eq!(
        access.groups.state.group(TabGroupId(1)).unwrap().members,
        [WindowId(1), WindowId(3)]
    );
    let rect = access.native.windows[&WindowId(1)].info.bounds;
    op(&mut access, TabOperation::Dissolve);
    assert_eq!(access.native.windows.len(), 3);
    assert_eq!(access.native.windows[&WindowId(1)].info.bounds, rect);
    op(&mut access, TabOperation::Undo);
    assert!(!access.groups.state.groups[0].members.contains(&WindowId(2)));
}
#[test]
fn template_restore_is_atomic_and_rejects_duplicate_or_ineligible_members() {
    let mut access = setup();
    let operation = TabOperation::Restore {
        members: vec![WindowId(1), WindowId(2)],
        region: Rect::new(0.25, 0.25, 0.5, 0.5),
        screen: 0,
        active: 0,
    };
    access
        .native
        .windows
        .get_mut(&WindowId(2))
        .unwrap()
        .info
        .fullscreen = true;
    assert!(
        access
            .tab_operation(operation.clone(), &screens(), &|| false)
            .is_err()
    );
    assert_eq!(access.native.writes, 0);
    access
        .native
        .windows
        .get_mut(&WindowId(2))
        .unwrap()
        .info
        .fullscreen = false;
    op(&mut access, operation);
    assert_eq!(
        access.native.windows[&WindowId(1)].info.bounds,
        Rect::new(300.0, 200.0, 600.0, 400.0)
    );
    assert_eq!(access.groups.state.groups[0].active, WindowId(1));
    assert_eq!(access.groups.state.target, None);
}

#[test]
fn editor_layout_history_and_initial_restore_preserve_membership() {
    use crate::api::window::{WindowEditResult, WindowOperation};
    use crate::platform::common::window_session::WindowSessionProbe;
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    let original = access
        .snapshot(WindowId(1), &screens())
        .unwrap()
        .info
        .bounds;
    let members = access.groups.state.groups[0].members.clone();
    let mut session = WindowSessionProbe::new(WindowId(2));
    let started = session.execute(
        &mut access,
        WindowOperation::BeginEdit {
            transaction: 9,
            targets: Vec::new(),
            screen: Some(0),
            group: 9,
        },
        &screens(),
    );
    let Some(WindowEditResult::Started { minimums, .. }) = started.edit.as_deref() else {
        panic!("missing edit: {started:?}");
    };
    assert_eq!(minimums.len(), 3);
    assert_eq!(
        started.windows.as_ref().unwrap().len(),
        4,
        "window identities remain visible"
    );
    let applied = session.execute(
        &mut access,
        WindowOperation::ApplyLayout {
            additional_screens: Vec::new(),
            transaction: 9,
            revision: 1,
            screen: 0,
            gap: 0.0,
            strict: true,
            placements: vec![
                (WindowId(1), Rect::new(0.0, 0.0, 0.5, 1.0)),
                (WindowId(3), Rect::new(0.5, 0.0, 0.5, 0.5)),
                (WindowId(4), Rect::new(0.5, 0.5, 0.5, 0.5)),
            ],
        },
        &screens(),
    );
    assert!(applied.message.is_none(), "{applied:?}");
    let arranged = access
        .snapshot(WindowId(1), &screens())
        .unwrap()
        .info
        .bounds;
    assert_ne!(arranged, original);
    assert_eq!(
        access
            .snapshot(WindowId(2), &screens())
            .unwrap()
            .info
            .bounds,
        arranged
    );
    session.execute(
        &mut access,
        WindowOperation::EndEdit {
            transaction: 9,
            commit: true,
        },
        &screens(),
    );
    session.execute(&mut access, WindowOperation::Undo, &screens());
    assert_eq!(
        access
            .snapshot(WindowId(2), &screens())
            .unwrap()
            .info
            .bounds,
        original
    );
    session.execute(&mut access, WindowOperation::Redo, &screens());
    assert_eq!(
        access
            .snapshot(WindowId(2), &screens())
            .unwrap()
            .info
            .bounds,
        arranged
    );
    session.execute(
        &mut access,
        WindowOperation::ResetInitial { group: 10 },
        &screens(),
    );
    assert_eq!(
        access
            .snapshot(WindowId(2), &screens())
            .unwrap()
            .info
            .bounds,
        original
    );
    assert_eq!(access.groups.state.groups[0].members, members);
}

#[test]
fn append_uses_current_active_frame_after_hidden_anchor_stayed_behind() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    let frame = Rect::new(210.0, 170.0, 520.0, 330.0);
    access
        .set_frame(WindowId(2), frame, &screens(), &|| false)
        .unwrap();
    let first = access.native.windows[&WindowId(1)].info.bounds;
    let writes = access.native.writes;
    access.reset();
    op(
        &mut access,
        TabOperation::Choose(WindowTarget::Group(TabGroupId(1))),
    );
    choose(&mut access, 3);
    assert_eq!(access.native.windows[&WindowId(1)].info.bounds, first);
    assert_eq!(access.native.windows[&WindowId(3)].info.bounds, frame);
    assert_eq!(access.native.writes, writes + 1);
    assert_eq!(
        access.native.hidden,
        BTreeSet::from([WindowId(1), WindowId(2)])
    );
}

#[test]
fn closing_active_member_reveals_survivor_even_when_alignment_is_refused() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    access
        .set_frame(
            WindowId(2),
            Rect::new(200.0, 200.0, 500.0, 330.0),
            &screens(),
            &|| false,
        )
        .unwrap();
    access.native.windows.remove(&WindowId(2));
    access.native.fail = Some(WindowId(1));
    access
        .native
        .events
        .push(TabNativeEvent::Closed(WindowId(2)));
    assert!(access.pump(&screens(), &|| false).is_err());
    assert!(!access.native.hidden.contains(&WindowId(1)));
    assert!(!access.persistent());
}

#[test]
fn close_visibility_failure_retries_after_the_last_group_is_gone() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    access.native.windows.remove(&WindowId(2));
    access.native.fail_visibility = Some((WindowId(1), false));
    access
        .native
        .events
        .push(TabNativeEvent::Closed(WindowId(2)));
    // The first attempt may report the failure while its recovery already succeeds.
    let _ = access.pump(&screens(), &|| false);
    access.pump(&screens(), &|| false).unwrap();
    assert!(!access.persistent());
    assert!(access.native.hidden.is_empty());
}

#[test]
fn failed_switch_keeps_previous_member_visible_and_dissolve_reveals_everyone() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    access
        .set_frame(
            WindowId(2),
            Rect::new(180.0, 170.0, 510.0, 340.0),
            &screens(),
            &|| false,
        )
        .unwrap();
    access.native.fail = Some(WindowId(1));
    assert!(
        access
            .activate_window(WindowId(1), &screens(), &|| false)
            .is_err()
    );
    assert_eq!(access.groups.state.groups[0].active, WindowId(2));
    assert!(access.native.hidden.contains(&WindowId(1)));
    assert!(!access.native.hidden.contains(&WindowId(2)));
    op(&mut access, TabOperation::Dissolve);
    assert!(access.native.hidden.is_empty());
}

#[test]
fn enumeration_observing_close_before_callback_still_aligns_and_reveals_survivor() {
    let mut access = setup();
    choose(&mut access, 1);
    choose(&mut access, 2);
    let frame = Rect::new(210.0, 200.0, 500.0, 330.0);
    access
        .set_frame(WindowId(2), frame, &screens(), &|| false)
        .unwrap();
    access.native.windows.remove(&WindowId(2));
    access.native.closed.push(WindowId(2));
    let windows = access.enumerate(&screens(), &|| false).unwrap();
    assert_eq!(access.native.windows[&WindowId(1)].info.bounds, frame);
    assert!(!access.native.hidden.contains(&WindowId(1)));
    assert_eq!(access.take_closed(), [WindowId(2)]);
    assert!(windows.iter().all(|window| window.id != WindowId(2)));
}

#[test]
fn state_changes_reserve_six_until_close_or_new_session() {
    use crate::api::window::WindowScope;
    let mut access = setup();
    let sample = access.native.windows[&WindowId(1)].clone();
    access.native.windows.clear();
    for id in 1..=7 {
        let mut window = sample.clone();
        window.info.id = WindowId(id);
        window.info.app = "Browser".into();
        access.native.windows.insert(WindowId(id), window);
    }
    let scope = Some(WindowScope {
        screen: Some(0),
        include_minimized: false,
    });
    access.set_scope(scope, true);
    access.enumerate(&screens(), &|| false).unwrap();
    assert!(access.groups.state.numbers.contains(&(WindowId(6), 6)));
    for _ in 0..3 {
        for (maximized, minimized) in [(true, false), (false, true), (false, false)] {
            let window = &mut access.native.windows.get_mut(&WindowId(6)).unwrap().info;
            window.maximized = maximized;
            window.minimized = minimized;
            let windows = access.enumerate(&screens(), &|| false).unwrap();
            assert_eq!(windows.iter().any(|w| w.id == WindowId(6)), !minimized);
            assert!(access.groups.state.numbers.contains(&(WindowId(6), 6)));
            assert_eq!(access.groups.state.numbers.len(), 7);
        }
    }
    access
        .native
        .windows
        .get_mut(&WindowId(6))
        .unwrap()
        .info
        .minimized = true;
    access.enumerate(&screens(), &|| false).unwrap();
    let mut added = sample;
    added.info.id = WindowId(8);
    access.native.windows.insert(WindowId(8), added);
    access.enumerate(&screens(), &|| false).unwrap();
    assert!(access.groups.state.numbers.contains(&(WindowId(8), 8)));
    access
        .native
        .windows
        .get_mut(&WindowId(6))
        .unwrap()
        .info
        .minimized = false;
    access.enumerate(&screens(), &|| false).unwrap();
    assert!(access.groups.state.numbers.contains(&(WindowId(6), 6)));
    access.native.windows.remove(&WindowId(6));
    access.native.closed.push(WindowId(6));
    access.enumerate(&screens(), &|| false).unwrap();
    assert!(
        !access
            .groups
            .state
            .numbers
            .iter()
            .any(|(id, _)| *id == WindowId(6))
    );
    access.set_scope(scope, true);
    access.enumerate(&screens(), &|| false).unwrap();
    let mut numbers: Vec<_> = access
        .groups
        .state
        .numbers
        .iter()
        .map(|(_, n)| *n)
        .collect();
    numbers.sort_unstable();
    assert_eq!(numbers, (1..=7).collect::<Vec<_>>());
}

#[test]
fn window_close_resolves_active_tab_and_dissolves_only_after_confirmed_closure() {
    let mut access = setup();
    for id in [1, 2, 3] {
        choose(&mut access, id);
    }
    for (closed, remaining) in [(WindowId(3), 2), (WindowId(2), 1)] {
        access.close(WindowId(1)).unwrap();
        assert_eq!(access.native.close_requests.borrow().last(), Some(&closed));
        assert_eq!(
            access.groups.state.groups[0].members.len(),
            remaining + 1,
            "a save/cancel dialog must not dissolve the group"
        );
        access.native.windows.remove(&closed);
        access.native.events.push(TabNativeEvent::Closed(closed));
        access.pump(&screens(), &|| false).unwrap();
        if remaining == 2 {
            assert_eq!(
                access.groups.state.groups[0].members,
                vec![WindowId(1), WindowId(2)]
            );
            assert_eq!(access.groups.state.groups[0].active, WindowId(2));
        } else {
            assert!(access.groups.state.groups.is_empty());
            assert!(access.native.bars.is_empty());
            assert!(!access.native.hidden.contains(&WindowId(1)));
            assert!(access.native.windows.contains_key(&WindowId(1)));
        }
    }
    assert_eq!(
        *access.native.close_requests.borrow(),
        vec![WindowId(3), WindowId(2)]
    );
    assert!(
        access.native.windows.contains_key(&WindowId(4)),
        "unrelated window stays open"
    );
}

#[test]
#[ignore = "explicit release-profile tab geometry baseline; uses only a simulated backend"]
fn tabs_geometry_baseline() {
    let mut rows = vec!["members,round,p50_ns,p95_ns,p99_ns,snapshots,writes".to_string()];
    for members in [2, 10, 30] {
        for round in 0..3 {
            let mut access = setup();
            let template = access.native.windows[&WindowId(1)].clone();
            access.native.windows.clear();
            for id in 1..=members {
                let mut value = template.clone();
                value.info.id = WindowId(id);
                value.info.title = format!("Window {id}");
                access.native.windows.insert(WindowId(id), value);
            }
            access.enumerate(&screens(), &|| false).unwrap();
            for id in 1..=members {
                choose(&mut access, id);
            }
            let active = access.groups.state.groups[0].active;
            let displays = screens();
            let mut samples = Vec::with_capacity(1000);
            let before = access.native.snapshot_reads.get();
            let writes = access.native.writes;
            for step in 0..1000 {
                let bounds = &mut access.native.windows.get_mut(&active).unwrap().info.bounds;
                bounds.x = 100.0 + (step % 200) as f64;
                bounds.width = 400.0 + (step % 100) as f64;
                access
                    .native
                    .events
                    .push(TabNativeEvent::GeometryChanged(active));
                let started = Instant::now();
                access.pump(&displays, &|| false).unwrap();
                samples.push(started.elapsed().as_nanos());
            }
            samples.sort_unstable();
            rows.push(format!(
                "{members},{round},{},{},{},{},{}",
                samples[500],
                samples[950],
                samples[990],
                access.native.snapshot_reads.get() - before,
                access.native.writes - writes
            ));
        }
    }
    let path =
        std::env::var("KEYSTEER_TABS_BENCH_OUTPUT").expect("set an output path under target");
    std::fs::write(path, rows.join("\n")).unwrap();
}

#[test]
fn geometry_updates_only_affected_group_and_retries_failed_publication() {
    let mut access = two_groups();
    let active = access.groups.state.groups[0].active;
    let group = access.groups.state.groups[0].id;
    let other = access.native.bars[1].clone();
    access.native.snapshot_reads.set(0);
    access.native.updates.clear();
    let writes = access.native.writes;
    access
        .native
        .windows
        .get_mut(&active)
        .unwrap()
        .info
        .bounds
        .x += 10.0;
    access
        .native
        .events
        .push(TabNativeEvent::GeometryChanged(active));
    access.native.fail_bar = true;
    assert!(access.pump(&screens(), &|| false).is_err());
    access.pump(&screens(), &|| false).unwrap();
    assert_eq!(access.native.snapshot_reads.get(), 1);
    assert_eq!(access.native.updates, [group]);
    assert_eq!(access.native.bars[1], other);
    assert_eq!(access.native.writes, writes);
}

#[test]
fn native_gesture_defers_header_correction_until_end() {
    let mut access = setup();
    access.native.header = 30.0;
    choose(&mut access, 1);
    choose(&mut access, 2);
    let active = access.groups.state.groups[0].active;
    let hidden = access.native.windows[&WindowId(1)].info.bounds;
    let writes = access.native.writes;
    access
        .native
        .events
        .push(TabNativeEvent::MoveResizeStarted(active));
    for x in 0..20 {
        access.native.windows.get_mut(&active).unwrap().info.bounds =
            Rect::new(x as f64, 0.0, 600.0, 400.0);
        access
            .native
            .events
            .push(TabNativeEvent::GeometryChanged(active));
        access.pump(&screens(), &|| false).unwrap();
    }
    assert_eq!(access.native.writes, writes);
    access
        .native
        .events
        .push(TabNativeEvent::MoveResizeEnded(active));
    access.pump(&screens(), &|| false).unwrap();
    assert_eq!(access.native.writes, writes + 1);
    assert_eq!(access.native.windows[&active].info.bounds.y, 30.0);
    assert_eq!(access.native.windows[&WindowId(1)].info.bounds, hidden);
    access
        .native
        .events
        .push(TabNativeEvent::GeometryChanged(active));
    access.pump(&screens(), &|| false).unwrap();
    assert_eq!(access.native.writes, writes + 1);
}
