pub const HEADER_SIZE: usize = 20;
pub const ENTRY_SIZE: usize = 48;
pub const MAGIC: &[u8; 8] = b"ESPAST01";
pub const VERSION: u16 = 1;

const ENTRY_NAME_SIZE: usize = 32;
const ENTRY_OFFSET_OFFSET: usize = 32;
const ENTRY_LEN_OFFSET: usize = 36;
const ENTRY_CRC_OFFSET: usize = 40;

#[derive(Clone, Copy)]
pub struct Header {
    pub entry_count: u16,
    pub table_offset: u32,
    pub data_offset: u32,
}

#[derive(Clone, Copy)]
pub struct AssetInfo {
    pub offset: u32,
    pub len: u32,
    pub crc32: u32,
}

impl Header {
    pub fn parse(bytes: &[u8; HEADER_SIZE]) -> Option<Self> {
        if &bytes[..MAGIC.len()] != MAGIC {
            return None;
        }

        let version = u16::from_le_bytes([bytes[8], bytes[9]]);
        if version != VERSION {
            return None;
        }

        Some(Self {
            entry_count: u16::from_le_bytes([bytes[10], bytes[11]]),
            table_offset: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            data_offset: u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
        })
    }

    pub fn table_len(&self) -> usize {
        usize::from(self.entry_count) * ENTRY_SIZE
    }
}

pub fn parse_entry(bytes: &[u8; ENTRY_SIZE], name: &str, data_offset: u32) -> Option<AssetInfo> {
    let raw_name = &bytes[..ENTRY_NAME_SIZE];
    let len = raw_name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(raw_name.len());

    if &raw_name[..len] != name.as_bytes() {
        return None;
    }

    let relative_offset = u32::from_le_bytes([
        bytes[ENTRY_OFFSET_OFFSET],
        bytes[ENTRY_OFFSET_OFFSET + 1],
        bytes[ENTRY_OFFSET_OFFSET + 2],
        bytes[ENTRY_OFFSET_OFFSET + 3],
    ]);
    let len = u32::from_le_bytes([
        bytes[ENTRY_LEN_OFFSET],
        bytes[ENTRY_LEN_OFFSET + 1],
        bytes[ENTRY_LEN_OFFSET + 2],
        bytes[ENTRY_LEN_OFFSET + 3],
    ]);
    let crc32 = u32::from_le_bytes([
        bytes[ENTRY_CRC_OFFSET],
        bytes[ENTRY_CRC_OFFSET + 1],
        bytes[ENTRY_CRC_OFFSET + 2],
        bytes[ENTRY_CRC_OFFSET + 3],
    ]);

    Some(AssetInfo {
        offset: data_offset.checked_add(relative_offset)?,
        len,
        crc32,
    })
}
