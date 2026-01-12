use windows_sys::Win32::{
    Foundation::ERROR_INSUFFICIENT_BUFFER,
    Storage::Packaging::Appx::{
        FindPackagesByPackageFamily, GetPackagePathByFullName, PACKAGE_FILTER_HEAD,
    },
};

use libloading::{Library, Symbol};
use std::ffi::c_void;
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

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
}

// =============================================================================
// Global Callback State
// =============================================================================

static CURRENT_MODE: AtomicI32 = AtomicI32::new(-1);
static IS_MONOCHROME: AtomicBool = AtomicBool::new(false);
static LAST_NON_EREADING_MODE: AtomicI32 = AtomicI32::new(1); // Default to Normal

static MANUAL_SLIDER: AtomicI32 = AtomicI32::new(50); // Default middle
static EYECARE_SLIDER: AtomicI32 = AtomicI32::new(2); // Default level
static EREADING_GRAYSCALE: AtomicI32 = AtomicI32::new(3); // Default grayscale 3
static EREADING_TEMP: AtomicI32 = AtomicI32::new(562); // Default temp

extern "C" fn mode_callback(func: i32, data: i32, str_data: *const i8) {
    let s = if str_data.is_null() {
        String::from("null")
    } else {
        unsafe {
            std::ffi::CStr::from_ptr(str_data)
                .to_string_lossy()
                .to_string()
        }
    };

    println!("func={}, data={}, str='{}'", func, data, s);

    match func {
        // Mode info callback
        18 => {
            // Parse "0_1_0_1_1,70,0" -> monochrome is the 3rd field
            let parts: Vec<&str> = s.split(',').collect();
            if parts.len() >= 3 {
                if let Ok(mono) = parts[2].parse::<i32>() {
                    IS_MONOCHROME.store(mono != 0, Ordering::SeqCst);
                }
            }
            CURRENT_MODE.store(data, Ordering::SeqCst);

            println!(
                "Mode updated: data={}, monochrome={}",
                data,
                IS_MONOCHROME.load(Ordering::SeqCst)
            );
        }
        // Manual slider callback
        20 => {
            MANUAL_SLIDER.store(data, Ordering::SeqCst);
            println!("Manual slider updated: {}", data);
        }
        // EyeCare slider callback
        21 => {
            EYECARE_SLIDER.store(data, Ordering::SeqCst);
            println!("EyeCare slider updated: {}", data);
        }
        // E-reading/Monochrome callback
        27 => {
            // Decode: value = (grayscale * 256) + temp - 206
            let raw = data + 206;
            let grayscale = raw / 256;
            let temp = raw % 256;
            EREADING_GRAYSCALE.store(grayscale, Ordering::SeqCst);
            EREADING_TEMP.store(temp, Ordering::SeqCst);
            println!("E-reading updated: grayscale={}, temp={}", grayscale, temp);
        }
        _ => {}
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

    /// Create from global state
    pub fn from_state() -> Self {
        Self {
            value: MANUAL_SLIDER.load(Ordering::SeqCst) as u8,
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

    /// Create from global state
    pub fn from_state() -> Self {
        Self {
            level: EYECARE_SLIDER.load(Ordering::SeqCst) as u8,
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

    /// Create from global state
    pub fn from_state() -> Self {
        Self {
            grayscale: EREADING_GRAYSCALE.load(Ordering::SeqCst) as u8,
            temp: EREADING_TEMP.load(Ordering::SeqCst) as u8,
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

pub struct AsusController {
    lib: Library,
    client: *mut c_void,
}

// Safety: The client pointer is only used with the DLL functions
// and the Library keeps the DLL loaded for the lifetime of AsusController
unsafe impl Send for AsusController {}
unsafe impl Sync for AsusController {}

impl AsusController {
    pub fn new() -> Result<Self, ControllerError> {
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
            set_callback(mode_callback, client);

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

    /// Refresh slider values from the device
    pub fn refresh_sliders(&self) -> Result<(), ControllerError> {
        self.call_rpc_get(b"MyOptGetSplendidManualModeFunc")?;
        self.call_rpc_get(b"MyOptGetSplendidEyecareModeFunc")?;
        self.call_rpc_get(b"MyOptGetSplendidMonochromeFunc")?;
        Ok(())
    }

    /// Get the current display mode
    pub fn get_current_mode(&self) -> Result<Box<dyn DisplayMode>, ControllerError> {
        self.call_rpc_get(b"MyOptGetSplendidColorModeFunc")?;

        // Wait for callback to populate values
        std::thread::sleep(std::time::Duration::from_millis(500));

        let mode_id = CURRENT_MODE.load(Ordering::SeqCst);
        let is_mono = IS_MONOCHROME.load(Ordering::SeqCst);

        self.mode_from_state(mode_id, is_mono)
    }

    /// Create a mode from current global state
    fn mode_from_state(
        &self,
        mode_id: i32,
        is_monochrome: bool,
    ) -> Result<Box<dyn DisplayMode>, ControllerError> {
        match (mode_id, is_monochrome) {
            (1, false) => Ok(Box::new(NormalMode::new())),
            (2, false) => Ok(Box::new(VividMode::new())),
            (6, false) => Ok(Box::new(ManualMode::from_state())),
            (7, false) => Ok(Box::new(EyeCareMode::from_state())),
            (_, true) => {
                LAST_NON_EREADING_MODE.store(mode_id, Ordering::SeqCst);
                Ok(Box::new(EReadingMode::from_state()))
            }
            _ => Err(ControllerError::ModeNotDetected),
        }
    }

    /// Restore the last non-e-reading mode
    fn restore_last_mode(&self) -> Box<dyn DisplayMode> {
        let last = LAST_NON_EREADING_MODE.load(Ordering::SeqCst);
        match last {
            2 => Box::new(VividMode::new()),
            6 => Box::new(ManualMode::from_state()),
            7 => Box::new(EyeCareMode::from_state()),
            _ => Box::new(NormalMode::new()), // Default to Normal
        }
    }

    /// Set a display mode
    pub fn set_mode(&self, mode: &dyn DisplayMode) -> Result<(), ControllerError> {
        mode.apply(self)
    }

    /// Toggle e-reading mode on/off
    pub fn toggle_e_reading(&self) -> Result<Box<dyn DisplayMode>, ControllerError> {
        let current = self.get_current_mode()?;
        println!("Current mode: {:?}", current);

        let target: Box<dyn DisplayMode> = if current.is_ereading() {
            // Exit e-reading - restore previous mode
            let restored = self.restore_last_mode();
            println!("Switching from E-Reading to {:?}", restored);
            restored
        } else {
            // Enter e-reading
            println!("Switching to E-Reading");
            Box::new(EReadingMode::from_state())
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
    let controller = AsusController::new()?;

    // Refresh slider values from device
    controller.refresh_sliders()?;

    // Toggle e-reading mode
    match controller.toggle_e_reading() {
        Ok(new_mode) => println!("Toggled to: {:?}", new_mode),
        Err(e) => println!("Error: {}", e),
    }

    Ok(())
}
