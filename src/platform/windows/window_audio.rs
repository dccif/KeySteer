//! Application audio sessions owned by the dedicated audio worker.

use crate::api::audio::AudioAction;
use std::collections::BTreeSet;
use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2, IMMDevice,
    IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, eCommunications, eConsole,
    eMultimedia, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};

use windows::core::Interface;

fn application_pids(root: u32, processes: &[(u32, u32, String)]) -> BTreeSet<u32> {
    let mut pids = BTreeSet::from([root]);
    let Some((_, _, executable)) = processes.iter().find(|(pid, _, _)| *pid == root) else {
        return pids;
    };
    loop {
        let before = pids.len();
        for (pid, parent, name) in processes {
            // Browser audio services use child processes. Restrict descendants
            // to the same executable so selecting a shell cannot mute its apps.
            if pids.contains(parent) && name.eq_ignore_ascii_case(executable) {
                pids.insert(*pid);
            }
        }
        if before == pids.len() {
            return pids;
        }
    }
}

fn level(current: f32, change: AudioAction) -> f32 {
    (current
        + if change == AudioAction::Up {
            0.01
        } else {
            -0.01
        })
    .clamp(0.0, 1.0)
}

pub(super) fn change(pid: u32, change: AudioAction) -> Result<String, String> {
    let _apartment = super::native::ComApartment::initialise()?;
    let pids = application_pids(
        pid,
        &super::native::audio_process_list()
            .map_err(|e| format!("Cannot inspect application processes: {e}"))?,
    );
    if matches!(
        change,
        AudioAction::DevicePrevious | AudioAction::DeviceNext
    ) {
        return switch_application(&pids, pid, change);
    }
    // SAFETY: COM interfaces are acquired, used and released on this apartment's
    // worker thread. Enumeration indices come from the native counts; only
    // matching non-system sessions are changed, scalar levels are clamped, and
    // null event-context pointers are explicitly allowed by Core Audio.
    unsafe {
        let collect = || -> windows::core::Result<Vec<(ISimpleAudioVolume, f32, bool)>> {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            let mut matched = Vec::new();
            for index in 0..devices.GetCount()? {
                let device = devices.Item(index)?;
                let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
                let sessions = manager.GetSessionEnumerator()?;
                for index in 0..sessions.GetCount()? {
                    let Ok(session) = sessions.GetSession(index) else {
                        continue;
                    };
                    let Ok(control) = session.cast::<IAudioSessionControl2>() else {
                        continue;
                    };
                    let Ok(owner) = control.GetProcessId() else {
                        continue;
                    };
                    if owner == 0 || !pids.contains(&owner) {
                        continue;
                    }
                    let volume = session.cast::<ISimpleAudioVolume>()?;
                    let current = volume.GetMasterVolume()?;
                    let muted = volume.GetMute()?.as_bool();
                    matched.push((volume, current, muted));
                }
            }
            Ok(matched)
        };
        let sessions = collect().map_err(|e| format!("Cannot read application audio: {e}"))?;
        if sessions.is_empty() {
            return Err("This application has no audio session yet".into());
        }
        let mute = sessions.iter().any(|(_, _, muted)| !muted);
        let mut maximum = 0.0_f32;
        for (applied, (volume, current, _)) in sessions.iter().enumerate() {
            let value = level(*current, change);
            let outcome = if change == AudioAction::ToggleMute {
                volume.SetMute(mute, std::ptr::null())
            } else {
                volume.SetMasterVolume(value, std::ptr::null())
            };
            outcome.map_err(|e| {
                format!(
                    "Application audio updated {applied}/{} sessions: {e}",
                    sessions.len()
                )
            })?;
            maximum = maximum.max(value);
        }
        Ok(if change == AudioAction::ToggleMute {
            if mute {
                "App muted".into()
            } else {
                "App unmuted".into()
            }
        } else {
            format!(
                "App volume {:.0}%{}",
                maximum * 100.0,
                if sessions.iter().all(|(_, _, muted)| *muted) {
                    " · muted"
                } else {
                    ""
                }
            )
        })
    }
}

struct Output {
    id: String,
    name: String,
}

fn device_id(device: &IMMDevice) -> windows::core::Result<String> {
    // SAFETY: GetId transfers a COM task allocation. Convert and free it on
    // both successful and unsuccessful UTF-16 conversion paths.
    unsafe {
        let value = device.GetId()?;
        let text = value.to_string();
        windows::Win32::System::Com::CoTaskMemFree(Some(value.0.cast()));
        Ok(text?)
    }
}

