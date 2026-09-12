//! Thread-owned Core Audio controller. Audio callbacks remain inside the bridge.

use crate::api::audio::AudioAction;
use std::ffi::{CStr, c_char, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;

unsafe extern "C" {
    fn KSAudioProcessIdentity(pid: i32) -> u64;
    fn KSMaintainAudio(controller: *mut c_void) -> bool;
    fn KSCreateAudioController(log: extern "C" fn(*const c_char)) -> *mut c_void;
    fn KSDestroyAudioController(controller: *mut c_void);
    fn KSChangeAudio(
        controller: *mut c_void,
        pid: i32,
        started: u64,
        action: u32,
        message: *mut c_char,
        capacity: usize,
    ) -> bool;
}

extern "C" fn report_audio_error(message: *const c_char) {
    if message.is_null() {
        return;
    }
    // SAFETY: the bridge supplies a NUL-terminated diagnostic valid for this
    // synchronous call on the controller owner thread; no pointer is retained.
    let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    crate::report_error!("audio", "{message}");
}

#[derive(Default)]
pub(super) struct AudioController {
    native: Option<NonNull<c_void>>,
    thread: PhantomData<Rc<()>>,
}
impl AudioController {
    pub(super) fn maintain(&mut self) -> bool {
        let Some(native) = self.native else {
            return false;
        };
        // SAFETY: maintenance runs on the controller's owner thread, outside
        // real-time callbacks; the bridge releases terminated application taps.
        unsafe { KSMaintainAudio(native.as_ptr()) }
    }

    pub(super) fn change(
        &mut self,
        process: Option<crate::platform::common::audio_worker::AudioProcess>,
        change: AudioAction,
    ) -> Result<String, String> {
        let mut message = [0_u8; 1024];
        let action = match change {
            AudioAction::Down => 0,
            AudioAction::Up => 1,
            AudioAction::ToggleMute => 2,
            AudioAction::DevicePrevious => 3,
            AudioAction::DeviceNext => 4,
        };
        // SAFETY: this !Send owner is only used on the audio worker. The bridge
        // retains its controller; the bounded buffer lives through the call.
        let ok = unsafe {
            if self.native.is_none() {
                self.native = NonNull::new(KSCreateAudioController(report_audio_error));
            }
            let native = self.native.ok_or("Cannot create native audio controller")?;
            KSChangeAudio(
                native.as_ptr(),
                process.map_or(0, |p| p.pid as i32),
                process.map_or(0, |p| p.started),
                action,
                message.as_mut_ptr().cast(),
                message.len(),
            )
        };
        let end = message
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(message.len());
        let message = String::from_utf8_lossy(&message[..end]).into_owned();
        if ok { Ok(message) } else { Err(message) }
    }
}
impl Drop for AudioController {
    fn drop(&mut self) {
        // SAFETY: exactly one owner releases this controller on its creating
        // worker, after the bridge stops callbacks and restores process output.
        if let Some(native) = self.native.take() {
            // SAFETY: take consumes the unique non-null owner; destruction runs
            // once on the creating worker, after native callbacks are stopped.
            unsafe {
                KSDestroyAudioController(native.as_ptr());
            }
        }
    }
}

pub(super) fn process(
    pid: i32,
) -> Result<crate::platform::common::audio_worker::AudioProcess, String> {
    // SAFETY: scalar PID input and scalar creation-identity output; no pointers
    // or Objective-C objects leave the calling worker.
    let started = unsafe { KSAudioProcessIdentity(pid) };
    if started == 0 {
        return Err("Application has exited or has no creation identity".into());
    }
    Ok(crate::platform::common::audio_worker::AudioProcess {
        pid: pid as u32,
        started,
    })
}
pub(super) fn create_backend() -> Box<dyn crate::platform::common::audio_worker::AudioBackend> {
    Box::new(AudioController::default())
}
impl crate::platform::common::audio_worker::AudioBackend for AudioController {
    fn execute(
        &mut self,
        process: Option<crate::platform::common::audio_worker::AudioProcess>,
        action: AudioAction,
    ) -> Result<String, String> {
        self.change(process, action)
    }
    fn maintain(&mut self) -> bool {
        AudioController::maintain(self)
    }
}
