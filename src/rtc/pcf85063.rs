use crate::bus::SharedI2cBus;
use esp_hal::i2c::master::Error;

const ADDRESS: u8 = 0x51;
const REG_CTRL1: u8 = 0x00;
const REG_SECONDS: u8 = 0x04;
const CTRL1_STOP: u8 = 1 << 5;
const DAYS_PER_100_YEARS: i64 = 36_525;
const MINUTES_PER_DAY: i64 = 24 * 60;
const SECONDS_PER_DAY: i64 = MINUTES_PER_DAY * 60;
const SECONDS_PER_100_YEARS: i64 = DAYS_PER_100_YEARS * SECONDS_PER_DAY;

#[derive(Clone, Copy)]
pub struct Pcf85063 {
    i2c: SharedI2cBus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub weekday: u8,
    pub clock_integrity: bool,
}

#[derive(Clone, Copy)]
pub enum ClockField {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
}

impl Pcf85063 {
    pub fn new(i2c: SharedI2cBus) -> Self {
        Self { i2c }
    }

    pub fn start(&self) -> Result<(), Error> {
        let mut ctrl = [0u8; 1];
        self.i2c
            .borrow_mut()
            .write_read(ADDRESS, &[REG_CTRL1], &mut ctrl)?;
        self.i2c
            .borrow_mut()
            .write(ADDRESS, &[REG_CTRL1, ctrl[0] & !CTRL1_STOP])
    }

    pub fn read_datetime(&self) -> Result<DateTime, Error> {
        let mut buffer = [0u8; 7];
        self.i2c
            .borrow_mut()
            .write_read(ADDRESS, &[REG_SECONDS], &mut buffer)?;

        Ok(DateTime {
            year: 2000 + bcd_to_dec(buffer[6]) as u16,
            month: bcd_to_dec(buffer[5] & 0x1f),
            day: bcd_to_dec(buffer[3] & 0x3f),
            hour: bcd_to_dec(buffer[2] & 0x3f),
            minute: bcd_to_dec(buffer[1] & 0x7f),
            second: bcd_to_dec(buffer[0] & 0x7f),
            weekday: bcd_to_dec(buffer[4] & 0x07),
            clock_integrity: buffer[0] & 0x80 == 0,
        })
    }

    pub fn write_datetime(&self, datetime: DateTime) -> Result<(), Error> {
        let buffer = [
            dec_to_bcd(datetime.second) & 0x7f,
            dec_to_bcd(datetime.minute),
            dec_to_bcd(datetime.hour),
            dec_to_bcd(datetime.day),
            weekday(datetime.year, datetime.month, datetime.day),
            dec_to_bcd(datetime.month),
            dec_to_bcd((datetime.year % 100) as u8),
        ];

        self.i2c.borrow_mut().write(
            ADDRESS,
            &[
                REG_SECONDS,
                buffer[0],
                buffer[1],
                buffer[2],
                buffer[3],
                buffer[4],
                buffer[5],
                buffer[6],
            ],
        )
    }

    pub fn adjust_minutes(&self, minutes: i32) -> Result<DateTime, Error> {
        let mut datetime = self.read_datetime()?;
        datetime.add_seconds(minutes as i64 * 60);
        self.write_datetime(datetime)?;
        Ok(datetime)
    }

    pub fn adjust_field(&self, field: ClockField, delta: i32) -> Result<DateTime, Error> {
        let datetime = self.read_datetime()?.adjusted(field, delta);
        self.write_datetime(datetime)?;
        Ok(datetime)
    }
}

impl DateTime {
    pub fn adjusted(mut self, field: ClockField, delta: i32) -> Self {
        self.adjust_field(field, delta);
        self
    }

