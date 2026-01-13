//! Display mode definitions.

use crate::controller::AsusController;
use crate::error::ControllerError;
use crate::state::ControllerState;

/// Trait for display mode implementations.
///
/// Each display mode knows how to apply itself to the ASUS controller.
pub trait DisplayMode: std::fmt::Debug + Send + Sync {
    /// Apply this mode using the controller.
    fn apply(&self, controller: &AsusController) -> Result<(), ControllerError>;

    /// Get the RPC symbol name for setting this mode.
    fn symbol(&self) -> &'static [u8];

    /// Whether this is an e-reading/monochrome mode.
    fn is_ereading(&self) -> bool {
        false
    }

    /// Get the mode ID for this mode (used for state tracking).
    fn mode_id(&self) -> i32;
}

// =============================================================================
// Normal Mode
// =============================================================================

/// Normal display mode - default color profile.
#[derive(Debug, Clone, Copy)]
pub struct NormalMode;

impl NormalMode {
    /// Create a new Normal mode.
    pub fn new() -> Self {
        Self
    }
}

impl Default for NormalMode {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayMode for NormalMode {
    fn apply(&self, controller: &AsusController) -> Result<(), ControllerError> {
        controller.set_splendid_mode(b"MyOptSetSplendidFunc", 1)
    }

    fn symbol(&self) -> &'static [u8] {
        b"MyOptSetSplendidFunc"
    }

    fn mode_id(&self) -> i32 {
        1
    }
}

// =============================================================================
// Vivid Mode
// =============================================================================

/// Vivid display mode - enhanced colors.
#[derive(Debug, Clone, Copy)]
pub struct VividMode;

impl VividMode {
    /// Create a new Vivid mode.
    pub fn new() -> Self {
        Self
    }
}

impl Default for VividMode {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayMode for VividMode {
    fn apply(&self, controller: &AsusController) -> Result<(), ControllerError> {
        controller.set_splendid_mode(b"MyOptSetSplendidFunc", 2)
    }

    fn symbol(&self) -> &'static [u8] {
        b"MyOptSetSplendidFunc"
    }

    fn mode_id(&self) -> i32 {
        2
    }
}

// =============================================================================
// Manual Mode
// =============================================================================

/// Manual display mode - user-adjustable color temperature.
#[derive(Debug, Clone, Copy)]
pub struct ManualMode {
    /// Color temperature value (0-100).
    pub value: u8,
}

impl ManualMode {
    /// Create a new Manual mode with the specified value.
    ///
    /// # Errors
    /// Returns an error if value > 100.
    pub fn new(value: u8) -> Result<Self, ControllerError> {
        if value > 100 {
            return Err(ControllerError::InvalidSliderValue {
                mode: "Manual",
                value,
                min: 0,
                max: 100,
            });
        }
        Ok(Self { value })
    }

    /// Create from a controller state snapshot.
    pub fn from_controller_state(state: &ControllerState) -> Self {
        Self {
            value: state.manual_slider,
        }
    }
}

impl DisplayMode for ManualMode {
    fn apply(&self, controller: &AsusController) -> Result<(), ControllerError> {
        controller.set_splendid_mode(b"MyOptSetSplendidManualFunc", self.value)
    }

    fn symbol(&self) -> &'static [u8] {
        b"MyOptSetSplendidManualFunc"
    }

    fn mode_id(&self) -> i32 {
        6
    }
}

// =============================================================================
// Eye Care Mode
// =============================================================================

/// Eye Care display mode - reduces blue light.
#[derive(Debug, Clone, Copy)]
pub struct EyeCareMode {
    /// Blue light filter level (0-4).
    pub level: u8,
}

impl EyeCareMode {
    /// Create a new Eye Care mode with the specified level.
    ///
    /// # Errors
    /// Returns an error if level > 4.
    pub fn new(level: u8) -> Result<Self, ControllerError> {
        if level > 4 {
            return Err(ControllerError::InvalidSliderValue {
                mode: "EyeCare",
                value: level,
                min: 0,
                max: 4,
            });
        }
        Ok(Self { level })
    }

    /// Create from a controller state snapshot.
    pub fn from_controller_state(state: &ControllerState) -> Self {
        Self {
            level: state.eyecare_level,
        }
    }
}

impl DisplayMode for EyeCareMode {
    fn apply(&self, controller: &AsusController) -> Result<(), ControllerError> {
        controller.set_splendid_mode(b"MyOptSetSplendidEyecareFunc", self.level)
    }

    fn symbol(&self) -> &'static [u8] {
        b"MyOptSetSplendidEyecareFunc"
    }

    fn mode_id(&self) -> i32 {
        7
    }
}

// =============================================================================
// E-Reading Mode
// =============================================================================

/// E-Reading display mode - grayscale with adjustable temperature.
#[derive(Debug, Clone, Copy)]
pub struct EReadingMode {
    /// Grayscale level (1-5).
    pub grayscale: u8,
    /// Temperature value.
    pub temp: u8,
}

impl EReadingMode {
    /// Create a new E-Reading mode.
    ///
    /// # Arguments
    /// * `grayscale` - Grayscale level (1-5)
    /// * `temp` - Temperature value
    ///
    /// # Errors
    /// Returns an error if grayscale is not in range 1-5.
    pub fn new(grayscale: u8, temp: u8) -> Result<Self, ControllerError> {
        if grayscale < 1 || grayscale > 5 {
            return Err(ControllerError::InvalidSliderValue {
                mode: "EReading grayscale",
                value: grayscale,
                min: 1,
                max: 5,
            });
        }
        Ok(Self { grayscale, temp })
    }

    /// Create from a controller state snapshot.
    pub fn from_controller_state(state: &ControllerState) -> Self {
        Self {
            grayscale: state.ereading_grayscale,
            temp: state.ereading_temp,
        }
    }
}

impl DisplayMode for EReadingMode {
    fn apply(&self, controller: &AsusController) -> Result<(), ControllerError> {
        // Convert from user-facing 1-5 to hardware 0-4
        controller.set_monochrome_mode(self.grayscale - 1, self.temp)
    }

    fn symbol(&self) -> &'static [u8] {
        b"MyOptSetSplendidMonochromeFunc"
    }

    fn is_ereading(&self) -> bool {
        true
    }

    fn mode_id(&self) -> i32 {
        -1 // Special case - e-reading doesn't have a single mode ID
    }
}
