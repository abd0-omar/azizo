//! Unofficial Rust API for ASUS Splendid display control.
//!
//! This crate provides programmatic access to ASUS display settings on Windows laptops
//! with Splendid display support.
//!
//! # Requirements
//!
//! - Windows 10/11
//! - ASUS laptop with Splendid display support
//! - ASUS PC Assistant installed
//!
//! # Example
//!
//! ```no_run
//! use asus_display_control::{AsusController, DisplayController, NormalMode, EyeCareMode};
//!
//! fn main() -> Result<(), asus_display_control::ControllerError> {
//!     // Create the controller (only one instance allowed)
//!     let controller = AsusController::new()?;
//!
//!     // Sync state from hardware
//!     controller.sync_all_sliders()?;
//!
//!     // Get current state
//!     let state = controller.get_state();
//!     println!("Current dimming: {}%", AsusController::dimming_to_percent(state.dimming));
//!
//!     // Set a mode
//!     controller.set_mode(&NormalMode::new())?;
//!
//!     // Set eye care mode with level 2
//!     controller.set_mode(&EyeCareMode::new(2)?)?;
//!
//!     // Toggle e-reading mode
//!     controller.toggle_e_reading()?;
//!
//!     // Adjust dimming (0-100%)
//!     controller.set_dimming_percent(75)?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Testing
//!
//! Use [`MockController`] to test code without hardware:
//!
//! ```
//! use asus_display_control::{MockController, DisplayController, NormalMode};
//!
//! let mock = MockController::new();
//! mock.set_mode(&NormalMode::new()).unwrap();
//! assert_eq!(mock.get_state().mode_id, 1);
//! ```
//!
//! # Disclaimer
//!
//! This is an **unofficial** library. It is not affiliated with or endorsed by ASUS.
//! Use at your own risk.

#![warn(missing_docs)]

mod controller;
mod error;
mod mock;
mod modes;
mod state;

// Re-export public API
pub use controller::{AsusController, DisplayController};
pub use error::ControllerError;
pub use mock::MockController;
pub use modes::{DisplayMode, EReadingMode, EyeCareMode, ManualMode, NormalMode, VividMode};
pub use state::ControllerState;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_controller_toggle_ereading() {
        let mock = MockController::new();

        let mode = mock.get_current_mode().unwrap();
        assert!(!mode.is_ereading());
        assert_eq!(mode.mode_id(), 1);

        let mode = mock.toggle_e_reading().unwrap();
        assert!(mode.is_ereading());

        let mode = mock.toggle_e_reading().unwrap();
        assert!(!mode.is_ereading());
        assert_eq!(mode.mode_id(), 1);
    }

    #[test]
    fn test_mock_controller_dimming() {
        let mock = MockController::new();

        mock.set_dimming(80).unwrap();
        assert_eq!(mock.get_state().dimming, 80);

        mock.set_dimming_percent(50).unwrap();
        let expected = AsusController::percent_to_dimming(50);
        assert_eq!(mock.get_state().dimming, expected);
    }

    #[test]
    fn test_dimming_conversion() {
        assert_eq!(AsusController::percent_to_dimming(0), 40);
        assert_eq!(AsusController::percent_to_dimming(100), 100);
        assert_eq!(AsusController::percent_to_dimming(50), 70);

        assert_eq!(AsusController::dimming_to_percent(40), 0);
        assert_eq!(AsusController::dimming_to_percent(100), 100);
        assert_eq!(AsusController::dimming_to_percent(70), 50);
    }

    #[test]
    fn test_mode_from_controller_state() {
        let state = ControllerState {
            manual_slider: 75,
            eyecare_level: 3,
            ereading_grayscale: 2,
            ereading_temp: 60,
            ..Default::default()
        };

        let manual = ManualMode::from_controller_state(&state);
        assert_eq!(manual.value, 75);

        let eyecare = EyeCareMode::from_controller_state(&state);
        assert_eq!(eyecare.level, 3);

        let ereading = EReadingMode::from_controller_state(&state);
        assert_eq!(ereading.grayscale, 2);
        assert_eq!(ereading.temp, 60);
    }
}
