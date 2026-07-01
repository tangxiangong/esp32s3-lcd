use crate::lcd::{
    axs15231b::Axs15231b,
    tca9554::{LCD_RST, Tca9554},
};
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

        let mut expander = Tca9554::new(i2c);
        expander.init_for_lcd().expect("TCA9554 init failed");
        expander
            .reset_lcd(delay)
            .expect("LCD expander reset failed");

        let backlight_pwm = Output::new(peripherals.GPIO42, Level::Low, OutputConfig::default());

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
            _backlight_pwm: backlight_pwm,
        }
    }
}
