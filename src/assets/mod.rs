mod flash;
mod package;

include!(concat!(env!("OUT_DIR"), "/assets_manifest.rs"));

pub const FONT_ASSET_NAME: &str = "font/unifont16.bmf";

#[derive(Clone, Copy, Debug, defmt::Format)]
pub enum Error {
    Flash(flash::Error),
    InvalidPackage,
    NotFound,
    OutOfBounds,
}

#[derive(Clone, Copy)]
pub struct AssetInfo {
    offset: u32,
    len: u32,
}

pub fn init(flash: esp_hal::peripherals::FLASH<'static>) {
    flash::init(flash);
}

pub fn asset_len(name: &str) -> Result<usize, Error> {
    Ok(find(name)?.len as usize)
}

pub fn read_asset(name: &str, offset: usize, buffer: &mut [u8]) -> Result<(), Error> {
    let asset = find(name)?;
    let end = offset.checked_add(buffer.len()).ok_or(Error::OutOfBounds)?;
    if end > asset.len as usize {
        return Err(Error::OutOfBounds);
    }

    read_package(
        asset
            .offset
            .checked_add(offset as u32)
            .ok_or(Error::OutOfBounds)?,
        buffer,
    )
}

fn find(name: &str) -> Result<AssetInfo, Error> {
    let header = read_header()?;
    let table_len = header.table_len();
    let table_end = header
        .table_offset
        .checked_add(table_len as u32)
        .ok_or(Error::InvalidPackage)?;

    // 资源包来自 build.rs 生成并由 cargo run 烧到 0x800000；这里仍做边界检查，
    // 防止错误烧录或布局变化导致越界读 flash。
    if header.table_offset as usize != package::HEADER_SIZE
        || table_end > header.data_offset
        || header.data_offset as usize > ASSETS_PACKAGE_LEN
        || ASSETS_PACKAGE_LEN > ASSETS_FLASH_CAPACITY
    {
        return Err(Error::InvalidPackage);
    }

    for index in 0..header.entry_count {
        let mut entry = [0u8; package::ENTRY_SIZE];
        let entry_offset = header
            .table_offset
            .checked_add(u32::from(index) * package::ENTRY_SIZE as u32)
            .ok_or(Error::InvalidPackage)?;
        read_package(entry_offset, &mut entry)?;

        if let Some(info) = package::parse_entry(&entry, name, header.data_offset) {
            let end = info
                .offset
                .checked_add(info.len)
                .ok_or(Error::InvalidPackage)?;
            if end as usize > ASSETS_PACKAGE_LEN {
                return Err(Error::InvalidPackage);
            }

            let _crc32 = info.crc32;
            return Ok(AssetInfo {
                offset: info.offset,
                len: info.len,
            });
        }
    }

    Err(Error::NotFound)
}

fn read_header() -> Result<package::Header, Error> {
    let mut header = [0u8; package::HEADER_SIZE];
    read_package(0, &mut header)?;
    package::Header::parse(&header).ok_or(Error::InvalidPackage)
}

fn read_package(offset: u32, buffer: &mut [u8]) -> Result<(), Error> {
    let end = (offset as usize)
        .checked_add(buffer.len())
        .ok_or(Error::OutOfBounds)?;
    if end > ASSETS_PACKAGE_LEN || end > ASSETS_FLASH_CAPACITY {
        return Err(Error::OutOfBounds);
    }

    // offset 是资源包内偏移，实际 flash 地址要叠加构建脚本生成的基地址。
    flash::read(
        ASSETS_FLASH_BASE
            .checked_add(offset)
            .ok_or(Error::OutOfBounds)?,
        buffer,
    )
    .map_err(Error::Flash)
}