fn device_name(device: &IMMDevice) -> windows::core::Result<String> {
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::StructuredStorage::{
        PropVariantClear, PropVariantToStringAlloc,
    };
    use windows::Win32::System::Com::{CoTaskMemFree, STGM_READ};
    // SAFETY: read-only property store, exact PKEY_Device_FriendlyName. Both
    // native output allocations are released before propagating any error.
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ)?;
        let key = PROPERTYKEY {
            fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
            pid: 14,
        };
        let mut value = store.GetValue(&key)?;
        let text = PropVariantToStringAlloc(&value);
        if let Err(error) = PropVariantClear(&mut value) {
            crate::report_error!("audio", "cannot release output name property: {error}");
        }
        let text = text?;
        let result = text.to_string();
        CoTaskMemFree(Some(text.0.cast()));
        Ok(result?)
    }
}

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    // SAFETY: callers own the worker-thread COM apartment and retain the
    // returned interface only inside that apartment's lifetime.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

fn outputs(enumerator: &IMMDeviceEnumerator) -> windows::core::Result<Vec<Output>> {
    // SAFETY: enumerate active render endpoints with native bounded indices;
    // devices remain reference-counted while their properties are read.
    unsafe {
        let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let mut result = Vec::new();
        for index in 0..devices.GetCount()? {
            let device = devices.Item(index)?;
            let id = device_id(&device)?;
            let name = device_name(&device).unwrap_or_else(|_| id.clone());
            result.push(Output { id, name });
        }
        result.sort_by_cached_key(|output| (output.name.to_lowercase(), output.id.clone()));
        Ok(result)
    }
}

fn next_output<'a>(
    outputs: &'a [Output],
    current: &str,
    previous: bool,
) -> Result<&'a Output, String> {
    if outputs.is_empty() {
        return Err("No active audio output devices".into());
    }
    let index = outputs
        .iter()
        .position(|output| output.id.eq_ignore_ascii_case(current));
    let next = match index {
        Some(index) if previous => (index + outputs.len() - 1) % outputs.len(),
        Some(index) => (index + 1) % outputs.len(),
        None if previous => outputs.len() - 1,
        None => 0,
    };
    Ok(&outputs[next])
}

fn default_id(
    enumerator: &IMMDeviceEnumerator,
    role: windows::Win32::Media::Audio::ERole,
) -> windows::core::Result<String> {
    // SAFETY: the enumerator is owned on the current COM thread; the returned
    // reference is released after copying its task-allocated ID.
    unsafe { device_id(&enumerator.GetDefaultAudioEndpoint(eRender, role)?) }
}

fn switch_application(
    pids: &BTreeSet<u32>,
    root: u32,
    change: AudioAction,
) -> Result<String, String> {
    let apply = || -> Result<String, String> {
        let factory = super::audio_policy::Factory::new().map_err(|e| e.to_string())?;
        let enumerator = enumerator().map_err(|e| e.to_string())?;
        let outputs = outputs(&enumerator).map_err(|e| e.to_string())?;
        let mut saved = Vec::new();
        for pid in pids {
            // Browser window/renderer processes may have no audio identity.
            // Windows returns E_INVALIDARG for them; keep the actual audio
            // service's policy instead of letting a silent parent block it.
            let console = match factory.get(*pid, eConsole) {
                Ok(value) => value,
                Err(error) if error.code() == windows::Win32::Foundation::E_INVALIDARG => continue,
                Err(error) => return Err(error.to_string()),
            };
            saved.push((*pid, eConsole, console));
            for role in [eMultimedia, eCommunications] {
                saved.push((
                    *pid,
                    role,
                    factory.get(*pid, role).map_err(|e| e.to_string())?,
                ));
            }
        }
        let (_, _, current) = saved
            .iter()
            .find(|(pid, role, _)| *pid == root && *role == eMultimedia)
            .or_else(|| saved.iter().find(|(_, role, _)| *role == eMultimedia))
            .ok_or("This application has no audio session yet")?;
        let current = if current.is_empty() {
            default_id(&enumerator, eMultimedia).map_err(|e| e.to_string())?
        } else {
            super::audio_policy::endpoint_id(current)
        };
        let output = next_output(&outputs, &current, change == AudioAction::DevicePrevious)?;
        let path = super::audio_policy::device_path(&output.id);
        for (index, (pid, role, _)) in saved.iter().enumerate() {
            if let Err(error) = factory.set(*pid, *role, &path) {
                let mut restored = true;
                for (pid, role, old) in &saved[..=index] {
                    restored &= factory.set(*pid, *role, old).is_ok();
                }
                return Err(format!(
                    "{error}; previous routing {}",
                    if restored {
                        "restored"
                    } else {
                        "could not be fully restored"
                    }
                ));
            }
        }
        Ok(format!("App output: {}", output.name))
    };
    apply().map_err(|error| format!("Cannot switch application output: {error}"))
}

