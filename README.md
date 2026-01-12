# azizo-core

Unofficial Rust API for ASUS Splendid display control on Windows.

## Requirements

- Windows 10/11
- ASUS laptop with Splendid display support
- [ASUS PC Assistant](https://www.asus.com/support/) installed

## Installation


## Usage

```rust
use azizo_core::{AsusController, DisplayController, NormalMode, EyeCareMode};

fn main() -> Result<(), azizo_core::ControllerError> {
    // Create the controller (only one instance allowed)
    let controller = AsusController::new()?;

    // Sync state from hardware
    controller.sync_all_sliders()?;

    // Get current state
    let state = controller.get_state();
    println!("Current dimming: {}%", AsusController::dimming_to_percent(state.dimming));

    // Set a mode
    controller.set_mode(&NormalMode::new())?;

    // Set eye care mode with level 2
    controller.set_mode(&EyeCareMode::new(2)?)?;

    // Toggle e-reading mode
    controller.toggle_e_reading()?;

    // Adjust dimming (0-100%)
    controller.set_dimming_percent(75)?;

    Ok(())
}
```

## Available Modes

| Mode | Description | Parameters |
|------|-------------|------------|
| `NormalMode` | Default color profile | None |
| `VividMode` | Enhanced colors | None |
| `ManualMode` | User-adjustable color temperature | `value: 0-100` |
| `EyeCareMode` | Blue light filter | `level: 0-4` |
| `EReadingMode` | Grayscale for reading | `grayscale: 0-4`, `temp` |

## Testing

Use `MockController` to test without hardware:

```rust
use azizo_core::{MockController, DisplayController, NormalMode};

let mock = MockController::new();
mock.set_mode(&NormalMode::new()).unwrap();
assert_eq!(mock.get_state().mode_id, 1);
```

## Examples

Run the toggle example:

```bash
cargo run --example toggle_ereading
```

## API

### `AsusController`

- `new()` - Create a new controller (only one instance allowed)
- `get_state()` - Get a snapshot of current state
- `set_mode(&mode)` - Set a display mode
- `toggle_e_reading()` - Toggle e-reading mode on/off
- `set_dimming(level)` - Set dimming (40-100 splendid units)
- `set_dimming_percent(percent)` - Set dimming (0-100%)
- `sync_all_sliders()` - Sync all values from hardware
- `refresh_sliders()` - Refresh slider values

### `ControllerState`

Snapshot struct containing:
- `mode_id` - Current mode (1=Normal, 2=Vivid, 6=Manual, 7=EyeCare)
- `is_monochrome` - Whether e-reading mode is active
- `dimming` - Dimming level (40-100)
- `manual_slider` - Manual mode value (0-100)
- `eyecare_level` - Eye care level (0-4)
- `ereading_grayscale` - E-reading grayscale (0-4)
- `ereading_temp` - E-reading temperature

## Limitations

- Requires ASUS PC Assistant to be installed
- Windows only

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
