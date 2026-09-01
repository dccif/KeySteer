//! OCR provider discovery, cached descriptors, and shutdown.

use super::*;

#[derive(Clone, Debug)]
pub(super) struct SystemOcrDescriptor {
    pub(super) languages: Vec<String>,
    pub(super) maximum_dimension: u32,
}

#[derive(Debug, Default)]
pub(super) struct OcrDiscoverySnapshot {
    pub(super) system: Option<Arc<SystemOcrDescriptor>>,
    pub(super) wechat: Option<Arc<WechatDescriptor>>,
}

#[derive(Debug)]
pub(super) enum OcrExecutionPlan {
    None,
    SystemOnly(Arc<SystemOcrDescriptor>),
    WechatOnly(Arc<WechatDescriptor>),
    Dual {
        system: Arc<SystemOcrDescriptor>,
        wechat: Arc<WechatDescriptor>,
    },
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OcrExecutionKind {
    None,
    SystemOnly,
    WechatOnly,
    Dual,
}

#[cfg(test)]
pub(super) fn ocr_execution_kind(
    system_available: bool,
    wechat_available: bool,
    detect_text: bool,
) -> OcrExecutionKind {
    if !detect_text {
        return OcrExecutionKind::None;
    }
    match (system_available, wechat_available) {
        (false, false) => OcrExecutionKind::None,
        (true, false) => OcrExecutionKind::SystemOnly,
        (false, true) => OcrExecutionKind::WechatOnly,
        (true, true) => OcrExecutionKind::Dual,
    }
}

impl OcrExecutionPlan {
    pub(super) fn from_snapshot(snapshot: &OcrDiscoverySnapshot, detect_text: bool) -> Self {
        if !detect_text {
            return Self::None;
        }
        match (&snapshot.system, &snapshot.wechat) {
            (Some(system), Some(wechat)) => Self::Dual {
                system: Arc::clone(system),
                wechat: Arc::clone(wechat),
            },
            (Some(system), None) => Self::SystemOnly(Arc::clone(system)),
            (None, Some(wechat)) => Self::WechatOnly(Arc::clone(wechat)),
            (None, None) => Self::None,
        }
    }

