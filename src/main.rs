use windows_sys::Win32::{
    Foundation::ERROR_INSUFFICIENT_BUFFER,
    Storage::Packaging::Appx::{
        FindPackagesByPackageFamily, GetPackagePathByFullName, PACKAGE_FILTER_HEAD,
    },
};

use libloading::{Library, Symbol};
use log::{debug, info};
use std::ffi::c_void;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};

const LOCAL_DLL_NAME: &str = "AsusCustomizationRpcClient.dll";

// =============================================================================
// Error Types
// =============================================================================

#[derive(Debug, thiserror::Error)]
pub enum ControllerError {
    #[error("Package not found (error code: {0})")]
    PackageNotFound(u32),

    #[error("Failed to get package path (error code: {0})")]
    PackagePathError(u32),

    #[error("Failed to load DLL: {0}")]
    DllLoad(#[from] libloading::Error),

    #[error("RPC initialization failed")]
    RpcInitFailed,

    #[error("Controller already initialized - only one instance allowed")]
    AlreadyInitialized,

    #[error("Invalid slider value {value} for {mode} (expected {min}-{max})")]
    InvalidSliderValue {
        mode: &'static str,
        value: u8,
        min: u8,
        max: u8,
    },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to get current mode")]
    ModeNotDetected,

    #[error("Failed to set dimming (error code: {0})")]
    DimmingFailed(i64),
}

// =============================================================================
// Controller State (snapshot of current values)
// =============================================================================

/// A snapshot of the controller's current state.
/// This captures all slider/mode values at a point in time.
#[derive(Debug, Clone, Default)]
pub struct ControllerState {
    pub mode_id: i32,
    pub is_monochrome: bool,
    pub dimming: i32,
    pub manual_slider: u8,
    pub eyecare_level: u8,
    pub ereading_grayscale: u8,
    pub ereading_temp: u8,
    pub last_non_ereading_mode: i32,
}

// =============================================================================
// Display Controller Trait
// =============================================================================

/// Trait for display controller implementations.
/// This allows for mock implementations in tests.
pub trait DisplayController: Send + Sync {
    /// Get a snapshot of the current controller state
    fn get_state(&self) -> ControllerState;

    /// Refresh slider values from the device
    fn refresh_sliders(&self) -> Result<(), ControllerError>;

    /// Sync all slider values from hardware
    fn sync_all_sliders(&self) -> Result<(), ControllerError>;

    /// Set the display dimming level (40-100)
    fn set_dimming(&self, level: i32) -> Result<(), ControllerError>;

    /// Set dimming using percentage (0-100)
    fn set_dimming_percent(&self, percent: i32) -> Result<(), ControllerError>;

    /// Get the current display mode
    fn get_current_mode(&self) -> Result<Box<dyn DisplayMode>, ControllerError>;

    /// Set a display mode
    fn set_mode(&self, mode: &dyn DisplayMode) -> Result<(), ControllerError>;

    /// Toggle e-reading mode on/off
    fn toggle_e_reading(&self) -> Result<Box<dyn DisplayMode>, ControllerError>;
}

// =============================================================================
// Callback State (private module with globals)
// =============================================================================

mod callback_state {
    use super::ControllerState;
    use log::{debug, trace};
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    static CURRENT_MODE: AtomicI32 = AtomicI32::new(-1);
    static IS_MONOCHROME: AtomicBool = AtomicBool::new(false);
    static LAST_NON_EREADING_MODE: AtomicI32 = AtomicI32::new(1); // Default to Normal

    static MANUAL_SLIDER: AtomicI32 = AtomicI32::new(50); // Default middle
    static EYECARE_SLIDER: AtomicI32 = AtomicI32::new(2); // Default level
    static EREADING_GRAYSCALE: AtomicI32 = AtomicI32::new(3); // Default grayscale 3
    static EREADING_TEMP: AtomicI32 = AtomicI32::new(562); // Default temp
    static CURRENT_DIMMING: AtomicI32 = AtomicI32::new(-1); // Dimming level (40-100)

