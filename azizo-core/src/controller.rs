//! ASUS display controller implementation.

use crate::error::ControllerError;
use crate::modes::{DisplayMode, EReadingMode, EyeCareMode, ManualMode, NormalMode, VividMode};
use crate::state::ControllerState;

use libloading::{Library, Symbol};
use log::{debug, info};
use std::ffi::c_void;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::{
    Foundation::ERROR_INSUFFICIENT_BUFFER,
    Storage::Packaging::Appx::{
        FindPackagesByPackageFamily, GetPackagePathByFullName, PACKAGE_FILTER_HEAD,
    },
};

const LOCAL_DLL_NAME: &str = "AsusCustomizationRpcClient.dll";

// =============================================================================
// Display Controller Trait
// =============================================================================

/// Trait for display controller implementations.
///
/// This allows for mock implementations in tests.
pub trait DisplayController: Send + Sync {
    /// Get a snapshot of the current controller state.
    fn get_state(&self) -> ControllerState;

    /// Refresh slider values from the device.
    fn refresh_sliders(&self) -> Result<(), ControllerError>;

    /// Sync all slider values from hardware.
    fn sync_all_sliders(&self) -> Result<(), ControllerError>;

    /// Set the display dimming level (40-100 in splendid units).
    fn set_dimming(&self, level: i32) -> Result<(), ControllerError>;

    /// Set dimming using percentage (0-100).
    fn set_dimming_percent(&self, percent: i32) -> Result<(), ControllerError>;

    /// Get the current display mode.
    fn get_current_mode(&self) -> Result<Box<dyn DisplayMode>, ControllerError>;

    /// Set a display mode.
    fn set_mode(&self, mode: &dyn DisplayMode) -> Result<(), ControllerError>;

    /// Toggle e-reading mode on/off.
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
    static LAST_NON_EREADING_MODE: AtomicI32 = AtomicI32::new(1);

    static MANUAL_SLIDER: AtomicI32 = AtomicI32::new(50);
    static EYECARE_SLIDER: AtomicI32 = AtomicI32::new(2);
    static EREADING_GRAYSCALE: AtomicI32 = AtomicI32::new(3);
    static EREADING_TEMP: AtomicI32 = AtomicI32::new(562);
    static CURRENT_DIMMING: AtomicI32 = AtomicI32::new(-1);

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

    pub(super) fn store_last_non_ereading_mode(mode_id: i32) {
        LAST_NON_EREADING_MODE.store(mode_id, Ordering::SeqCst);
    }

    pub(super) fn store_dimming(value: i32) {
        CURRENT_DIMMING.store(value, Ordering::SeqCst);
    }

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
            18 => {
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
            20 => {
                MANUAL_SLIDER.store(data, Ordering::SeqCst);
                debug!("manual slider updated: {}", data);
            }
            21 => {
                EYECARE_SLIDER.store(data, Ordering::SeqCst);
                debug!("eyecare slider updated: {}", data);
            }
            27 => {
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
// AsusController
// =============================================================================

/// Guard to ensure only one controller instance exists at a time.
static INSTANCE_EXISTS: AtomicBool = AtomicBool::new(false);

/// The ASUS display controller.
///
/// Provides access to ASUS Splendid display settings including:
/// - Display modes (Normal, Vivid, Manual, Eye Care, E-Reading)
/// - Dimming control
/// - Slider values
///
/// # Example
///
/// ```no_run
/// use asus_display_control::{AsusController, DisplayController, NormalMode};
///
/// let controller = AsusController::new()?;
/// controller.set_mode(&NormalMode::new())?;
/// # Ok::<(), asus_display_control::ControllerError>(())
/// ```
///
/// # Limitations
///
/// Only one instance can exist at a time due to DLL/RPC constraints.
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
    ///
    /// - [`ControllerError::AlreadyInitialized`] if another instance already exists
    /// - [`ControllerError::PackageNotFound`] if the ASUS package is not installed
    /// - [`ControllerError::DllLoad`] if the DLL fails to load
    /// - [`ControllerError::RpcInitFailed`] if RPC initialization fails
    pub fn new() -> Result<Self, ControllerError> {
        if INSTANCE_EXISTS.swap(true, Ordering::SeqCst) {
            return Err(ControllerError::AlreadyInitialized);
        }

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

            type CallbackFn = unsafe extern "C" fn(i32, i32, *const i8);
            type SetCallbackFn = unsafe extern "C" fn(CallbackFn, *mut c_void);
            let set_callback: Symbol<SetCallbackFn> =
                lib.get(b"SetCallbackForReturnOptimizationResult")?;
            set_callback(callback_state::mode_callback, client);

            Ok(Self { lib, client })
        }
    }

    fn call_rpc_get(&self, symbol: &[u8]) -> Result<i64, ControllerError> {
        unsafe {
            type GetFn = unsafe extern "C" fn(*mut c_void) -> i64;
            let func: Symbol<GetFn> = self.lib.get(symbol)?;
            Ok(func(self.client))
        }
    }

    /// Set a splendid mode with a value parameter.
    ///
    /// This is used internally by mode implementations.
    pub fn set_splendid_mode(&self, symbol: &[u8], value: u8) -> Result<(), ControllerError> {
        unsafe {
            type SetModeFn = unsafe extern "C" fn(u8, *const i8, *mut c_void) -> i64;
            let set_fn: Symbol<SetModeFn> = self.lib.get(symbol)?;
            let empty_str = b"\0".as_ptr() as *const i8;
            set_fn(value, empty_str, self.client);
            Ok(())
        }
    }

    /// Set monochrome/e-reading mode with grayscale and temp.
    ///
    /// This is used internally by [`EReadingMode`].
    pub fn set_monochrome_mode(&self, grayscale: u8, temp: u8) -> Result<(), ControllerError> {
        unsafe {
            type SetMonoFn = unsafe extern "C" fn(i32, *mut c_void) -> i64;
            let set_mono: Symbol<SetMonoFn> = self.lib.get(b"MyOptSetSplendidMonochromeFunc")?;
            let value = (grayscale as i32 * 256) + temp as i32 - 206;
            set_mono(value, self.client);
            Ok(())
        }
    }

    /// Convert dimming from splendid units (40-100) to percentage (0-100).
    pub fn dimming_to_percent(splendid_value: i32) -> i32 {
        let clamped = splendid_value.clamp(40, 100);
        ((clamped - 40) as f32 / 60.0 * 100.0).round() as i32
    }

    /// Convert dimming from percentage (0-100) to splendid units (40-100).
    pub fn percent_to_dimming(percent: i32) -> i32 {
        40 + (percent as f32 / 100.0 * 60.0).round() as i32
    }

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

    fn restore_last_mode(&self, state: &ControllerState) -> Box<dyn DisplayMode> {
        match state.last_non_ereading_mode {
            2 => Box::new(VividMode::new()),
            6 => Box::new(ManualMode::from_controller_state(state)),
            7 => Box::new(EyeCareMode::from_controller_state(state)),
            _ => Box::new(NormalMode::new()),
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

        let _ = self.get_current_mode();
        self.refresh_sliders()?;
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
            let restored = self.restore_last_mode(&state);
            info!("switching from e-reading to {:?}", restored);
            restored
        } else {
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
            type UninitFn = unsafe extern "C" fn(*mut c_void);
            if let Ok(uninit) = self.lib.get::<UninitFn>(b"MyOptRpcClientUninitialize") {
                uninit(self.client);
            }
        }
        INSTANCE_EXISTS.store(false, Ordering::SeqCst);
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
