#[test]
fn process_reload_defers_commit_until_old_runtime_shutdown() {
    struct IdleTimer;
    impl Mode for IdleTimer {
        fn id(&self) -> ModeId { ModeId::idle() }
        fn handle(&mut self, event: &ModeEvent, _: &HostContext<'_>) -> CommandBatch {
            match event {
                ModeEvent::Activated { .. } => CommandBatch::from(vec![Command::SetTimer {
                    id: "old-plan".into(), delay: Duration::ZERO, repeating: false,
                }]),
                ModeEvent::Timer { .. } => panic!("old timer fired after Reload was accepted"),
                _ => CommandBatch::default(),
            }
        }
    }
    struct Restart(Arc<Mutex<Recorder>>);
    impl PreparedRestart for Restart {
        fn commit(self: Box<Self>) -> Result<(), String> {
            let mut log = self.0.lock().unwrap();
            assert_eq!(log.shutdowns, 1);
            log.timeline.push("restart");
            Ok(())
        }
    }
    struct Repository(Arc<Mutex<Recorder>>);
    impl ConfigurationRepository for Repository {
        fn source_text(&self) -> Result<String, String> { Ok("old".into()) }
        fn source_path(&self) -> Option<std::path::PathBuf> { None }
        fn reload_candidate(&self) -> Result<ConfigurationCandidate, String> {
            let mut config = Config::default();
            config.grid.enabled = false;
            Ok(ConfigurationCandidate {
                plan: crate::app::configuration::compile(&config)?,
                repository: Box::new(Self(self.0.clone())),
                source_path: None,
                restart: Some(Box::new(Restart(self.0.clone()))),
            })
        }
        fn set_candidate(&self, _: &str, _: &str) -> Result<ConfigurationCandidate, String> { unreachable!() }
    }
    let (mut backend, log) = FakeBackend::new(vec![BackendEvent::ReloadConfig]);
    let mut engine = Engine::from_plan(crate::app::configuration::compile(&Config::default()).unwrap(), Appearance::Dark).unwrap();
    engine.attach_configuration(Box::new(Repository(log.clone())));
    engine.register(Box::new(IdleTimer));
    engine.run(&mut backend).unwrap();
    // No partial installation of the candidate into the retiring engine.
    assert!(engine.registry.contains_key(&ModeId::grid()));
    assert_eq!(engine.configuration.as_ref().unwrap().source_text().unwrap(), "old");
    assert_eq!(log.lock().unwrap().shutdowns, 1);
    assert!(engine.scheduler.timers.is_empty());
    assert!(!log.lock().unwrap().timeline.contains(&"restart"));
    let restart = engine.take_restart().unwrap();
    drop(engine);
    drop(backend);
    restart.commit().unwrap();
    assert!(log.lock().unwrap().timeline.contains(&"restart"));
}

#[test]
fn invalid_process_reload_keeps_active_mode_and_configuration() {
    let directory = std::env::temp_dir().join(format!("keysteer-invalid-process-reload-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("keysteer.user.toml");
    let config = Config::default();
    let source = config.to_toml().unwrap();
    let store = ConfigStore::from_validated_text(path.clone(), source.clone(), crate::platform::atomic_replace);
    let mut engine = Engine::from_plan(crate::app::configuration::compile(&config).unwrap(), Appearance::Dark).unwrap();
    engine.attach_configuration(Box::new(crate::app::configuration::ConfigRepository::new(config, source.clone(), Some(store), None).with_process_reload()));
    let (mut backend, log) = FakeBackend::new(vec![]);
    engine.activate(ModeId::normal(), None, &mut backend).unwrap();
    for invalid in ["[", "[normal.targeting]\nmethod = 'grid'\nkeys = 'h'\ngrid_cols = 1\ngrid_rows = 1\n"] {
        std::fs::write(&path, invalid).unwrap();
        assert!(engine.reload_config(&mut backend).is_err());
        assert!(!engine.should_quit);
        assert!(engine.take_restart().is_none());
        assert_eq!(engine.active_mode(), &ModeId::normal());
        assert_eq!(engine.configuration.as_ref().unwrap().source_text().unwrap(), source);
        assert_eq!(log.lock().unwrap().shutdowns, 0);
    }
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn successful_background_reload_releases_configuration_resources_before_handoff() {
    use std::sync::mpsc;
    struct Restart;
    impl PreparedRestart for Restart {
        fn commit(self: Box<Self>) -> Result<(), String> { Ok(()) }
    }
    #[derive(Clone)]
    struct Repository(Arc<()>);
    impl ConfigurationRepository for Repository {
        fn fork_for_worker(&self) -> Option<Box<dyn ConfigurationRepository>> { Some(Box::new(self.clone())) }
        fn source_text(&self) -> Result<String, String> { Ok("old".into()) }
        fn source_path(&self) -> Option<std::path::PathBuf> { None }
        fn reload_candidate(&self) -> Result<ConfigurationCandidate, String> {
            assert!(Arc::strong_count(&self.0) >= 2);
            Ok(ConfigurationCandidate {
                plan: crate::app::configuration::compile(&Config::default())?,
                repository: Box::new(self.clone()), source_path: None,
                restart: Some(Box::new(Restart)),
            })
        }
        fn set_candidate(&self, _: &str, _: &str) -> Result<ConfigurationCandidate, String> {
            panic!("retiring process must not execute queued configuration changes")
        }
    }
    let lifetime = Arc::new(());
    let mut engine = Engine::from_plan(crate::app::configuration::compile(&Config::default()).unwrap(), Appearance::Dark).unwrap();
    engine.attach_configuration(Box::new(Repository(lifetime.clone())));
    let (mut backend, _) = FakeBackend::new(vec![]);
    let (events, received) = mpsc::channel();
    backend.event_sender = Some(events);
    engine.request_configuration(configuration_work::Operation::Reload, &mut backend).unwrap();
    engine.request_configuration(configuration_work::Operation::Set { path: "test".into(), value: "queued".into() }, &mut backend).unwrap();
    assert!(matches!(received.recv_timeout(Duration::from_secs(2)).unwrap(), BackendEvent::ConfigurationReady));
    engine.finish_configuration(&mut backend).unwrap();
    engine.configuration_work.assert_released();
    assert!(engine.should_quit);
    assert_eq!(Arc::strong_count(&lifetime), 2); // candidate and worker copies were released
    assert!(received.try_recv().is_err());
    let restart = engine.take_restart().unwrap();
    drop(engine);
    drop(backend);
    assert_eq!(Arc::strong_count(&lifetime), 1);
    restart.commit().unwrap();
}
