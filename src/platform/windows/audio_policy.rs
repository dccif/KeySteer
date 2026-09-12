//! Versioned private Windows audio-policy ABI. Only explicit user shortcuts
//! call setters. QueryInterface/activation failures propagate without fallback
//! to another audio scope. Layouts verified against EarTrumpet's MIT-licensed
//! Interop/MMDeviceAPI/{IPolicyConfig,IAudioPolicyConfigFactoryVariantFor21H2,
//! IAudioPolicyConfigFactoryVariantForDownlevel}.cs declarations.
use std::ffi::c_void;
use windows::Win32::Media::Audio::{EDataFlow, ERole, eRender};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::System::WinRT::RoGetActivationFactory;
use windows::core::{GUID, HRESULT, HSTRING, IInspectable_Vtbl, IUnknown_Vtbl, Interface, PCWSTR};

windows::core::define_interface!(Policy, PolicyVtbl, 0xf8679f50_850a_41cf_9c72_430f290290c8);
#[repr(C)]
pub struct PolicyVtbl {
    base: IUnknown_Vtbl,
    unused: [usize; 10],
    set_default: unsafe extern "system" fn(*mut c_void, PCWSTR, ERole) -> HRESULT,
}

windows::core::define_interface!(Modern, FactoryVtbl, 0xab3d4648_e242_459f_b02f_541c70306324);
windows::core::define_interface!(Legacy, FactoryVtbl, 0x2a59116d_6c4f_45e0_a74f_707e3fef9258);
#[repr(C)]
pub struct FactoryVtbl {
    base: IInspectable_Vtbl,
    unused: [usize; 19],
    set: unsafe extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut c_void) -> HRESULT,
    get: unsafe extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut *mut c_void) -> HRESULT,
}

pub(super) fn set_system(id: &str, role: ERole) -> windows::core::Result<()> {
    let id: Vec<_> = id.encode_utf16().chain([0]).collect();
    // SAFETY: activation validates the exact interface IID and vtable layout;
    // the terminated ID and interface outlive the synchronous call.
    unsafe {
        let policy: Policy = CoCreateInstance(
            &GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9),
            None,
            CLSCTX_ALL,
        )?;
        (policy.vtable().set_default)(policy.as_raw(), PCWSTR(id.as_ptr()), role).ok()
    }
}

pub(super) enum Factory {
    Modern(Modern),
    Legacy(Legacy),
}
impl Factory {
    pub(super) fn new() -> windows::core::Result<Self> {
        let class = HSTRING::from("Windows.Media.Internal.AudioPolicyConfig");
        // SAFETY: exact modern/legacy IIDs share the documented-above slot
        // layout. Each returned interface owns its COM reference on this thread.
        unsafe {
            RoGetActivationFactory::<Modern>(&class)
                .map(Self::Modern)
                .or_else(|_| RoGetActivationFactory::<Legacy>(&class).map(Self::Legacy))
        }
    }
    fn parts(&self) -> (*mut c_void, &FactoryVtbl) {
        match self {
            Self::Modern(v) => (v.as_raw(), v.vtable()),
            Self::Legacy(v) => (v.as_raw(), v.vtable()),
        }
    }
    pub(super) fn get(&self, pid: u32, role: ERole) -> windows::core::Result<HSTRING> {
        let (this, vtable) = self.parts();
        let mut value = HSTRING::new();
        // SAFETY: the output is a transferred HSTRING owned by the caller;
        // HSTRING is repr(transparent) over its handle. Write into an empty
        // owner so even a partially returned allocation is freed on failure.
        unsafe {
            let result = (vtable.get)(
                this,
                pid,
                eRender,
                role,
                std::ptr::from_mut(&mut value).cast(),
            );
            result.ok()?;
            Ok(value)
        }
    }
    pub(super) fn set(&self, pid: u32, role: ERole, value: &HSTRING) -> windows::core::Result<()> {
        let (this, vtable) = self.parts();
        // SAFETY: the borrowed HSTRING stays alive throughout this synchronous
        // call. Read the repr(transparent) handle without transferring ownership;
        // a null/empty HSTRING resets the application to system default.
        unsafe {
            (vtable.set)(
                this,
                pid,
                eRender,
                role,
                *std::ptr::from_ref(value).cast::<*mut c_void>(),
            )
            .ok()
        }
    }
}

pub(super) fn device_path(id: &str) -> HSTRING {
    HSTRING::from(format!(
        r"\\?\SWD#MMDEVAPI#{id}#{{e6327cad-dcec-4949-ae8a-991e976a79d2}}"
    ))
}

pub(super) fn endpoint_id(path: &HSTRING) -> String {
    let text = path.to_string_lossy();
    let id = text.strip_prefix(r"\\?\SWD#MMDEVAPI#").unwrap_or(&text);
    id.strip_suffix("#{e6327cad-dcec-4949-ae8a-991e976a79d2}")
        .unwrap_or(id)
        .to_string()
}
