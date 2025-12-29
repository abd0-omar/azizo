use windows_sys::Win32::{
    Foundation::ERROR_INSUFFICIENT_BUFFER,
    Storage::Packaging::Appx::{
        FindPackagesByPackageFamily, GetPackagePathByFullName, PACKAGE_FILTER_HEAD,
    },
};

use libloading::{Library, Symbol};
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

const LOCAL_DLL_NAME: &str = "AsusCustomizationRpcClient.dll";

// Global state for callback
static CURRENT_MODE: AtomicI32 = AtomicI32::new(-1);
static IS_MONOCHROME: AtomicBool = AtomicBool::new(false);
// this won't have `EReading`
static LAST_NON_EREADING_MODE: AtomicI32 = AtomicI32::new(1); // Default to Normal

static MANUAL_SLIDER: AtomicI32 = AtomicI32::new(50); // Default middle
static EYECARE_SLIDER: AtomicI32 = AtomicI32::new(2); // Default level
static EREADING_GRAYSCALE: AtomicI32 = AtomicI32::new(3); // Default grayscale 3
static EREADING_TEMP: AtomicI32 = AtomicI32::new(562); // Default temp 3

/// Splendid display modes with associated slider values
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplendidMode {
    Normal,
    Vivid,
    /// Manual color temperature: 0-100
    Manual(u8),
    /// EyeCare blue light filter level: 0-4
    EyeCare(u8),
    /// E-Reading mode with grayscale and temperature adjustments
    EReading {
        /// Grayscale level: 0-4 (maps to 1-5 internally)
        grayscale: u8,
        /// Temperature: 0-100 (maps to -50 to +50 internally)
        temp: u8,
    },
}

impl SplendidMode {
    /// Create from mode ID, loading slider values from global state
    fn from_mode_id(mode_id: i32) -> Self {
        match mode_id {
            1 => Self::Normal,
            2 => Self::Vivid,
            6 => Self::Manual(MANUAL_SLIDER.load(Ordering::SeqCst) as u8),
            7 => Self::EyeCare(EYECARE_SLIDER.load(Ordering::SeqCst) as u8),
            _ => unreachable!(),
        }
    }

    fn from_callback(data: i32, is_monochrome: bool) -> Option<Self> {
        match (data, is_monochrome) {
            (1, false) => Some(Self::Normal),
            (2, false) => Some(Self::Vivid),
            (6, false) => Some(Self::Manual(MANUAL_SLIDER.load(Ordering::SeqCst) as u8)),
            (7, false) => Some(Self::EyeCare(EYECARE_SLIDER.load(Ordering::SeqCst) as u8)),
            (n, true) => {
                LAST_NON_EREADING_MODE.store(n, Ordering::SeqCst);
                Some(Self::EReading {
                    grayscale: EREADING_GRAYSCALE.load(Ordering::SeqCst) as u8,
                    temp: EREADING_TEMP.load(Ordering::SeqCst) as u8,
                })
            }
            _ => None,
        }
    }
}

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

    // Log ALL callbacks for debugging
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
            // So: raw = value + 206, grayscale = raw / 256, temp = raw % 256
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

pub struct AsusController {
    lib: Library,
    client: *mut std::ffi::c_void,
}

#[derive(Debug)]
pub struct SliderError(libloading::Error);

impl From<libloading::Error> for SliderError {
    fn from(value: libloading::Error) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for SliderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "failed to store slider value in global {}", self.0,)
    }
}

impl std::error::Error for SliderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl AsusController {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let full_name = find_asus_package().map_err(|e| format!("Package error: {}", e))?;
        let path = get_package_path(&full_name).map_err(|e| format!("Path error: {}", e))?;
        let dll_path = format!("{}\\ModuleDll\\HWSettings\\{}", path, LOCAL_DLL_NAME);

        fs::copy(&dll_path, LOCAL_DLL_NAME)?;