    pub(super) fn into_descriptors(
        self,
    ) -> (
        Option<Arc<SystemOcrDescriptor>>,
        Option<Arc<WechatDescriptor>>,
    ) {
        match self {
            Self::None => (None, None),
            Self::SystemOnly(system) => (Some(system), None),
            Self::WechatOnly(wechat) => (None, Some(wechat)),
            Self::Dual { system, wechat } => (Some(system), Some(wechat)),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum DiscoveryState {
    Pending,
    Ready(Arc<OcrDiscoverySnapshot>),
    Unavailable,
}

pub(super) struct DiscoveryShared {
    pub(super) state: Mutex<DiscoveryState>,
    pub(super) ready: Condvar,
    pub(super) stopping: AtomicBool,
    pub(super) started: AtomicBool,
    pub(super) completed: AtomicBool,
}

impl Default for DiscoveryShared {
    fn default() -> Self {
        Self {
            state: Mutex::new(DiscoveryState::Pending),
            ready: Condvar::new(),
            stopping: AtomicBool::new(false),
            started: AtomicBool::new(false),
            completed: AtomicBool::new(false),
        }
    }
}

#[derive(Clone)]
pub(super) struct DiscoveryHandle(pub(super) Arc<DiscoveryShared>);

impl DiscoveryHandle {
    pub(super) fn wait(
        &self,
        deadline: Instant,
        cancelled: impl Fn() -> bool,
    ) -> Option<Arc<OcrDiscoverySnapshot>> {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            match &*state {
                DiscoveryState::Ready(snapshot) => return Some(Arc::clone(snapshot)),
                DiscoveryState::Unavailable => {
                    return Some(Arc::new(OcrDiscoverySnapshot::default()));
                }
                DiscoveryState::Pending => {}
            }
            if cancelled() || Instant::now() >= deadline {
                return None;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            state = self
                .0
                .ready
                .wait_timeout(state, remaining)
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
    }
}

pub(super) struct OcrDiscovery {
    pub(super) shared: Arc<DiscoveryShared>,
    pub(super) worker: Option<WorkerJoin>,
}

impl OcrDiscovery {
    pub(super) fn new() -> Self {
        Self {
            shared: Arc::new(DiscoveryShared::default()),
            worker: None,
        }
    }

    pub(super) fn start(&mut self) {
        if self.shared.stopping.load(Ordering::Acquire)
            || self
                .shared
                .started
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }
        let worker_shared = Arc::clone(&self.shared);
        self.worker = WorkerJoin::spawn(
            "Windows OCR discovery",
            std::thread::Builder::new().name("keysteer-ocr-discovery".into()),
            move || discover_ocr(worker_shared),
        )
        .map_err(|error| {
            crate::support::logging::report_error("windows-vision", &error);
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            *state = DiscoveryState::Unavailable;
            self.shared.completed.store(true, Ordering::Release);
            self.shared.ready.notify_all();
        })
        .ok();
    }

    pub(super) fn handle(&self) -> DiscoveryHandle {
        DiscoveryHandle(Arc::clone(&self.shared))
    }

    pub(super) fn reap_finished(&mut self) -> Result<(), String> {
        if !self.shared.completed.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(worker) = self.worker.as_mut()
            && worker.reap_finished()?
        {
            self.worker.take();
        }
        Ok(())
    }

    pub(super) fn stop_until(&mut self, deadline: Instant) -> Result<(), String> {
        self.shared.stopping.store(true, Ordering::Release);
        self.shared.ready.notify_all();
        if let Some(worker) = self.worker.as_mut() {
            worker.join_until(deadline)?;
        }
        self.worker.take();
        Ok(())
    }
}

pub(super) fn discover_ocr(shared: Arc<DiscoveryShared>) {
    if shared.stopping.load(Ordering::Acquire) {
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *state = DiscoveryState::Unavailable;
        shared.completed.store(true, Ordering::Release);
        shared.ready.notify_all();
        return;
    }
    if let Err(error) = crate::platform::windows::native::prefer_background_work() {
        crate::report_warning!(
            "windows-vision",
            "cannot lower OCR discovery priority: {error}"
        );
    }
    let snapshot = probe_ocr(|| shared.stopping.load(Ordering::Acquire));
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    *state = match snapshot {
        Some(snapshot) if snapshot.system.is_some() || snapshot.wechat.is_some() => {
            DiscoveryState::Ready(Arc::new(snapshot))
        }
        _ => DiscoveryState::Unavailable,
    };
    shared.completed.store(true, Ordering::Release);
    shared.ready.notify_all();
    crate::support::perf_probe::mark("ocr_ready");
}

pub(super) fn probe_ocr(cancelled: impl Fn() -> bool + Copy) -> Option<OcrDiscoverySnapshot> {
    if cancelled() {
        return None;
    }
    let system = match probe_system_ocr(cancelled) {
        Ok(descriptor) => {
            crate::log_info!(
                "windows-vision",
                "system OCR discovered (languages [{}], maximum image dimension {})",
                descriptor.languages.join(", "),
                descriptor.maximum_dimension
            );
            Some(Arc::new(descriptor))
        }
        Err(_error) if cancelled() => return None,
        Err(error) => {
            crate::report_warning!("windows-vision", "system OCR unavailable: {error}");
            None
        }
    };
    if cancelled() {
        return None;
    }
    let wechat = match crate::platform::windows::wechat_ocr::discover_descriptor() {
        Ok(Some(descriptor)) => {
            crate::log_info!(
                "windows-vision",
                "WeChat OCR discovered ({})",
                descriptor.description()
            );
            Some(Arc::new(descriptor))
        }
        Ok(None) => {
            crate::report_warning!(
                "windows-vision",
                "WeChat OCR unavailable: optional components were not found"
            );
            None
        }
        Err(error) => {
            crate::report_warning!("windows-vision", "WeChat OCR unavailable: {error}");
            None
        }
    };
    Some(OcrDiscoverySnapshot { system, wechat })
}

pub(super) fn probe_system_ocr(
    cancelled: impl Fn() -> bool,
) -> Result<SystemOcrDescriptor, String> {
    let apartment = crate::platform::windows::native::ComApartment::initialise()?;
    let result = (|| {
        if cancelled() {
            return Err("system OCR discovery cancelled".into());
        }
        let (maximum, languages) = crate::platform::windows::native::probe_system_ocr_factory()?;
        Ok(SystemOcrDescriptor {
            languages,
            maximum_dimension: maximum,
        })
    })();
    drop(apartment);
    result
}
