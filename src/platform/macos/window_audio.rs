//! Thread-owned Core Audio controller. Audio callbacks remain inside the bridge.
use crate::api::audio::AudioAction;
use std::cell::Cell;
use std::ffi::{CStr, c_char, c_void};
use std::marker::PhantomData;
use std::rc::Rc;

unsafe extern "C" {
    fn KSMaintainAudio(controller: *mut c_void) -> bool;
    fn KSCreateAudioController(log: extern "C" fn(*const c_char)) -> *mut c_void;
    fn KSDestroyAudioController(controller: *mut c_void);
    fn KSChangeAudio(
        controller: *mut c_void,
        pid: i32,
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
    native: Cell<*mut c_void>,
    thread: PhantomData<Rc<()>>,
}
impl AudioController {
    pub(super) fn maintain(&self) -> bool {
        if self.native.get().is_null() {
            return false;
        }
        // SAFETY: maintenance runs on the controller's owner thread, outside
        // real-time callbacks; the bridge releases terminated application taps.
        unsafe { KSMaintainAudio(self.native.get()) }
    }

    pub(super) fn change(&self, pid: Option<i32>, change: AudioAction) -> Result<String, String> {
        let mut message = [0_u8; 1024];
        let action = match change {
            AudioAction::Down => 0,
            AudioAction::Up => 1,
            AudioAction::ToggleMute => 2,
            AudioAction::DevicePrevious => 3,
            AudioAction::DeviceNext => 4,
        };
        // SAFETY: this !Send owner is only used on the window worker. The bridge
        // retains its controller; the bounded buffer lives through the call.
        let ok = unsafe {
            if self.native.get().is_null() {
                self.native.set(KSCreateAudioController(report_audio_error));
            }
            KSChangeAudio(
                self.native.get(),
                pid.unwrap_or(0),
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
        unsafe {
            KSDestroyAudioController(self.native.get());
        }
    }
}