    /// Get a snapshot of all current state values
    pub(super) fn snapshot() -> ControllerState {
        ControllerState {
            mode_id: CURRENT_MODE.load(Ordering::SeqCst),
            is_monochrome: IS_MONOCHROME.load(Ordering::SeqCst),
            dimming: CURRENT_DIMMING.load(Ordering::SeqCst),
            manual_slider: MANUAL_SLIDER.load(Ordering::SeqCst) as u8,
            eyecare_level: EYECARE_SLIDER.load(Ordering::SeqCst) as u8,
            ereading_grayscale: EREADING_GRAYSCALE.load(Ordering::SeqCst) as u8,
            ereading_temp: EREADING_TEMP.load(Ordering::SeqCst) as u8,
            last_non_ereading_mode: LAST_NON_EREADING_MODE.load(Ordering::SeqCst),
        }
    }

    /// Store the last non-e-reading mode
    pub(super) fn store_last_non_ereading_mode(mode_id: i32) {
        LAST_NON_EREADING_MODE.store(mode_id, Ordering::SeqCst);
    }

    /// Store the current dimming value
    pub(super) fn store_dimming(value: i32) {
        CURRENT_DIMMING.store(value, Ordering::SeqCst);
    }

    /// The callback function for the ASUS DLL
    pub(super) extern "C" fn mode_callback(func: i32, data: i32, str_data: *const i8) {
        let s = if str_data.is_null() {
            String::from("null")
        } else {
            unsafe {
                std::ffi::CStr::from_ptr(str_data)
                    .to_string_lossy()
                    .to_string()
            }
        };

        trace!("callback: func={}, data={}, str='{}'", func, data, s);

        match func {
            // Mode info callback
            18 => {
                // Parse "0_1_0_1_1,70,0" -> dimming is 2nd field, monochrome is 3rd field
                let parts: Vec<&str> = s.split(',').collect();
                if parts.len() >= 2 {
                    if let Ok(dimming) = parts[1].parse::<i32>() {
                        CURRENT_DIMMING.store(dimming, Ordering::SeqCst);
                    }
                }
                if parts.len() >= 3 {
                    if let Ok(mono) = parts[2].parse::<i32>() {
                        IS_MONOCHROME.store(mono != 0, Ordering::SeqCst);
                    }
                }
                CURRENT_MODE.store(data, Ordering::SeqCst);

                debug!(
                    "mode updated: data={}, dimming={}, monochrome={}",
                    data,
                    CURRENT_DIMMING.load(Ordering::SeqCst),
                    IS_MONOCHROME.load(Ordering::SeqCst)
                );
            }
            // Manual slider callback
            20 => {
                MANUAL_SLIDER.store(data, Ordering::SeqCst);
                debug!("manual slider updated: {}", data);
            }
            // EyeCare slider callback
            21 => {
                EYECARE_SLIDER.store(data, Ordering::SeqCst);
                debug!("eyecare slider updated: {}", data);
            }
            // E-reading/Monochrome callback
            27 => {
                // Decode: value = (grayscale * 256) + temp - 206
                let raw = data + 206;
                let grayscale = raw / 256;
                let temp = raw % 256;
                EREADING_GRAYSCALE.store(grayscale, Ordering::SeqCst);
                EREADING_TEMP.store(temp, Ordering::SeqCst);
                debug!("e-reading updated: grayscale={}, temp={}", grayscale, temp);
            }
            _ => {}
        }
    }
}

// =============================================================================
// Display Mode Trait
// =============================================================================

pub trait DisplayMode: std::fmt::Debug + Send + Sync {
    /// Apply this mode using the controller
    fn apply(&self, controller: &AsusController) -> Result<(), ControllerError>;

    /// Get the RPC symbol name for setting this mode
    fn symbol(&self) -> &'static [u8];

    /// Whether this is an e-reading/monochrome mode
    fn is_ereading(&self) -> bool {
        false
    }