pub(super) fn system(change: AudioAction) -> Result<String, String> {
    let _apartment = super::native::ComApartment::initialise()?;
    let enumerator = enumerator().map_err(|e| format!("Cannot read system audio: {e}"))?;
    if matches!(
        change,
        AudioAction::DevicePrevious | AudioAction::DeviceNext
    ) {
        let outputs = outputs(&enumerator).map_err(|e| e.to_string())?;
        let saved = [eConsole, eMultimedia, eCommunications]
            .into_iter()
            .map(|role| default_id(&enumerator, role).map(|id| (role, id)))
            .collect::<windows::core::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        let output = next_output(&outputs, &saved[1].1, change == AudioAction::DevicePrevious)?;
        for (index, (role, _)) in saved.iter().enumerate() {
            if let Err(error) = super::audio_policy::set_system(&output.id, *role) {
                let mut restored = true;
                for (role, id) in &saved[..=index] {
                    restored &= super::audio_policy::set_system(id, *role).is_ok();
                }
                return Err(format!(
                    "Cannot switch system output: {error}; previous devices {}",
                    if restored {
                        "restored"
                    } else {
                        "could not be fully restored"
                    }
                ));
            }
        }
        return Ok(format!("System output: {}", output.name));
    }
    // SAFETY: only the user's current default render endpoint is changed.
    // The endpoint interface is worker-owned and scalar levels are clamped.
    unsafe {
        use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
        let apply = || -> windows::core::Result<String> {
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
            let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            if change == AudioAction::ToggleMute {
                let muted = !volume.GetMute()?.as_bool();
                volume.SetMute(muted, std::ptr::null())?;
                return Ok(if muted {
                    "System muted"
                } else {
                    "System unmuted"
                }
                .into());
            }
            let value = level(volume.GetMasterVolumeLevelScalar()?, change);
            volume.SetMasterVolumeLevelScalar(value, std::ptr::null())?;
            Ok(format!("System volume {:.0}%", value * 100.0))
        };
        apply().map_err(|e| format!("Cannot change system volume: {e}"))
    }
}

