//! Reusable desktop capture owns its DC and pixel surface.
#![forbid(unsafe_code)]
use super::gdi::ScreenDc;
use super::{GdiDibSurface, NativeDimensions};

#[must_use = "prepared capture resources must remain on their acquiring thread"]
pub(crate) struct PreparedCapture {
    screen: ScreenDc,
    surface: GdiDibSurface,
}

impl PreparedCapture {
    pub(crate) fn new(width: u32, height: u32) -> Result<Self, String> {
        let dimensions = NativeDimensions::from_usize(width as usize, height as usize)?;
        let screen = ScreenDc::acquire()?;
        let surface = GdiDibSurface::new(Some(screen.raw()), dimensions)?;
        Ok(Self { screen, surface })
    }

    pub(crate) fn capture_with<R>(
        &mut self,
        source_x: i32,
        source_y: i32,
        source_width: i32,
        source_height: i32,
        consume: impl FnOnce(&[u8], u32, u32) -> Result<R, String>,
    ) -> Result<R, String> {
        self.surface.copy_from(
            self.screen.raw(),
            source_x,
            source_y,
            source_width,
            source_height,
            consume,
        )
    }
}