    /// Get the mode ID for this mode (used for state tracking)
    fn mode_id(&self) -> i32;
}

// =============================================================================
// Mode Implementations
// =============================================================================

#[derive(Debug, Clone, Copy)]
pub struct NormalMode;

impl NormalMode {
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

#[derive(Debug, Clone, Copy)]
pub struct VividMode;

impl VividMode {
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

#[derive(Debug, Clone, Copy)]
pub struct ManualMode {
    pub value: u8,
}

impl ManualMode {
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

    /// Create from controller state snapshot
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

#[derive(Debug, Clone, Copy)]
pub struct EyeCareMode {
    pub level: u8,
}

impl EyeCareMode {
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

    /// Create from controller state snapshot
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

#[derive(Debug, Clone, Copy)]
pub struct EReadingMode {
    /// Grayscale level: 0-4 (maps to 1-5 internally)
    pub grayscale: u8,
    /// Temperature: 0-100 (maps to -50 to +50 internally)
    pub temp: u8,
}

impl EReadingMode {
    pub fn new(grayscale: u8, temp: u8) -> Result<Self, ControllerError> {
        if grayscale > 4 {
            return Err(ControllerError::InvalidSliderValue {
                mode: "EReading grayscale",
                value: grayscale,
                min: 0,
                max: 4,
            });
        }
        // temp is stored as raw value from callback, allow full range
        Ok(Self { grayscale, temp })
    }

