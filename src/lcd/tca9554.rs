use esp_hal::{Blocking, delay::Delay, i2c::master::I2c};

pub const BL_EN: u8 = 1 << 1;
pub const LCD_RST: u8 = 1 << 5;

const ADDRESS: u8 = 0x20;
const REG_OUTPUT: u8 = 0x01;
const REG_POLARITY: u8 = 0x02;
const REG_CONFIG: u8 = 0x03;

pub struct Tca9554<'d> {
    i2c: I2c<'d, Blocking>,
    output: u8,
    config: u8,
}

impl<'d> Tca9554<'d> {
    pub fn new(i2c: I2c<'d, Blocking>) -> Self {
        Self {
            i2c,
            output: 0xff,
            config: 0xff,
        }
    }

    pub fn init_for_lcd(&mut self) -> Result<(), esp_hal::i2c::master::Error> {
        self.config &= !(BL_EN | LCD_RST);
        self.output &= !BL_EN;
        self.output |= LCD_RST;

        self.i2c.write(ADDRESS, &[REG_POLARITY, 0x00])?;
        self.i2c.write(ADDRESS, &[REG_OUTPUT, self.output])?;
        self.i2c.write(ADDRESS, &[REG_CONFIG, self.config])
    }

    pub fn reset_lcd(&mut self, delay: &mut Delay) -> Result<(), esp_hal::i2c::master::Error> {
        self.set_high(LCD_RST)?;
        delay.delay_millis(30);
        self.set_low(LCD_RST)?;
        delay.delay_millis(250);
        self.set_high(LCD_RST)?;
        delay.delay_millis(30);
        Ok(())
    }

    pub fn enable_backlight(&mut self) -> Result<(), esp_hal::i2c::master::Error> {
        self.set_high(BL_EN)
    }

    pub fn set_high(&mut self, mask: u8) -> Result<(), esp_hal::i2c::master::Error> {
        self.output |= mask;
        self.write_output()
    }

    fn set_low(&mut self, mask: u8) -> Result<(), esp_hal::i2c::master::Error> {
        self.output &= !mask;
        self.write_output()
    }

    fn write_output(&mut self) -> Result<(), esp_hal::i2c::master::Error> {
        self.i2c.write(ADDRESS, &[REG_OUTPUT, self.output])
    }
}
