use crate::{
    assets,
    lcd::{
        axs15231b::Axs15231b,
        tca9554::{LCD_RST, Tca9554},
        touch::Touch,
    },
    radio::Wireless,
    rtc::pcf85063::Pcf85063,
};
use alloc::boxed::Box;
use core::cell::RefCell;
use esp_hal::{
    delay::Delay,
    dma::{DmaRxBuf, DmaTxBuf},
    gpio::{Level, Output, OutputConfig},
    i2c::master::{Config as I2cConfig, I2c},
    interrupt::software::SoftwareInterruptControl,
    peripherals::Peripherals,
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
    timer::timg::TimerGroup,
};

pub struct Board {
    pub display: Axs15231b<'static>,
    pub rtc: Pcf85063,
    pub touch: Touch,
    pub wireless: Wireless,
    _backlight_pwm: Output<'static>,
}

impl Board {
    pub fn init(peripherals: Peripherals, delay: &mut Delay) -> Self {
        // Slint 软件渲染、帧缓冲和无线栈都需要堆内存；PSRAM 也在这里接入
        // allocator，避免应用层再直接持有底层外设。
        esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
        esp_alloc::heap_allocator!(size: 64 * 1024);
        esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);
        assets::init(peripherals.FLASH);

        // esp-radio/bleps 依赖 esp-rtos 的时间和软件中断支持，必须在无线
        // 外设初始化前启动。
        let timg0 = TimerGroup::new(peripherals.TIMG0);
        let software_interrupt = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
        esp_rtos::start(timg0.timer0, software_interrupt.software_interrupt0);

        // GPIO47/GPIO48 是板上共享 I2C，总线上挂有 TCA9554、PCF85063，
        // 以及硬件上预留的 IMU/音频控制器件。
        let i2c = I2c::new(
            peripherals.I2C0,
            I2cConfig::default().with_frequency(Rate::from_khz(400)),
        )
        .expect("I2C0 init failed")
        .with_sda(peripherals.GPIO47)
        .with_scl(peripherals.GPIO48);
        // 多个驱动共享同一个阻塞 I2C 外设；泄漏成 'static 是嵌入式单例
        // 资源的有意设计，生命周期等同于固件运行周期。
        let i2c = Box::leak(Box::new(RefCell::new(i2c)));

        let mut expander = Tca9554::new(i2c);
        let rtc = Pcf85063::new(i2c);
        rtc.start().expect("PCF85063 start failed");
        // LCD 上电后先通过 IO 扩展器配置复位/背光相关输出，再执行面板复位。
        expander.init_for_lcd().expect("TCA9554 init failed");
        expander
            .reset_lcd(delay)
            .expect("LCD expander reset failed");

        // 当前代码保留 GPIO42 输出句柄，避免该引脚被释放；实际背光使能
        // 仍由 TCA9554 的 BL_EN 位完成。
        let backlight_pwm = Output::new(peripherals.GPIO42, Level::Low, OutputConfig::default());

        // 触摸控制器使用独立 I2C1，避免与 RTC/EXIO 共享总线轮询互相影响。
        let touch_i2c = I2c::new(
            peripherals.I2C1,
            I2cConfig::default().with_frequency(Rate::from_khz(300)),
        )
        .expect("I2C1 init failed")
        .with_sda(peripherals.GPIO17)
        .with_scl(peripherals.GPIO18);
        let touch = Touch::new(touch_i2c);

        let spi = Spi::new(
            peripherals.SPI3,
            SpiConfig::default()
                .with_frequency(Rate::from_mhz(40))
                .with_mode(Mode::_3),
        )
        .expect("SPI3 init failed")
        .with_sck(peripherals.GPIO10)
        .with_sio0(peripherals.GPIO11)
        .with_sio1(peripherals.GPIO12)
        .with_sio2(peripherals.GPIO13)
        .with_sio3(peripherals.GPIO14)
        .with_cs(peripherals.GPIO9);

        // LCD flush 以块为单位发送 RGB565 数据；DMA 缓冲区大小需要和
        // axs15231b.rs 中的 NATIVE_CHUNK_* 常量一起评估。
        let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) =
            esp_hal::dma_buffers!(64, 24000);
        let dma_rx_buf = DmaRxBuf::new(rx_descriptors, rx_buffer).expect("DMA RX buffer failed");
        let dma_tx_buf = DmaTxBuf::new(tx_descriptors, tx_buffer).expect("DMA TX buffer failed");
        let spi = spi
            .with_dma(peripherals.DMA_CH0)
            .with_buffers(dma_rx_buf, dma_tx_buf);

        let mut display = Axs15231b::new(spi);
        display.init(delay).expect("LCD init failed");
        // 初始化命令完成后再释放复位并打开背光，避免用户看到未初始化画面。
        expander.set_high(LCD_RST).expect("LCD reset level failed");
        expander.enable_backlight().expect("LCD backlight failed");

        // 无线初始化失败不会阻止显示/RTC 功能启动，错误在 Wireless 内记录。
        let wireless = Wireless::new(peripherals.WIFI, peripherals.BT);

        Self {
            display,
            rtc,
            touch,
            wireless,
            _backlight_pwm: backlight_pwm,
        }
    }
}
