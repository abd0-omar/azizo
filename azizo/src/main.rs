use std::sync::Arc;

use azizo_core::{
    AsusController, ControllerError, DisplayController, EReadingMode, EyeCareMode, ManualMode,
    NormalMode, VividMode,
};
use iced::keyboard::{self, Event as KeyboardEvent, Key};
use iced::widget::{button, column, container, row, slider, text, toggler};
use iced::{Element, Subscription, Task, Theme};

pub fn main() -> iced::Result {
    iced::application(AzizoApp::default, AzizoApp::update, AzizoApp::view)
        .title("Azizo - ASUS Display Control")
        .subscription(AzizoApp::subscription)
        .theme(AzizoApp::theme)
        .run()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModeType {
    Normal,
    Vivid,
    Manual,
    EyeCare,
}

struct AzizoApp {
    controller: Option<Arc<AsusController>>,
    error_message: Option<String>,

    // Display state
    dimming_percent: i32,
    current_mode: ModeType,
    is_ereading: bool,

    // Mode sliders
    manual_value: i32,
    eyecare_level: i32,
    ereading_grayscale: i32,
    ereading_temp: i32,
}

#[derive(Debug, Clone)]
enum Message {
    // Dimming
    DimmingChanged(i32),
    IncreaseDimming,
    DecreaseDimming,

    // Mode selection
    SetMode(ModeType),
    ToggleEReading(bool),

    // Mode-specific sliders
    ManualSliderChanged(i32),
    EyeCareSliderChanged(i32),
    EReadingGrayscaleChanged(i32),
    EReadingTempChanged(i32),

    // Sync
    SyncFromHardware,

    // Keyboard event
    KeyboardEvent(KeyboardEvent),
}

impl Default for AzizoApp {
    fn default() -> Self {
        let mut app = Self {
            controller: None,
            error_message: None,
            dimming_percent: 100,
            current_mode: ModeType::Normal,
            is_ereading: false,
            manual_value: 50,
            eyecare_level: 2,
            ereading_grayscale: 3,
            ereading_temp: 60,
        };

        // Try to initialize controller
        match AsusController::new() {
            Ok(controller) => {
                let controller = Arc::new(controller);

                // Sync initial state
                if let Err(e) = controller.sync_all_sliders() {
                    app.error_message = Some(format!("Sync error: {}", e));
                } else {
                    let state = controller.get_state();
                    app.dimming_percent = AsusController::dimming_to_percent(state.dimming);
                    app.manual_value = state.manual_slider as i32;
                    app.eyecare_level = state.eyecare_level as i32;
                    app.ereading_grayscale = state.ereading_grayscale as i32;
                    app.ereading_temp = state.ereading_temp as i32;
                    app.is_ereading = state.is_monochrome;

                    // Determine current mode
                    app.current_mode = match state.mode_id {
                        1 => ModeType::Normal,
                        2 => ModeType::Vivid,
                        6 => ModeType::Manual,
                        7 => ModeType::EyeCare,
                        _ => ModeType::Normal,
                    };
                }

                app.controller = Some(controller);
            }
            Err(e) => {
                app.error_message = Some(format!("Failed to initialize: {}", e));
            }
        }

        app
    }
}

impl AzizoApp {
    fn update(&mut self, message: Message) -> Task<Message> {
        // Clear previous errors on new actions
        if !matches!(
            message,
            Message::SyncFromHardware | Message::KeyboardEvent(_)
        ) {
            self.error_message = None;
        }

        match message {
            Message::DimmingChanged(value) => {
                self.dimming_percent = value;
                if let Some(ref controller) = self.controller {
                    if let Err(e) = controller.set_dimming_percent(value) {
                        self.error_message = Some(format!("Dimming error: {}", e));
                    }
                }
            }

            Message::IncreaseDimming => {
                let new_value = (self.dimming_percent + 10).min(100);
                return self.update(Message::DimmingChanged(new_value));
            }

            Message::DecreaseDimming => {
                let new_value = (self.dimming_percent - 10).max(0);
                return self.update(Message::DimmingChanged(new_value));
            }

            Message::SetMode(mode) => {
                self.current_mode = mode;
                if let Some(ref controller) = self.controller {
                    let result: Result<(), ControllerError> = match mode {
                        ModeType::Normal => controller.set_mode(&NormalMode::new()),
                        ModeType::Vivid => controller.set_mode(&VividMode::new()),
                        ModeType::Manual => match ManualMode::new(self.manual_value as u8) {
                            Ok(m) => controller.set_mode(&m),
                            Err(e) => Err(e),
                        },
                        ModeType::EyeCare => match EyeCareMode::new(self.eyecare_level as u8) {
                            Ok(m) => controller.set_mode(&m),
                            Err(e) => Err(e),
                        },
                    };
                    if let Err(e) = result {
                        self.error_message = Some(format!("Mode error: {}", e));
                    }
                }
            }

            Message::ToggleEReading(enabled) => {
                self.is_ereading = enabled;
                if let Some(ref controller) = self.controller {
                    if enabled {
                        // Enable e-reading
                        match EReadingMode::new(
                            self.ereading_grayscale as u8,
                            self.ereading_temp as u8,
                        ) {
                            Ok(mode) => {
                                if let Err(e) = controller.set_mode(&mode) {
                                    self.error_message = Some(format!("E-Reading error: {}", e));
                                }
                            }
                            Err(e) => {
                                self.error_message = Some(format!("E-Reading error: {}", e));
                            }
                        }
                    } else {
                        // Disable - restore previous mode
                        if let Err(e) = controller.toggle_e_reading() {
                            self.error_message = Some(format!("E-Reading toggle error: {}", e));
                        }
                    }
                }
            }

            Message::ManualSliderChanged(value) => {
                self.manual_value = value;
                if self.current_mode == ModeType::Manual {
                    if let Some(ref controller) = self.controller {
                        if let Ok(mode) = ManualMode::new(value as u8) {
                            if let Err(e) = controller.set_mode(&mode) {
                                self.error_message = Some(format!("Manual error: {}", e));
                            }
                        }
                    }
                }
            }

            Message::EyeCareSliderChanged(value) => {
                self.eyecare_level = value;
                if self.current_mode == ModeType::EyeCare {
                    if let Some(ref controller) = self.controller {
                        if let Ok(mode) = EyeCareMode::new(value as u8) {
                            if let Err(e) = controller.set_mode(&mode) {
                                self.error_message = Some(format!("EyeCare error: {}", e));
                            }
                        }
                    }
                }
            }

            Message::EReadingGrayscaleChanged(value) => {
                self.ereading_grayscale = value;
                if self.is_ereading {
                    if let Some(ref controller) = self.controller {
                        if let Ok(mode) = EReadingMode::new(value as u8, self.ereading_temp as u8) {
                            if let Err(e) = controller.set_mode(&mode) {
                                self.error_message = Some(format!("E-Reading error: {}", e));
                            }
                        }
                    }
                }
            }

            Message::EReadingTempChanged(value) => {
                self.ereading_temp = value;
                if self.is_ereading {
                    if let Some(ref controller) = self.controller {
                        if let Ok(mode) =
                            EReadingMode::new(self.ereading_grayscale as u8, value as u8)
                        {
                            if let Err(e) = controller.set_mode(&mode) {
                                self.error_message = Some(format!("E-Reading error: {}", e));
                            }
                        }
                    }
                }
            }

            Message::SyncFromHardware => {
                if let Some(ref controller) = self.controller {
                    match controller.sync_all_sliders() {
                        Ok(()) => {
                            let state = controller.get_state();
                            self.dimming_percent =
                                AsusController::dimming_to_percent(state.dimming);
                            self.manual_value = state.manual_slider as i32;
                            self.eyecare_level = state.eyecare_level as i32;
                            self.ereading_grayscale = state.ereading_grayscale as i32;
                            self.ereading_temp = state.ereading_temp as i32;
                            self.is_ereading = state.is_monochrome;

                            self.current_mode = match state.mode_id {
                                1 => ModeType::Normal,
                                2 => ModeType::Vivid,
                                6 => ModeType::Manual,
                                7 => ModeType::EyeCare,
                                _ => ModeType::Normal,
                            };
                            self.error_message = Some("Synced!".to_string());
                        }
                        Err(e) => {
                            self.error_message = Some(format!("Sync error: {}", e));
                        }
                    }
                }
            }

            Message::KeyboardEvent(event) => {
                if let KeyboardEvent::KeyPressed { key, modifiers, .. } = event {
                    // Check for Ctrl+Shift+Win (Logo) modifier combination
                    let has_modifiers =
                        modifiers.control() && modifiers.shift() && modifiers.logo();

                    if has_modifiers {
                        match key.as_ref() {
                            Key::Character(c) if c == "." || c == ">" => {
                                return self.update(Message::IncreaseDimming);
                            }
                            Key::Character(c) if c == "," || c == "<" => {
                                return self.update(Message::DecreaseDimming);
                            }
                            Key::Character(c) if c == "/" => {
                                return self.update(Message::SyncFromHardware);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let title = text("Azizo - ASUS Display Control").size(24);

        // Error/status message
        let status = if let Some(ref msg) = self.error_message {
            text(msg).size(14)
        } else {
            text("").size(14)
        };

        // Dimming slider
        let dimming_section = column![
            text(format!("Dimming: {}%", self.dimming_percent)).size(16),
            slider(0..=100, self.dimming_percent, Message::DimmingChanged).step(10),
        ]
        .spacing(5);

        // Mode buttons
        let mode_buttons = row![
            mode_button("Normal", ModeType::Normal, self.current_mode),
            mode_button("Vivid", ModeType::Vivid, self.current_mode),
            mode_button("Manual", ModeType::Manual, self.current_mode),
            mode_button("Eye Care", ModeType::EyeCare, self.current_mode),
        ]
        .spacing(10);

        // Manual slider (only shown when Manual mode is selected)
        let manual_section = if self.current_mode == ModeType::Manual {
            column![
                text(format!("Manual Temperature: {}", self.manual_value)).size(14),
                slider(0..=100, self.manual_value, Message::ManualSliderChanged).step(1),
            ]
            .spacing(5)
        } else {
            column![]
        };

        // Eye Care slider (only shown when EyeCare mode is selected)
        let eyecare_section = if self.current_mode == ModeType::EyeCare {
            column![
                text(format!("Eye Care Level: {}", self.eyecare_level)).size(14),
                slider(0..=4, self.eyecare_level, Message::EyeCareSliderChanged).step(1),
            ]
            .spacing(5)
        } else {
            column![]
        };

        // E-Reading toggle and sliders
        let ereading_section = column![
            toggler(self.is_ereading)
                .label("E-Reading Mode")
                .on_toggle(Message::ToggleEReading),
        ]
        .spacing(5);

        let ereading_sliders = if self.is_ereading {
            column![
                text(format!("Grayscale: {}", self.ereading_grayscale)).size(14),
                slider(
                    0..=4,
                    self.ereading_grayscale,
                    Message::EReadingGrayscaleChanged
                )
                .step(1),
                text(format!("Temperature: {}", self.ereading_temp)).size(14),
                slider(0..=100, self.ereading_temp, Message::EReadingTempChanged).step(1),
            ]
            .spacing(5)
        } else {
            column![]
        };

        // Sync button
        let sync_button = button("Sync from Hardware").on_press(Message::SyncFromHardware);

        // Keyboard shortcuts hint
        let shortcuts_hint =
            text("Shortcuts: Ctrl+Shift+Win+< / > (dimming) | Ctrl+Shift+Win+/ (sync)").size(12);

        // Main layout
        let content = column![
            title,
            status,
            dimming_section,
            text("Mode:").size(16),
            mode_buttons,
            manual_section,
            eyecare_section,
            ereading_section,
            ereading_sliders,
            sync_button,
            shortcuts_hint,
        ]
        .spacing(15)
        .padding(20);

        container(content).into()
    }

    fn subscription(&self) -> Subscription<Message> {
        keyboard::listen().map(Message::KeyboardEvent)
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }
}

fn mode_button(label: &str, mode: ModeType, current: ModeType) -> Element<'_, Message> {
    let btn = button(text(label));
    if mode == current {
        // Selected state - don't allow clicking
        btn.into()
    } else {
        btn.on_press(Message::SetMode(mode)).into()
    }
}
