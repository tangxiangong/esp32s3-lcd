#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use defmt::info;
use esp_hal::{clock::CpuClock, delay::Delay, main};
use esp32s3_lcd::{app, board::Board};
use panic_rtt_target as _;

// 生成 ESP-IDF bootloader 需要的应用描述符；缺少它时镜像格式无法被
// ESP bootloader 正确识别。
// 参考：<https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[main]
fn main() -> ! {
    // 当前工程使用 RTT 承载 defmt 日志，不依赖 UART 控制台输出。
    rtt_target::rtt_init_defmt!();
    info!("booting esp32s3 lcd hello world");

    // main 只负责启动和转交外设所有权；具体硬件初始化集中在 Board::init。
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    let mut delay = Delay::new();
    let board = Board::init(peripherals, &mut delay);

    app::run(board)
}
