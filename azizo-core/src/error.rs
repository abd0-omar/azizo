//! Error types for the ASUS display controller.

/// Errors that can occur when using the ASUS display controller.
#[derive(Debug, thiserror::Error)]
pub enum ControllerError {
    /// The ASUS PC Assistant package was not found.
    #[error("Package not found (error code: {0})")]
    PackageNotFound(u32),

    /// Failed to get the package installation path.
    #[error("Failed to get package path (error code: {0})")]
    PackagePathError(u32),

    /// Failed to load the ASUS DLL.
    #[error("Failed to load DLL: {0}")]
    DllLoad(#[from] libloading::Error),

    /// RPC client initialization failed.
    #[error("RPC initialization failed")]
    RpcInitFailed,

    /// Attempted to create a second controller instance.
    #[error("Controller already initialized - only one instance allowed")]
    AlreadyInitialized,

    /// A slider value was outside the valid range.
    #[error("Invalid slider value {value} for {mode} (expected {min}-{max})")]
    InvalidSliderValue {
        /// The mode name.
        mode: &'static str,
        /// The invalid value provided.
        value: u8,
        /// Minimum allowed value.
        min: u8,
        /// Maximum allowed value.
        max: u8,
    },

    /// An I/O error occurred (e.g., copying the DLL).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to detect the current display mode.
    #[error("Failed to get current mode")]
    ModeNotDetected,

    /// Failed to set the dimming level.
    #[error("Failed to set dimming (error code: {0})")]
    DimmingFailed(i64),
}
