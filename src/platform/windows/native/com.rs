//! Thread-bound COM apartment lifetime.
use std::{marker::PhantomData, rc::Rc};

pub(crate) struct ComApartment(PhantomData<Rc<()>>);

impl ComApartment {
    pub(crate) fn initialise() -> Result<Self, String> {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

        // SAFETY: the returned !Send guard binds the successful COM apartment
        // initialization to this thread and balances it in Drop.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|error| format!("cannot initialize COM apartment: {error}"))?;
        Ok(Self(PhantomData))
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        use windows::Win32::System::Com::CoUninitialize;

        // SAFETY: this !Send guard is dropped on the same thread that
        // successfully initialized the apartment.
        unsafe { CoUninitialize() };
    }
}