    fn adjust_field(&mut self, field: ClockField, delta: i32) {
        self.normalize();

        match field {
            ClockField::Year => self.add_years(delta),
            ClockField::Month => self.add_months(delta),
            ClockField::Day => self.add_seconds(delta as i64 * SECONDS_PER_DAY),
            ClockField::Hour => self.add_seconds(delta as i64 * 60 * 60),
            ClockField::Minute => self.add_seconds(delta as i64 * 60),
            ClockField::Second => self.add_seconds(delta as i64),
        }

        self.clock_integrity = true;
    }

    fn normalize(&mut self) {
        self.year = 2000 + self.year.saturating_sub(2000) % 100;
        self.month = self.month.clamp(1, 12);
        self.day = self.day.clamp(1, days_in_month(self.year, self.month));
        self.hour = self.hour.min(23);
        self.minute = self.minute.min(59);
        self.second = self.second.min(59);
        self.weekday = weekday(self.year, self.month, self.day);
    }

    fn add_years(&mut self, years: i32) {
        let year = (self.year as i32 - 2000 + years).rem_euclid(100);
        self.year = 2000 + year as u16;
        self.day = self.day.min(days_in_month(self.year, self.month));
        self.weekday = weekday(self.year, self.month, self.day);
    }

    fn add_months(&mut self, months: i32) {
        let month = (self.year as i32 - 2000) * 12 + self.month as i32 - 1 + months;
        let month = month.rem_euclid(100 * 12);
        self.year = 2000 + (month / 12) as u16;
        self.month = (month % 12 + 1) as u8;
        self.day = self.day.min(days_in_month(self.year, self.month));
        self.weekday = weekday(self.year, self.month, self.day);
    }

    fn add_seconds(&mut self, seconds: i64) {
        let day_index = days_since_2000(self.year, self.month, self.day);
        let second_of_day =
            self.hour as i64 * 60 * 60 + self.minute as i64 * 60 + self.second as i64;
        let total = (day_index * SECONDS_PER_DAY + second_of_day + seconds)
            .rem_euclid(SECONDS_PER_100_YEARS);
        let day_index = total / SECONDS_PER_DAY;
        let second_of_day = total % SECONDS_PER_DAY;
        let (year, month, day) = date_from_days_since_2000(day_index);

        self.year = year;
        self.month = month;
        self.day = day;
        self.hour = (second_of_day / (60 * 60)) as u8;
        self.minute = (second_of_day / 60 % 60) as u8;
        self.second = (second_of_day % 60) as u8;
        self.weekday = weekday(year, month, day);
    }
}

fn bcd_to_dec(value: u8) -> u8 {
    (value >> 4) * 10 + (value & 0x0f)
}

fn dec_to_bcd(value: u8) -> u8 {
    ((value / 10) << 4) | (value % 10)
}

fn is_leap_year(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 31,
    }
}

fn days_since_2000(year: u16, month: u8, day: u8) -> i64 {
    let year_days = (2000..year)
        .map(|current| if is_leap_year(current) { 366 } else { 365 })
        .sum::<i64>();
    let month_days = (1..month)
        .map(|current| days_in_month(year, current) as i64)
        .sum::<i64>();

    year_days + month_days + day as i64 - 1
}

fn date_from_days_since_2000(mut days: i64) -> (u16, u8, u8) {
    let mut year = 2000;
    while days >= if is_leap_year(year) { 366 } else { 365 } {
        days -= if is_leap_year(year) { 366 } else { 365 };
        year += 1;
    }

    let mut month = 1;
    while days >= days_in_month(year, month) as i64 {
        days -= days_in_month(year, month) as i64;
        month += 1;
    }

    (year, month, (days + 1) as u8)
}

fn weekday(year: u16, mut month: u8, day: u8) -> u8 {
    let mut year = year as u32;
    if month < 3 {
        month += 12;
        year -= 1;
    }

    let value = (day as u32
        + ((month as u32 + 1) * 26) / 10
        + year
        + year / 4
        + 6 * (year / 100)
        + year / 400)
        % 7;

    if value == 0 { 6 } else { (value - 1) as u8 }
}