        unsafe {
            let lib = Library::new(LOCAL_DLL_NAME)?;

            type InitFn = unsafe extern "C" fn(*mut *mut std::ffi::c_void) -> i64;
            let init: Symbol<InitFn> = lib.get(b"MyOptRpcClientInitialize")?;

            let mut client: *mut std::ffi::c_void = std::ptr::null_mut();
            let result = init(&mut client);
            if result != 0 || client.is_null() {
                return Err("Failed to initialize RPC".into());
            }

            // Register callback
            type CallbackFn = unsafe extern "C" fn(i32, i32, *const i8);
            type SetCallbackFn = unsafe extern "C" fn(CallbackFn, *mut std::ffi::c_void);
            let set_callback: Symbol<SetCallbackFn> =
                lib.get(b"SetCallbackForReturnOptimizationResult")?;
            set_callback(mode_callback, client);

            Ok(Self { lib, client })
        }
    }

    pub fn get_current_mode(&self) -> Option<SplendidMode> {
        unsafe {
            type GetColorModeFn = unsafe extern "C" fn(*mut std::ffi::c_void) -> i64;
            let get_color_mode: Symbol<GetColorModeFn> =
                self.lib.get(b"MyOptGetSplendidColorModeFunc").ok()?;

            get_color_mode(self.client);

            // Wait for callback
            std::thread::sleep(std::time::Duration::from_millis(500));

            // callback populated the global values by now
            let mode = CURRENT_MODE.load(Ordering::SeqCst);
            let mono = IS_MONOCHROME.load(Ordering::SeqCst);
            // just prints the values, the fn name could be misleading, it just
            // detects the mode from the values that we had from callback
            SplendidMode::from_callback(mode, mono)
        }
    }

    fn store_slider_value_in_global(&self, symbol: &[u8]) -> Result<i64, SliderError> {
        unsafe {
            type GetSliderFn = unsafe extern "C" fn(*mut std::ffi::c_void) -> i64;
            let get_slider_fn: Symbol<GetSliderFn> = self.lib.get(symbol)?;

            Ok(get_slider_fn(self.client))
        }
    }

    pub fn get_manual_sliding(&self) -> Result<i64, SliderError> {
        self.store_slider_value_in_global(b"MyOptGetSplendidManualModeFunc")
    }

    pub fn get_eyecare_sliding(&self) -> Result<i64, SliderError> {
        self.store_slider_value_in_global(b"MyOptGetSplendidEyecareModeFunc")
    }

    pub fn get_ereading_value(&self) -> Result<(i32, i32), SliderError> {
        let _get_mono = self.store_slider_value_in_global(b"MyOptGetSplendidMonochromeFunc")?;

        let grayscale = EREADING_GRAYSCALE.load(Ordering::SeqCst);
        let temp = EREADING_TEMP.load(Ordering::SeqCst);
        Ok((grayscale, temp))
    }

    fn set_mode_helper(&self, value: u8, mode: SplendidMode) -> Result<(), SliderError> {
        let symbol: &[u8] = match mode {
            SplendidMode::Normal | SplendidMode::Vivid => b"MyOptSetSplendidFunc",
            SplendidMode::Manual(_) => b"MyOptSetSplendidManualFunc",
            SplendidMode::EyeCare(_) => b"MyOptSetSplendidEyecareFunc",
            SplendidMode::EReading { .. } => unreachable!(),
        };

        type SetModeFn = unsafe extern "C" fn(u8, *const i8, *mut std::ffi::c_void) -> i64;
        let set_fn: Symbol<SetModeFn> = unsafe { self.lib.get(symbol) }?;
        let empty_str = b"\0".as_ptr() as *const i8;
        unsafe { set_fn(value, empty_str, self.client) };
        Ok(())
    }

    // Update set_mode to restore slider values
    pub fn set_mode(&self, mode: SplendidMode) -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            match mode {
                SplendidMode::Normal => self.set_mode_helper(1, mode)?,
                SplendidMode::Vivid => self.set_mode_helper(2, mode)?,
                SplendidMode::Manual(slider) => {
                    self.set_mode_helper(slider, mode)?;
                }
                SplendidMode::EyeCare(level) => {
                    self.set_mode_helper(level, mode)?;
                }
                SplendidMode::EReading { grayscale, temp } => {
                    type SetMonoFn = unsafe extern "C" fn(i32, *mut std::ffi::c_void) -> i64;
                    let set_mono: Symbol<SetMonoFn> =
                        self.lib.get(b"MyOptSetSplendidMonochromeFunc")?;
                    let value = (grayscale as i32 * 256) + temp as i32 - 206;
                    set_mono(value, self.client);
                }
            }
            Ok(())
        }
    }

    pub fn toggle_e_reading(&self) -> Result<SplendidMode, Box<dyn std::error::Error>> {
        let current = self.get_current_mode();
        println!("Current mode: {:?}", current);

        match current {
            Some(SplendidMode::EReading { .. }) => {
                // Switch back to last mode or Normal
                let last = LAST_NON_EREADING_MODE.load(Ordering::SeqCst);
                let target = SplendidMode::from_mode_id(last);
                println!("Switching from E-Reading to {:?}", target);
                self.set_mode(target)?;
                Ok(target)
            }
            Some(mode) => {
                println!("Switching from {:?} to E-Reading", mode);
                let ereading = SplendidMode::EReading {
                    grayscale: EREADING_GRAYSCALE.load(Ordering::SeqCst) as u8,
                    temp: EREADING_TEMP.load(Ordering::SeqCst) as u8,
                };
                self.set_mode(ereading)?;
                Ok(ereading)
            }
            None => {
                println!("Unknown mode, switching to E-Reading");
                let ereading = SplendidMode::EReading {
                    grayscale: EREADING_GRAYSCALE.load(Ordering::SeqCst) as u8,
                    temp: EREADING_TEMP.load(Ordering::SeqCst) as u8,
                };
                self.set_mode(ereading)?;
                Ok(ereading)
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let controller = AsusController::new().expect("Failed to create controller");

    controller.get_manual_sliding()?;
    controller.get_eyecare_sliding()?;
    controller.get_ereading_value()?;

    // Toggle e-reading mode
    match controller.toggle_e_reading() {
        Ok(new_mode) => println!("Toggled to: {:?}", new_mode),
        Err(e) => println!("Error: {}", e),
    }
    Ok(())
}

fn find_asus_package() -> Result<String, u32> {
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
        return Err(result);
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
        return Err(result);
    }

    let full_name = unsafe {
        let ptr = package_names[0];
        let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    };

    Ok(full_name)
}

fn get_package_path(full_name: &str) -> Result<String, u32> {
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
        return Err(result);
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
        return Err(result);
    }

    let len = buffer.iter().take_while(|&&c| c != 0).count();
    Ok(String::from_utf16_lossy(&buffer[..len]))
}
