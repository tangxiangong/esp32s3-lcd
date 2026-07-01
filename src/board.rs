use crate::{
    lcd::{
        axs15231b::Axs15231b,
        tca9554::{LCD_RST, Tca9554},
        touch::Touch,
    },
    rtc::pcf85063::Pcf85063,
};
use alloc::boxed::Box;
use core::cell::RefCell;
use esp_hal::{
    delay::Delay,
    dma::{DmaRxBuf, DmaTxBuf},
    gpio::{Level, Output, OutputConfig},
    i2c::master::{Config as I2cConfig, I2c},
    peripherals::Peripherals,
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
};

pub struct Board {
    pub display: Axs15231b<'static>,
    pub rtc: Pcf85063,
    pub touch: Touch,
    _backlight_pwm: Output<'static>,
}

impl Board {
    pub fn init(peripherals: Peripherals, delay: &mut Delay) -> Self {
        esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
        esp_alloc::heap_allocator!(size: 64 * 1024);
        esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);

        let i2c = I2c::new(
            peripherals.I2C0,
            I2cConfig::default().with_frequency(Rate::from_khz(400)),
        )
        .expect("I2C0 init failed")
        .with_sda(peripherals.GPIO47)
        .with_scl(peripherals.GPIO48);
        let i2c = Box::leak(Box::new(RefCell::new(i2c)));

        let mut expander = Tca9554::new(i2c);
        let rtc = Pcf85063::new(i2c);
        rtc.start().expect("PCF85063 start failed");
        expander.init_for_lcd().expect("TCA9554 init failed");
        expander
            .reset_lcd(delay)
            .expect("LCD expander reset failed");

        let backlight_pwm = Output::new(peripherals.GPIO42, Level::Low, OutputConfig::default());

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

        let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) =
            esp_hal::dma_buffers!(64, 24000);
        let dma_rx_buf = DmaRxBuf::new(rx_descriptors, rx_buffer).expect("DMA RX buffer failed");
        let dma_tx_buf = DmaTxBuf::new(tx_descriptors, tx_buffer).expect("DMA TX buffer failed");
        let spi = spi
            .with_dma(peripherals.DMA_CH0)
            .with_buffers(dma_rx_buf, dma_tx_buf);

        let mut display = Axs15231b::new(spi);
        display.init(delay).expect("LCD init failed");
        expander.set_high(LCD_RST).expect("LCD reset level failed");
        expander.enable_backlight().expect("LCD backlight failed");

        Self {
            display,
            rtc,
            touch,
            _backlight_pwm: backlight_pwm,
        }
    }
}
