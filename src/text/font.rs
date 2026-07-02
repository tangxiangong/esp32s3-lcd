use core::cell::RefCell;

use critical_section::Mutex;
use defmt::{error, info};

use crate::assets::{self, FONT_ASSET_NAME};

const RECORD_SIZE: usize = 35;
const WIDTH_OFFSET: usize = 2;
const BITMAP_OFFSET: usize = 3;
const GLYPH_HEIGHT: usize = 16;
const GLYPH_ROW_SIZE: usize = 2;
const GLYPH_BITMAP_SIZE: usize = GLYPH_HEIGHT * GLYPH_ROW_SIZE;

static FONT_LEN: Mutex<RefCell<Option<usize>>> = Mutex::new(RefCell::new(None));

pub struct Glyph {
    width: usize,
    rows: [u8; GLYPH_BITMAP_SIZE],
}

impl Glyph {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn row(&self, row: usize) -> u16 {
        debug_assert!(row < GLYPH_HEIGHT);
        let offset = row * GLYPH_ROW_SIZE;
        u16::from_be_bytes([self.rows[offset], self.rows[offset + 1]])
    }
}

pub fn glyph(ch: char) -> Option<Glyph> {
    let codepoint = ch as u32;
    if codepoint > u16::MAX as u32 {
        return None;
    }

    let codepoint = codepoint as u16;
    let record_count = font_len()? / RECORD_SIZE;
    let mut low = 0usize;
    let mut high = record_count;
    let mut record = [0u8; RECORD_SIZE];

    while low < high {
        let mid = (low + high) / 2;
        read_record(mid, &mut record).ok()?;
        let mid_codepoint = u16::from_be_bytes([record[0], record[1]]);

        match mid_codepoint.cmp(&codepoint) {
            core::cmp::Ordering::Less => low = mid + 1,
            core::cmp::Ordering::Greater => high = mid,
            core::cmp::Ordering::Equal => {
                let mut rows = [0u8; GLYPH_BITMAP_SIZE];
                rows.copy_from_slice(&record[BITMAP_OFFSET..BITMAP_OFFSET + GLYPH_BITMAP_SIZE]);
                return Some(Glyph {
                    width: record[WIDTH_OFFSET] as usize,
                    rows,
                });
            }
        }
    }

    None
}

fn font_len() -> Option<usize> {
    if let Some(len) = critical_section::with(|cs| *FONT_LEN.borrow_ref(cs)) {
        return (len != 0).then_some(len);
    }

    let Ok(len) = assets::asset_len(FONT_ASSET_NAME) else {
        error!("display font asset not found");
        critical_section::with(|cs| {
            FONT_LEN.borrow_ref_mut(cs).replace(0);
        });
        return None;
    };

    if len % RECORD_SIZE != 0 {
        error!("display font asset has invalid length: {}", len);
        critical_section::with(|cs| {
            FONT_LEN.borrow_ref_mut(cs).replace(0);
        });
        return None;
    }

    info!("display font asset loaded: {} bytes", len);
    critical_section::with(|cs| {
        FONT_LEN.borrow_ref_mut(cs).replace(len);
    });

    Some(len)
}

fn read_record(index: usize, record: &mut [u8; RECORD_SIZE]) -> Result<(), assets::Error> {
    assets::read_asset(FONT_ASSET_NAME, index * RECORD_SIZE, record)
}

const _: () = {
    assert!(2 + 1 + GLYPH_HEIGHT * GLYPH_ROW_SIZE == RECORD_SIZE);
};
