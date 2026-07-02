use core::cell::RefCell;

use critical_section::Mutex;
use embedded_storage::ReadStorage;
use esp_hal::peripherals::FLASH;
use esp_storage::{FlashStorage, FlashStorageError};

static STORAGE: Mutex<RefCell<Option<FlashStorage<'static>>>> = Mutex::new(RefCell::new(None));

#[derive(Clone, Copy, Debug, defmt::Format)]
pub enum Error {
    NotInitialized,
    Storage(FlashStorageError),
}

pub fn init(flash: FLASH<'static>) {
    critical_section::with(|cs| {
        STORAGE.borrow_ref_mut(cs).replace(FlashStorage::new(flash));
    });
}

pub fn read(offset: u32, buffer: &mut [u8]) -> Result<(), Error> {
    critical_section::with(|cs| {
        let mut storage = STORAGE.borrow_ref_mut(cs);
        let storage = storage.as_mut().ok_or(Error::NotInitialized)?;
        storage.read(offset, buffer).map_err(Error::Storage)
    })
}
