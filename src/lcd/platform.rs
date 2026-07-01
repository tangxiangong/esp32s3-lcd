extern crate alloc;

use alloc::rc::Rc;
use core::time::Duration;
use esp_hal::time::Instant;
use slint::platform::{Platform, WindowAdapter, software_renderer::MinimalSoftwareWindow};

pub struct EspPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl EspPlatform {
    pub fn new(window: Rc<MinimalSoftwareWindow>) -> Self {
        Self { window }
    }
}

impl Platform for EspPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        Duration::from_micros(Instant::now().duration_since_epoch().as_micros())
    }

    fn run_event_loop(&self) -> Result<(), slint::PlatformError> {
        Ok(())
    }
}
