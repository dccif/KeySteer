//! Owned kernel handles and child-process containment.
use windows::Win32::Foundation::HANDLE;

#[must_use = "closing the job is the fail-safe that terminates its helper process"]
pub(crate) struct KillOnCloseJob(OwnedHandle);

impl KillOnCloseJob {
    pub(crate) fn create() -> Result<Self, String> {
        use windows::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };

        // SAFETY: no security attributes or name are supplied. The returned
        // handle transfers immediately into the owner before configuration.
        let job = Self(OwnedHandle::new(
            unsafe { CreateJobObjectW(None, None) }
                .map_err(|error| format!("cannot create WeChat OCR job object: {error}"))?,
        ));
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the pointer and byte count describe the exact initialized
        // information struct and remain valid for this synchronous call.
        unsafe {
            SetInformationJobObject(
                job.0.raw(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        }
        .map_err(|error| format!("cannot configure WeChat OCR job object: {error}"))?;
        Ok(job)
    }

    pub(crate) fn assign(&self, process: HANDLE) -> Result<(), String> {
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;

        // SAFETY: both handles are live for the duration of this synchronous
        // call; ownership of the process handle remains with `Child`.
        unsafe { AssignProcessToJobObject(self.0.raw(), process) }
            .map_err(|error| format!("cannot contain WeChat OCR helper in job object: {error}"))
    }
}

/// A process or thread handle that is closed exactly once.
#[repr(transparent)]
pub(super) struct OwnedHandle(HANDLE);

impl OwnedHandle {
    pub(super) fn new(handle: HANDLE) -> Self {
        Self(handle)
    }

    pub(super) fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    #[inline(always)]
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;

        // SAFETY: this wrapper is created only from an owned successful handle
        // and Drop is its sole close path.
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            crate::report_error!("windows-native", "CloseHandle failed: {error}");
        }
    }
}
