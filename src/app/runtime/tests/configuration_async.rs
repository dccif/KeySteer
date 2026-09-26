#[test]
fn background_configuration_does_not_block_and_queued_changes_use_the_latest_repository() {
    use std::sync::{Condvar, mpsc};
    #[derive(Clone)]
    struct Repository {
        value: u32,
        gate: Arc<(Mutex<bool>, Condvar)>,
        entered: mpsc::Sender<std::thread::ThreadId>,
    }
    impl ConfigurationRepository for Repository {
        fn fork_for_worker(&self) -> Option<Box<dyn ConfigurationRepository>> { Some(Box::new(self.clone())) }
        fn source_text(&self) -> Result<String, String> { Ok(self.value.to_string()) }
        fn source_path(&self) -> Option<std::path::PathBuf> { None }
        fn reload_candidate(&self) -> Result<ConfigurationCandidate, String> { self.set_candidate("", "0") }
        fn set_candidate(&self, _: &str, value: &str) -> Result<ConfigurationCandidate, String> {
            self.entered.send(std::thread::current().id()).unwrap();
            let (lock, ready) = &*self.gate;
            let _guard = ready.wait_while(lock.lock().unwrap(), |released| !*released).unwrap();
            let increment: u32 = value.parse().map_err(|_| "invalid configuration")?;
            let mut repository = self.clone();
            repository.value += increment;
            Ok(ConfigurationCandidate { plan: crate::app::configuration::compile(&Config::default())?, repository: Box::new(repository), source_path: None, restart: None })
        }
    }
    let (entered, entering) = mpsc::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    engine.attach_configuration(Box::new(Repository { value: 1, gate: gate.clone(), entered }));
    let (mut backend, _) = FakeBackend::new(vec![]);
    let (events, received) = mpsc::channel();
    backend.event_sender = Some(events);
    let set = |value: &str| configuration_work::Operation::Set { path: "test".into(), value: value.into() };
    engine.request_configuration(set("2"), &mut backend).unwrap();
    let worker = entering.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_ne!(worker, std::thread::current().id());
    // The worker remains blocked, while the engine can accept more input.
    engine.request_configuration(set("invalid"), &mut backend).unwrap();
    engine.request_configuration(set("4"), &mut backend).unwrap();
    assert_eq!(engine.configuration.as_ref().unwrap().source_text().unwrap(), "1");
    assert!(received.try_recv().is_err());
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    for expected in ["3", "3", "7"] {
        assert!(matches!(received.recv_timeout(Duration::from_secs(2)).unwrap(), BackendEvent::ConfigurationReady));
        engine.finish_configuration(&mut backend).unwrap();
        assert_eq!(engine.configuration.as_ref().unwrap().source_text().unwrap(), expected);
    }
    engine.configuration_work.assert_released();
    engine.configuration_work.shutdown().unwrap();
}

#[test]
fn failed_background_reload_releases_work_and_discards_queue_before_retry() {
    use std::sync::{Condvar, mpsc};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    #[derive(Clone)]
    struct Repository {
        gate: Arc<(Mutex<bool>, Condvar)>,
        valid: Arc<AtomicBool>,
        calls: Arc<AtomicUsize>,
        lifetime: Arc<()>,
    }
    impl ConfigurationRepository for Repository {
        fn fork_for_worker(&self) -> Option<Box<dyn ConfigurationRepository>> { Some(Box::new(self.clone())) }
        fn source_text(&self) -> Result<String, String> { Ok("last-valid".into()) }
        fn source_path(&self) -> Option<std::path::PathBuf> { None }
        fn reload_candidate(&self) -> Result<ConfigurationCandidate, String> {
            assert!(Arc::strong_count(&self.lifetime) >= 2);
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (lock, ready) = &*self.gate;
            let _guard = ready.wait_while(lock.lock().unwrap(), |released| !*released).unwrap();
            if !self.valid.load(Ordering::SeqCst) { return Err("invalid configuration".into()); }
            Ok(ConfigurationCandidate {
                plan: crate::app::configuration::compile(&Config::default())?,
                repository: Box::new(self.clone()), source_path: None, restart: None,
            })
        }
        fn set_candidate(&self, _: &str, _: &str) -> Result<ConfigurationCandidate, String> {
            panic!("queued change must be discarded after failed Reload")
        }
    }
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let valid = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let lifetime = Arc::new(());
    let mut engine = Engine::from_plan(crate::app::configuration::compile(&Config::default()).unwrap(), Appearance::Dark).unwrap();
    engine.attach_configuration(Box::new(Repository { gate: gate.clone(), valid: valid.clone(), calls: calls.clone(), lifetime: lifetime.clone() }));
    let (mut backend, log) = FakeBackend::new(vec![]);
    let (events, received) = mpsc::channel();
    backend.event_sender = Some(events);
    engine.activate(ModeId::normal(), None, &mut backend).unwrap();
    engine.request_configuration(configuration_work::Operation::Reload, &mut backend).unwrap();
    engine.request_configuration(configuration_work::Operation::Set { path: "test".into(), value: "queued".into() }, &mut backend).unwrap();
    engine.request_configuration(configuration_work::Operation::Reload, &mut backend).unwrap();
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    assert!(matches!(received.recv_timeout(Duration::from_secs(2)).unwrap(), BackendEvent::ConfigurationReady));
    engine.finish_configuration(&mut backend).unwrap();
    engine.configuration_work.assert_released();
    assert_eq!(Arc::strong_count(&lifetime), 2); // only test and active repository
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(engine.active_mode(), &ModeId::normal());
    assert_eq!(engine.configuration.as_ref().unwrap().source_text().unwrap(), "last-valid");
    assert_eq!(log.lock().unwrap().shutdowns, 0);
    assert!(!engine.should_quit);
    assert!(received.try_recv().is_err());
    // A corrected file can be processed by a fresh worker on the next click.
    valid.store(true, Ordering::SeqCst);
    engine.request_configuration(configuration_work::Operation::Reload, &mut backend).unwrap();
    assert!(matches!(received.recv_timeout(Duration::from_secs(2)).unwrap(), BackendEvent::ConfigurationReady));
    engine.finish_configuration(&mut backend).unwrap();
    engine.configuration_work.assert_released();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(Arc::strong_count(&lifetime), 2);
}