pub(super) fn process(
    pid: u32,
) -> Result<crate::platform::common::audio_worker::AudioProcess, String> {
    super::native::with_process_identity(pid, |started| {
        Ok(crate::platform::common::audio_worker::AudioProcess { pid, started })
    })
}
pub(super) fn create_backend() -> Box<dyn crate::platform::common::audio_worker::AudioBackend> {
    Box::new(WindowsAudio)
}
struct WindowsAudio;
impl crate::platform::common::audio_worker::AudioBackend for WindowsAudio {
    fn execute(
        &mut self,
        process: Option<crate::platform::common::audio_worker::AudioProcess>,
        action: AudioAction,
    ) -> Result<String, String> {
        match process {
            None => system(action),
            Some(process) => super::native::with_process_identity(process.pid, |started| {
                if started != process.started {
                    return Err("Application audio target has expired".into());
                }
                change(process.pid, action)
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn process_snapshot_retains_current_process_identity() -> windows::core::Result<()> {
        let processes = super::super::native::audio_process_list()?;
        assert!(
            processes
                .iter()
                .any(|(pid, _, name)| *pid == std::process::id() && !name.is_empty())
        );
        Ok(())
    }

    use super::*;
    #[test]
    fn output_cycle_wraps_both_directions_and_handles_removed_devices() {
        let outputs = ["a", "b", "c"].map(|id| Output {
            id: id.into(),
            name: id.into(),
        });
        assert_eq!(next_output(&outputs, "c", false).unwrap().id, "a");
        assert_eq!(next_output(&outputs, "a", true).unwrap().id, "c");
        assert_eq!(next_output(&outputs, "removed", false).unwrap().id, "a");
        assert_eq!(next_output(&outputs, "removed", true).unwrap().id, "c");
        assert!(next_output(&[], "a", false).is_err());
        assert_eq!(next_output(&outputs[..1], "a", true).unwrap().id, "a");
    }

    #[test]
    #[ignore = "requires native audio; writes only existing system defaults and this test process's routing, then restores routing"]
    fn native_audio_policy_routing_and_system_interfaces() -> Result<(), Box<dyn std::error::Error>>
    {
        use super::super::audio_policy;
        use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
        let _apartment = super::super::native::ComApartment::initialise()?;
        let enumerator = enumerator()?;
        let outputs = outputs(&enumerator)?;
        assert!(!outputs.is_empty());
        // SAFETY: this test owns an unstarted silent audio client. Free the
        // COM format allocation immediately after the initialization call.
        let _client = unsafe {
            use windows::Win32::Media::Audio::{AUDCLNT_SHAREMODE_SHARED, IAudioClient};
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let format = client.GetMixFormat()?;
            let initialized =
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, format, None);
            windows::Win32::System::Com::CoTaskMemFree(Some(format.cast()));
            initialized?;
            client
        };
        let factory = audio_policy::Factory::new().map_err(|e| format!("factory: {e}"))?;
        let pid = std::process::id();
        for role in [eConsole, eMultimedia, eCommunications] {
            let id = default_id(&enumerator, role)?;
            audio_policy::set_system(&id, role)
                .map_err(|e| format!("system role {role:?}: {e}"))?;
            assert_eq!(default_id(&enumerator, role)?, id);
            let old = factory
                .get(pid, role)
                .map_err(|e| format!("get role {role:?}: {e}"))?;
            let set = factory.set(pid, role, &audio_policy::device_path(&id));
            let read = factory.get(pid, role);
            let restore = factory.set(pid, role, &old);
            restore.map_err(|e| format!("restore role {role:?}: {e}"))?;
            set.map_err(|e| format!("set role {role:?}: {e}"))?;
            assert_eq!(audio_policy::endpoint_id(&read?), id);
            assert_eq!(factory.get(pid, role)?, old);
        }
        let saved = [eConsole, eMultimedia, eCommunications]
            .into_iter()
            .map(|role| factory.get(pid, role).map(|value| (role, value)))
            .collect::<windows::core::Result<Vec<_>>>()?;
        let result = switch_application(
            &BTreeSet::from([pid, u32::MAX]),
            u32::MAX,
            AudioAction::DeviceNext,
        );
        let selected = factory.get(pid, eMultimedia);
        for (role, old) in &saved {
            factory.set(pid, *role, old)?;
        }
        assert!(result?.starts_with("App output: "));
        let current = default_id(&enumerator, eMultimedia)?;
        assert_eq!(
            audio_policy::endpoint_id(&selected?),
            next_output(&outputs, &current, false)?.id
        );
        // SAFETY: read the default output's volume interface without changing
        // live playback; all COM references are local to this apartment.
        unsafe {
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
            let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            assert!((0.0..=1.0).contains(&volume.GetMasterVolumeLevelScalar()?));
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires an audio output; creates and changes only this test process's silent session"]
    fn native_application_audio_session_volume_and_mute() -> Result<(), Box<dyn std::error::Error>>
    {
        use windows::Win32::Media::Audio::{AUDCLNT_SHAREMODE_SHARED, IAudioClient, eMultimedia};
        use windows::Win32::System::Com::CoTaskMemFree;
        let _apartment = super::super::native::ComApartment::initialise()?;
        // SAFETY: the test owns this silent, unstarted client and session. The
        // COM-allocated mix format is freed immediately after initialization;
        // interfaces drop before the apartment, including on test failure.
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let format = client.GetMixFormat()?;
            let guid = windows::core::GUID::from_u128(0xb7ff166f_3a10_4169_b606_d240e4156abc);
            let initialized = client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                0,
                1_000_000,
                0,
                format,
                Some(&guid),
            );
            CoTaskMemFree(Some(format.cast()));
            initialized?;
            let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
            let volume = manager.GetSimpleAudioVolume(Some(&guid), 0)?;
            volume.SetMasterVolume(0.5, std::ptr::null())?;
            volume.SetMute(false, std::ptr::null())?;
            change(std::process::id(), AudioAction::Down)?;
            assert!((volume.GetMasterVolume()? - 0.49).abs() < 0.001);
            change(std::process::id(), AudioAction::Up)?;
            assert!((volume.GetMasterVolume()? - 0.5).abs() < 0.001);
            change(std::process::id(), AudioAction::ToggleMute)?;
            assert!(volume.GetMute()?.as_bool());
            change(std::process::id(), AudioAction::ToggleMute)?;
            assert!(!volume.GetMute()?.as_bool());
        }
        Ok(())
    }

    #[test]
    fn limits_audio_to_target_and_its_same_application_children() {
        let processes = [
            (1, 0, "shell"),
            (2, 1, "browser"),
            (3, 2, "browser"),
            (4, 3, "BROWSER"),
            (5, 2, "other"),
            (6, 0, "browser"),
        ]
        .map(|(p, parent, name)| (p, parent, name.into()));
        assert_eq!(application_pids(1, &processes), BTreeSet::from([1]));
        assert_eq!(application_pids(2, &processes), BTreeSet::from([2, 3, 4]));
        assert_eq!(application_pids(9, &processes), BTreeSet::from([9]));
    }
    #[test]
    fn volume_steps_are_bounded() {
        assert_eq!(level(0.995, AudioAction::Up), 1.0);
        assert_eq!(level(0.005, AudioAction::Down), 0.0);
        assert!((level(0.5, AudioAction::Down) - 0.49).abs() < 0.0001);
    }
}
