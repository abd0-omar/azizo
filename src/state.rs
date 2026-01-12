//! Controller state snapshot.

/// A snapshot of the controller's current state.
///
/// This captures all slider/mode values at a point in time.
/// Use [`DisplayController::get_state`](crate::DisplayController::get_state) to obtain a snapshot.
#[derive(Debug, Clone, Default)]
pub struct ControllerState {
    /// The current mode ID (1=Normal, 2=Vivid, 6=Manual, 7=EyeCare).
    pub mode_id: i32,
    /// Whether monochrome/e-reading mode is active.
    pub is_monochrome: bool,
    /// Display dimming level (40-100 in splendid units).
    pub dimming: i32,
    /// Manual mode color temperature slider value (0-100).
    pub manual_slider: u8,
    /// Eye care mode level (0-4).
    pub eyecare_level: u8,
    /// E-reading grayscale level (0-4).
    pub ereading_grayscale: u8,
    /// E-reading temperature value.
    pub ereading_temp: u8,
    /// The last non-e-reading mode ID (for restoration).
    pub last_non_ereading_mode: i32,
}
