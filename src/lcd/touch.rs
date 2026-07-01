use esp_hal::{Blocking, i2c::master::I2c};

const ADDRESS: u8 = 0x3b;
const READ_COMMAND: [u8; 11] = [
    0xb5, 0xab, 0xa5, 0x5a, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x00,
];

pub struct Touch {
    i2c: I2c<'static, Blocking>,
    pressed: bool,
    last_x: u16,
    last_y: u16,
}

#[derive(Clone, Copy)]
pub struct TouchPoint {
    pub x: u16,
    pub y: u16,
    pub pressed: bool,
}

impl Touch {
    pub fn new(i2c: I2c<'static, Blocking>) -> Self {
        Self {
            i2c,
            pressed: false,
            last_x: 0,
            last_y: 0,
        }
    }

    pub fn read(&mut self) -> Result<Option<TouchPoint>, esp_hal::i2c::master::Error> {
        let mut buffer = [0u8; 32];
        self.i2c.write_read(ADDRESS, &READ_COMMAND, &mut buffer)?;

        let pressed = buffer[1] > 0 && buffer[1] < 5;
        if !pressed {
            if self.pressed {
                self.pressed = false;
                return Ok(Some(TouchPoint {
                    x: self.last_x,
                    y: self.last_y,
                    pressed,
                }));
            }

            return Ok(None);
        }

        self.pressed = true;
        let raw_x = (((buffer[2] as u16) & 0x0f) << 8) | buffer[3] as u16;
        let raw_y = (((buffer[4] as u16) & 0x0f) << 8) | buffer[5] as u16;
        let x = 639u16.saturating_sub(raw_x.min(639));
        let y = 171u16.saturating_sub(raw_y.min(171));
        self.last_x = x;
        self.last_y = y;

        Ok(Some(TouchPoint { x, y, pressed }))
    }
}