    /// Create from controller state snapshot
    pub fn from_controller_state(state: &ControllerState) -> Self {
        Self {
            grayscale: state.ereading_grayscale,
            temp: state.ereading_temp,
        }
    }
}

impl DisplayMode for EReadingMode {
    fn apply(&self, controller: &AsusController) -> Result<(), ControllerError> {
        controller.set_monochrome_mode(self.grayscale, self.temp)
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

// =============================================================================
// AsusController
// =============================================================================

/// Guard to ensure only one controller instance exists at a time
static INSTANCE_EXISTS: AtomicBool = AtomicBool::new(false);

pub struct AsusController {
    lib: Library,
    client: *mut c_void,
}

// Safety: The client pointer is only used with the DLL functions
// and the Library keeps the DLL loaded for the lifetime of AsusController
unsafe impl Send for AsusController {}
unsafe impl Sync for AsusController {}

impl AsusController {
    /// Create a new controller instance.
    ///
    /// Only one instance can exist at a time due to DLL/RPC limitations.
    /// The instance guard is released when the controller is dropped.
    ///
    /// # Errors
    /// - `AlreadyInitialized` if another instance already exists
    /// - `PackageNotFound` if the ASUS package is not installed
    /// - `DllLoad` if the DLL fails to load
    /// - `RpcInitFailed` if RPC initialization fails
    pub fn new() -> Result<Self, ControllerError> {
        // Check if an instance already exists
        if INSTANCE_EXISTS.swap(true, Ordering::SeqCst) {
            return Err(ControllerError::AlreadyInitialized);
        }

        // If initialization fails, release the guard
        match Self::init_internal() {
            Ok(controller) => Ok(controller),
            Err(e) => {
                INSTANCE_EXISTS.store(false, Ordering::SeqCst);
                Err(e)
            }
        }
    }

    fn init_internal() -> Result<Self, ControllerError> {
        let full_name = find_asus_package()?;
        let path = get_package_path(&full_name)?;
        let dll_path = format!("{}\\ModuleDll\\HWSettings\\{}", path, LOCAL_DLL_NAME);

        fs::copy(&dll_path, LOCAL_DLL_NAME)?;

        unsafe {
            let lib = Library::new(LOCAL_DLL_NAME)?;

            type InitFn = unsafe extern "C" fn(*mut *mut c_void) -> i64;
            let init: Symbol<InitFn> = lib.get(b"MyOptRpcClientInitialize")?;

            let mut client: *mut c_void = std::ptr::null_mut();
            let result = init(&mut client);
            if result != 0 || client.is_null() {
                return Err(ControllerError::RpcInitFailed);
            }

            // Register callback
            type CallbackFn = unsafe extern "C" fn(i32, i32, *const i8);
            type SetCallbackFn = unsafe extern "C" fn(CallbackFn, *mut c_void);
            let set_callback: Symbol<SetCallbackFn> =
                lib.get(b"SetCallbackForReturnOptimizationResult")?;
            set_callback(callback_state::mode_callback, client);

            Ok(Self { lib, client })
        }
    }

    /// Call an RPC function that takes only the client pointer and returns i64
    fn call_rpc_get(&self, symbol: &[u8]) -> Result<i64, ControllerError> {
        unsafe {
            type GetFn = unsafe extern "C" fn(*mut c_void) -> i64;
            let func: Symbol<GetFn> = self.lib.get(symbol)?;
            Ok(func(self.client))
        }
    }

    /// Set a splendid mode with a value parameter
    fn set_splendid_mode(&self, symbol: &[u8], value: u8) -> Result<(), ControllerError> {
        unsafe {
            type SetModeFn = unsafe extern "C" fn(u8, *const i8, *mut c_void) -> i64;
            let set_fn: Symbol<SetModeFn> = self.lib.get(symbol)?;
            let empty_str = b"\0".as_ptr() as *const i8;
            set_fn(value, empty_str, self.client);
            Ok(())
        }
    }

    /// Set monochrome/e-reading mode with grayscale and temp
    fn set_monochrome_mode(&self, grayscale: u8, temp: u8) -> Result<(), ControllerError> {
        unsafe {
            type SetMonoFn = unsafe extern "C" fn(i32, *mut c_void) -> i64;
            let set_mono: Symbol<SetMonoFn> = self.lib.get(b"MyOptSetSplendidMonochromeFunc")?;
            let value = (grayscale as i32 * 256) + temp as i32 - 206;
            set_mono(value, self.client);
            Ok(())
        }
    }

    /// Convert dimming from splendid units (40-100) to percentage (0-100)
    pub fn dimming_to_percent(splendid_value: i32) -> i32 {
        let clamped = splendid_value.clamp(40, 100);
        ((clamped - 40) as f32 / 60.0 * 100.0).round() as i32
    }

    /// Convert dimming from percentage (0-100) to splendid units (40-100)
    pub fn percent_to_dimming(percent: i32) -> i32 {
        40 + (percent as f32 / 100.0 * 60.0).round() as i32
    }

    /// Create a mode from state snapshot
    fn mode_from_state(
        &self,
        state: &ControllerState,
    ) -> Result<Box<dyn DisplayMode>, ControllerError> {
        match (state.mode_id, state.is_monochrome) {
            (1, false) => Ok(Box::new(NormalMode::new())),
            (2, false) => Ok(Box::new(VividMode::new())),
            (6, false) => Ok(Box::new(ManualMode::from_controller_state(state))),
            (7, false) => Ok(Box::new(EyeCareMode::from_controller_state(state))),
            (_, true) => {
                callback_state::store_last_non_ereading_mode(state.mode_id);
                Ok(Box::new(EReadingMode::from_controller_state(state)))
            }
            _ => Err(ControllerError::ModeNotDetected),
        }
    }

    /// Restore the last non-e-reading mode
    fn restore_last_mode(&self, state: &ControllerState) -> Box<dyn DisplayMode> {
        match state.last_non_ereading_mode {
            2 => Box::new(VividMode::new()),
            6 => Box::new(ManualMode::from_controller_state(state)),
            7 => Box::new(EyeCareMode::from_controller_state(state)),
            _ => Box::new(NormalMode::new()), // Default to Normal
        }
    }
}

impl DisplayController for AsusController {
    fn get_state(&self) -> ControllerState {
        callback_state::snapshot()
    }

    fn refresh_sliders(&self) -> Result<(), ControllerError> {
        self.call_rpc_get(b"MyOptGetSplendidManualModeFunc")?;
        self.call_rpc_get(b"MyOptGetSplendidEyecareModeFunc")?;
        self.call_rpc_get(b"MyOptGetSplendidMonochromeFunc")?;
        Ok(())
    }

    fn sync_all_sliders(&self) -> Result<(), ControllerError> {
        debug!("syncing all sliders from ASUS...");

        // 1. Get current mode - this also fetches dimming value via func=18 callback
        let _ = self.get_current_mode();

        // 2. Refresh all other sliders
        self.refresh_sliders()?;

        // Wait for callbacks
        std::thread::sleep(std::time::Duration::from_millis(500));

        let state = self.get_state();
        debug!(
            "sync complete: dimming={}({}%), manual={}, eyecare={}, e-reading(grayscale={}, temp={})",
            state.dimming,
            Self::dimming_to_percent(state.dimming),
            state.manual_slider,
            state.eyecare_level,
            state.ereading_grayscale,
            state.ereading_temp
        );
        Ok(())
    }

    fn set_dimming(&self, level: i32) -> Result<(), ControllerError> {
        let level = level.clamp(40, 100);
        unsafe {
            type SetDimmingFn = unsafe extern "C" fn(i32, *const i8, *mut c_void) -> i64;
            let set_dimming: Symbol<SetDimmingFn> = self.lib.get(b"MyOptSetSplendidDimmingFunc")?;

            let empty_str = b"\0".as_ptr() as *const i8;
            let result = set_dimming(level, empty_str, self.client);
            debug!("set dimming to {}, result: {}", level, result);

            if result == 0 {
                callback_state::store_dimming(level);
                Ok(())
            } else {
                Err(ControllerError::DimmingFailed(result))
            }
        }
    }

    fn set_dimming_percent(&self, percent: i32) -> Result<(), ControllerError> {
        let splendid_value = Self::percent_to_dimming(percent.clamp(0, 100));
        self.set_dimming(splendid_value)
    }

    fn get_current_mode(&self) -> Result<Box<dyn DisplayMode>, ControllerError> {
        self.call_rpc_get(b"MyOptGetSplendidColorModeFunc")?;

        // Wait for callback to populate values
        std::thread::sleep(std::time::Duration::from_millis(500));

        let state = self.get_state();
        self.mode_from_state(&state)
    }

    fn set_mode(&self, mode: &dyn DisplayMode) -> Result<(), ControllerError> {
        mode.apply(self)
    }

    fn toggle_e_reading(&self) -> Result<Box<dyn DisplayMode>, ControllerError> {
        let current = self.get_current_mode()?;
        debug!("current mode: {:?}", current);

        let state = self.get_state();
        let target: Box<dyn DisplayMode> = if current.is_ereading() {
            // Exit e-reading - restore previous mode
            let restored = self.restore_last_mode(&state);
            info!("switching from e-reading to {:?}", restored);
            restored
        } else {
            // Enter e-reading
            info!("switching to e-reading");
            Box::new(EReadingMode::from_controller_state(&state))
        };

        self.set_mode(&*target)?;
        Ok(target)
    }
}

impl Drop for AsusController {
    fn drop(&mut self) {
        unsafe {
            // Try to uninitialize the RPC client
            type UninitFn = unsafe extern "C" fn(*mut c_void);
            if let Ok(uninit) = self.lib.get::<UninitFn>(b"MyOptRpcClientUninitialize") {
                uninit(self.client);
            }
        }
        // Release the instance guard
        INSTANCE_EXISTS.store(false, Ordering::SeqCst);
    }
}

// =============================================================================
// Mock Controller (for testing)
// =============================================================================

#[cfg(test)]
pub struct MockController {
    state: std::sync::Mutex<ControllerState>,
}

#[cfg(test)]
impl MockController {
    pub fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(ControllerState {
                mode_id: 1,
                is_monochrome: false,
                dimming: 70,
                manual_slider: 50,
                eyecare_level: 2,
                ereading_grayscale: 3,
                ereading_temp: 50,
                last_non_ereading_mode: 1,
            }),
        }
    }

