//! Checked native dimensions and slice lengths.
#![forbid(unsafe_code)]

/// Dimensions that are representable by Win32 APIs and by a Rust byte slice.
///
/// Construction performs every narrowing conversion and length calculation so
/// native allocation sizes cannot diverge from the slices exposed to Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeDimensions {
    width: i32,
    height: i32,
    byte_len: usize,
}

impl NativeDimensions {
    pub(crate) fn from_usize(width: usize, height: usize) -> Result<Self, String> {
        let width_i32 =
            i32::try_from(width).map_err(|_| format!("native width {width} exceeds i32::MAX"))?;
        let height_i32 = i32::try_from(height)
            .map_err(|_| format!("native height {height} exceeds i32::MAX"))?;
        if width_i32 == 0 || height_i32 == 0 {
            return Err("native dimensions must be positive".into());
        }
        let byte_len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .filter(|length| *length <= isize::MAX as usize)
            .ok_or_else(|| format!("native BGRA surface {width}x{height} is too large"))?;
        Ok(Self {
            width: width_i32,
            height: height_i32,
            byte_len,
        })
    }

    pub(crate) fn from_f64(width: f64, height: f64) -> Result<Self, String> {
        fn rounded(value: f64, name: &str) -> Result<usize, String> {
            if !value.is_finite() || value <= 0.0 || value.round() > i32::MAX as f64 {
                return Err(format!("invalid native {name} {value}"));
            }
            Ok(value.round().max(1.0) as usize)
        }

        Self::from_usize(rounded(width, "width")?, rounded(height, "height")?)
    }

    pub(crate) const fn width_i32(self) -> i32 {
        self.width
    }

    pub(crate) const fn height_i32(self) -> i32 {
        self.height
    }

    pub(crate) const fn width_u32(self) -> u32 {
        self.width as u32
    }

    pub(crate) const fn height_u32(self) -> u32 {
        self.height as u32
    }

    pub(crate) const fn byte_len(self) -> usize {
        self.byte_len
    }
}

#[cfg(test)]
mod native_dimension_tests {
    use super::NativeDimensions;

    #[test]
    fn native_dimensions_reject_unrepresentable_surfaces() {
        assert!(NativeDimensions::from_usize(0, 1).is_err());
        assert!(NativeDimensions::from_usize(1, 0).is_err());
        assert!(NativeDimensions::from_usize(i32::MAX as usize + 1, 1).is_err());
        assert!(NativeDimensions::from_usize(i32::MAX as usize, i32::MAX as usize).is_err());
        assert!(NativeDimensions::from_f64(f64::NAN, 1.0).is_err());
        assert!(NativeDimensions::from_f64(1.0, f64::INFINITY).is_err());
    }

    #[test]
    fn native_dimensions_preserve_the_validated_byte_length() {
        let dimensions = NativeDimensions::from_usize(3840, 2160).unwrap();
        assert_eq!(dimensions.width_i32(), 3840);
        assert_eq!(dimensions.height_i32(), 2160);
        assert_eq!(dimensions.byte_len(), 3840 * 2160 * 4);
    }
}
