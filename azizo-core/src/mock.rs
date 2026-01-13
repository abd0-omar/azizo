//! Mock controller for testing.

use crate::controller::{AsusController, DisplayController};
use crate::error::ControllerError;
use crate::modes::{DisplayMode, EReadingMode, EyeCareMode, ManualMode, NormalMode, VividMode};
use crate::state::ControllerState;
use std::sync::Mutex;

/// A mock display controller for testing.
///
/// This allows testing code that depends on [`DisplayController`] without
/// requiring actual ASUS hardware or the ASUS DLL.
///
/// # Example
///
/// ```
/// use azizo_core::{MockController, DisplayController, NormalMode};
///
/// let mock = MockController::new();
/// mock.set_mode(&NormalMode::new()).unwrap();
/// assert_eq!(mock.get_state().mode_id, 1);
/// ```
pub struct MockController {
    state: Mutex<ControllerState>,
}

impl MockController {
    /// Create a new mock controller with default state.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ControllerState {
                mode_id: 1,
                is_monochrome: false,
                dimming: 70,
                manual_slider: 50,
                eyecare_level: 2,
                ereading_grayscale: 4,
                ereading_temp: 50,
                last_non_ereading_mode: 1,
            }),
        }
    }

    /// Create a mock controller with custom initial state.
    pub fn with_state(state: ControllerState) -> Self {
        Self {
            state: Mutex::new(state),
        }
    }
}

impl Default for MockController {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayController for MockController {
    fn get_state(&self) -> ControllerState {
        self.state.lock().unwrap().clone()
    }

    fn refresh_sliders(&self) -> Result<(), ControllerError> {
        Ok(())
    }

    fn sync_all_sliders(&self) -> Result<(), ControllerError> {
        Ok(())
    }

    fn set_dimming(&self, level: i32) -> Result<(), ControllerError> {
        self.state.lock().unwrap().dimming = level.clamp(40, 100);
        Ok(())
    }

    fn set_dimming_percent(&self, percent: i32) -> Result<(), ControllerError> {
        let splendid_value = AsusController::percent_to_dimming(percent.clamp(0, 100));
        self.set_dimming(splendid_value)
    }

    fn get_current_mode(&self) -> Result<Box<dyn DisplayMode>, ControllerError> {
        let state = self.get_state();
        match (state.mode_id, state.is_monochrome) {
            (1, false) => Ok(Box::new(NormalMode::new())),
            (2, false) => Ok(Box::new(VividMode::new())),
            (6, false) => Ok(Box::new(ManualMode::from_controller_state(&state))),
            (7, false) => Ok(Box::new(EyeCareMode::from_controller_state(&state))),
            (_, true) => Ok(Box::new(EReadingMode::from_controller_state(&state))),
            _ => Err(ControllerError::ModeNotDetected),
        }
    }

    fn set_mode(&self, mode: &dyn DisplayMode) -> Result<(), ControllerError> {
        let mut state = self.state.lock().unwrap();
        if mode.is_ereading() {
            state.last_non_ereading_mode = state.mode_id;
            state.is_monochrome = true;
        } else {
            state.mode_id = mode.mode_id();
            state.is_monochrome = false;
        }
        Ok(())
    }

    fn toggle_e_reading(&self) -> Result<Box<dyn DisplayMode>, ControllerError> {
        let state = self.get_state();
        if state.is_monochrome {
            let restored: Box<dyn DisplayMode> = match state.last_non_ereading_mode {
                2 => Box::new(VividMode::new()),
                6 => Box::new(ManualMode::from_controller_state(&state)),
                7 => Box::new(EyeCareMode::from_controller_state(&state)),
                _ => Box::new(NormalMode::new()),
            };
            self.set_mode(&*restored)?;
            Ok(restored)
        } else {
            let ereading = Box::new(EReadingMode::from_controller_state(&state));
            self.set_mode(&*ereading)?;
            Ok(ereading)
        }
    }
}
