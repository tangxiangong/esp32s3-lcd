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
        // Slint timer/animation 使用该时间源；实际调度由 app::run 主循环推进。
        Duration::from_micros(Instant::now().duration_since_epoch().as_micros())
    }

    fn run_event_loop(&self) -> Result<(), slint::PlatformError> {
        // 嵌入式固件没有托管事件循环，app::run 会显式更新 timer、触摸和绘制。
        Ok(())
    }
}