    pub fn with_state(state: ControllerState) -> Self {
        Self {
            state: std::sync::Mutex::new(state),
        }
    }
}

#[cfg(test)]
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
            // Exit e-reading
            let restored: Box<dyn DisplayMode> = match state.last_non_ereading_mode {
                2 => Box::new(VividMode::new()),
                6 => Box::new(ManualMode::from_controller_state(&state)),
                7 => Box::new(EyeCareMode::from_controller_state(&state)),
                _ => Box::new(NormalMode::new()),
            };
            self.set_mode(&*restored)?;
            Ok(restored)
        } else {
            // Enter e-reading
            let ereading = Box::new(EReadingMode::from_controller_state(&state));
            self.set_mode(&*ereading)?;
            Ok(ereading)
        }
    }
}

// =============================================================================
// Windows Package Helpers
// =============================================================================

fn find_asus_package() -> Result<String, ControllerError> {
    let family_name: Vec<u16> = "B9ECED6F.ASUSPCAssistant_qmba6cd70vzyy\0"
        .encode_utf16()
        .collect();

    let mut count = 0u32;
    let mut buffer_length = 0u32;

    let result = unsafe {
        FindPackagesByPackageFamily(
            family_name.as_ptr(),
            PACKAGE_FILTER_HEAD,
            &mut count,
            std::ptr::null_mut(),
            &mut buffer_length,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };

    if result != ERROR_INSUFFICIENT_BUFFER {
        return Err(ControllerError::PackageNotFound(result));
    }

    let mut package_names: Vec<*mut u16> = vec![std::ptr::null_mut(); count as usize];
    let mut buffer: Vec<u16> = vec![0; buffer_length as usize];

    let result = unsafe {
        FindPackagesByPackageFamily(
            family_name.as_ptr(),
            PACKAGE_FILTER_HEAD,
            &mut count,
            package_names.as_mut_ptr(),
            &mut buffer_length,
            buffer.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    };

    if result != 0 {
        return Err(ControllerError::PackageNotFound(result));
    }

    let full_name = unsafe {
        let ptr = package_names[0];
        let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    };

    Ok(full_name)
}

fn get_package_path(full_name: &str) -> Result<String, ControllerError> {
    let full_name_wide: Vec<u16> = format!("{}\0", full_name).encode_utf16().collect();
    let mut buffer_length = 0u32;

    let result = unsafe {
        GetPackagePathByFullName(
            full_name_wide.as_ptr(),
            &mut buffer_length,
            std::ptr::null_mut(),
        )
    };

    if result != ERROR_INSUFFICIENT_BUFFER {
        return Err(ControllerError::PackagePathError(result));
    }

    let mut buffer: Vec<u16> = vec![0; buffer_length as usize];
    let result = unsafe {
        GetPackagePathByFullName(
            full_name_wide.as_ptr(),
            &mut buffer_length,
            buffer.as_mut_ptr(),
        )
    };

    if result != 0 {
        return Err(ControllerError::PackagePathError(result));
    }

    let len = buffer.iter().take_while(|&&c| c != 0).count();
    Ok(String::from_utf16_lossy(&buffer[..len]))
}

// =============================================================================
// Main
// =============================================================================

fn main() -> Result<(), ControllerError> {
    let ctrl = AsusController::new()?;

    // Refresh slider values from device
    ctrl.refresh_sliders()?;

    // Toggle e-reading mode
    match ctrl.toggle_e_reading() {
        Ok(new_mode) => println!("Toggled to: {:?}", new_mode),
        Err(e) => println!("Error: {}", e),
    }

    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_controller_toggle_ereading() {
        let mock = MockController::new();

        // Initially in Normal mode
        let mode = mock.get_current_mode().unwrap();
        assert!(!mode.is_ereading());
        assert_eq!(mode.mode_id(), 1);

        // Toggle to e-reading
        let mode = mock.toggle_e_reading().unwrap();
        assert!(mode.is_ereading());

        // Toggle back to Normal
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
        // 0% -> 40, 100% -> 100
        assert_eq!(AsusController::percent_to_dimming(0), 40);
        assert_eq!(AsusController::percent_to_dimming(100), 100);
        assert_eq!(AsusController::percent_to_dimming(50), 70);

        // Reverse
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
