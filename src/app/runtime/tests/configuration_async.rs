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
            Ok(ConfigurationCandidate { plan: crate::app::configuration::compile(&Config::default())?, repository: Box::new(repository), source_path: None })
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
    engine.configuration_work.shutdown().unwrap();
}
