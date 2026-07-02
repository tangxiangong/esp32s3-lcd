use slint::platform::software_renderer::Rgb565Pixel;

mod font;

const GLYPH_HEIGHT: usize = 16;
const LINE_SPACING: usize = 2;
const STATUS_TEXT_RESTORE_X: usize = 516;
const STATUS_TEXT_RESTORE_Y: usize = 0;
const STATUS_TEXT_RESTORE_WIDTH: usize = 124;
const STATUS_TEXT_RESTORE_HEIGHT: usize = 34;
const STATUS_TEXT_X: usize = 522;
const STATUS_TEXT_Y: usize = 8;

struct Rect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

pub fn text_width(text: &str) -> usize {
    text.chars()
        .map(|ch| font::glyph(ch).map_or(8, |glyph| glyph.width()))
        .sum()
}

pub fn restore_status_background(
    frame: &mut [Rgb565Pixel],
    background: &[Rgb565Pixel],
    frame_width: usize,
    frame_height: usize,
) {
    copy_rect_from_background(
        frame,
        background,
        frame_width,
        frame_height,
        Rect {
            x: STATUS_TEXT_RESTORE_X,
            y: STATUS_TEXT_RESTORE_Y,
            width: STATUS_TEXT_RESTORE_WIDTH,
            height: STATUS_TEXT_RESTORE_HEIGHT,
        },
    );
}

pub fn draw_status_text(
    frame: &mut [Rgb565Pixel],
    frame_width: usize,
    frame_height: usize,
    text: &str,
    color: Rgb565Pixel,
) {
    draw_text(
        frame,
        frame_width,
        frame_height,
        STATUS_TEXT_X,
        STATUS_TEXT_Y,
        text,
        color,
    );
}

pub fn draw_text(
    frame: &mut [Rgb565Pixel],
    frame_width: usize,
    frame_height: usize,
    x: usize,
    y: usize,
    text: &str,
    color: Rgb565Pixel,
) {
    let mut cursor_x = x;
    let mut cursor_y = y;

    for ch in text.chars() {
        match ch {
            '\n' => {
                cursor_x = x;
                cursor_y += GLYPH_HEIGHT + LINE_SPACING;
            }
            '\r' => {}
            _ => {
                let advance = draw_char(
                    frame,
                    frame_width,
                    frame_height,
                    cursor_x,
                    cursor_y,
                    ch,
                    color,
                );
                cursor_x += advance;
            }
        }

        if cursor_y >= frame_height {
            break;
        }
    }
}

fn draw_char(
    frame: &mut [Rgb565Pixel],
    frame_width: usize,
    frame_height: usize,
    x: usize,
    y: usize,
    ch: char,
    color: Rgb565Pixel,
) -> usize {
    let Some(glyph) = font::glyph(ch).or_else(|| font::glyph('\u{FFFD}')) else {
        return 8;
    };

    for row in 0..GLYPH_HEIGHT {
        let dst_y = y + row;
        if dst_y >= frame_height {
            break;
        }

        let bits = glyph.row(row);
        for col in 0..glyph.width() {
            let dst_x = x + col;
            if dst_x >= frame_width {
                break;
            }

            let mask = 0x8000 >> col;
            if bits & mask != 0 {
                frame[dst_y * frame_width + dst_x] = color;
            }
        }
    }

    glyph.width()
}

fn copy_rect_from_background(
    frame: &mut [Rgb565Pixel],
    background: &[Rgb565Pixel],
    frame_width: usize,
    frame_height: usize,
    rect: Rect,
) {
    if frame.len() != background.len() {
        return;
    }

    let right = (rect.x + rect.width).min(frame_width);
    let bottom = (rect.y + rect.height).min(frame_height);

    for dst_y in rect.y..bottom {
        let row_start = dst_y * frame_width;
        for dst_x in rect.x..right {
            let index = row_start + dst_x;
            frame[index] = background[index];
        }
    }
}
