//! Example: Toggle e-reading mode on/off.
//!
//! Run with: `cargo run --example toggle_ereading`

use azizo_core::{AsusController, ControllerError, DisplayController};

fn main() -> Result<(), ControllerError> {
    // Initialize logging (optional)
    env_logger::init();

    // Create the controller
    let controller = AsusController::new()?;

    // Sync all values from hardware
    println!("Syncing with hardware...");
    controller.sync_all_sliders()?;

    // Get current state
    let state = controller.get_state();
    println!(
        "Current state: mode={}, dimming={}%",
        state.mode_id,
        AsusController::dimming_to_percent(state.dimming)
    );

    // Toggle e-reading mode
    match controller.toggle_e_reading() {
        Ok(new_mode) => println!("Toggled to: {:?}", new_mode),
        Err(e) => eprintln!("Error toggling mode: {}", e),
    }

    Ok(())
}
