use core::cell::RefCell;
use esp_hal::{Blocking, i2c::master::I2c};

pub type SharedI2cBus = &'static RefCell<I2c<'static, Blocking>>;
