use core::ops::Range;
use esp_hal::{
    Blocking,
    delay::Delay,
    spi,
    spi::master::{Address, Command, DataMode, SpiDmaBus},
};
use slint::platform::software_renderer::{LineBufferProvider, Rgb565Pixel};

pub const DISPLAY_WIDTH: usize = 640;
pub const DISPLAY_HEIGHT: usize = 172;
// 面板显存原生方向是 172 x 640；应用和 Slint 统一使用 640 x 172 横屏坐标。
pub const NATIVE_WIDTH: usize = 172;
pub const NATIVE_HEIGHT: usize = 640;
// 分块转换可以降低一次 flush 需要的临时内存，同时保持 DMA 写入效率。
pub const NATIVE_CHUNK_ROWS: usize = 64;
pub const NATIVE_CHUNK_PIXELS: usize = NATIVE_WIDTH * NATIVE_CHUNK_ROWS;
pub const NATIVE_CHUNK_BYTES: usize = NATIVE_CHUNK_PIXELS * 2;

const CASET: u8 = 0x2a;
const MADCTL: u8 = 0x36;
const COLMOD: u8 = 0x3a;
const RAMWR: u8 = 0x2c;
const RAMWRC: u8 = 0x3c;
const SLPOUT: u8 = 0x11;
const DISPON: u8 = 0x29;
const WRITE_CMD: u32 = 0x02;
const WRITE_COLOR: u32 = 0x32;
const NATIVE_LINE_BYTES: usize = NATIVE_WIDTH * 2;

pub struct Axs15231b<'d> {
    spi: SpiDmaBus<'d, Blocking>,
}

impl<'d> Axs15231b<'d> {
    pub fn new(spi: SpiDmaBus<'d, Blocking>) -> Self {
        Self { spi }
    }

    pub fn init(&mut self, delay: &mut Delay) -> Result<(), spi::Error> {
        // 初始化序列保持最小化：退出睡眠、设置 RGB565、打开显示。
        // 如需增加厂商寄存器配置，先核对 AXS15231B 数据手册/示例。
        self.write_command(SLPOUT, &[])?;
        delay.delay_millis(100);
        self.write_command(MADCTL, &[0x00])?;
        self.write_command(COLMOD, &[0x55])?;
        self.write_command(SLPOUT, &[])?;
        delay.delay_millis(100);
        self.write_command(DISPON, &[])?;
        delay.delay_millis(100);
        Ok(())
    }

    pub fn clear(&mut self, color: Rgb565Pixel) -> Result<(), spi::Error> {
        self.set_column_range(0, NATIVE_WIDTH)?;

        let line = [color; NATIVE_WIDTH];
        for y in 0..NATIVE_HEIGHT {
            self.write_native_pixels(y, &line)?;
        }

        Ok(())
    }

    pub fn flush_landscape_frame(
        &mut self,
        frame: &[Rgb565Pixel],
        native_chunk: &mut [Rgb565Pixel],
        byte_chunk: &mut [u8],
    ) -> Result<(), spi::Error> {
        debug_assert_eq!(frame.len(), DISPLAY_WIDTH * DISPLAY_HEIGHT);
        debug_assert!(native_chunk.len() >= NATIVE_CHUNK_PIXELS);
        debug_assert!(byte_chunk.len() >= NATIVE_CHUNK_BYTES);

        self.set_column_range(0, NATIVE_WIDTH)?;

        for native_y_start in (0..NATIVE_HEIGHT).step_by(NATIVE_CHUNK_ROWS) {
            let rows = (NATIVE_HEIGHT - native_y_start).min(NATIVE_CHUNK_ROWS);
            let chunk = &mut native_chunk[..rows * NATIVE_WIDTH];

            for row in 0..rows {
                let native_y = native_y_start + row;
                let native_row = &mut chunk[row * NATIVE_WIDTH..(row + 1) * NATIVE_WIDTH];
                (0..NATIVE_WIDTH).for_each(|native_x| {
                    // 将 Slint 横屏坐标旋转到 LCD 原生竖屏显存坐标。
                    let logical_x = native_y;
                    let logical_y = DISPLAY_HEIGHT - native_x - 1;
                    native_row[native_x] = frame[logical_y * DISPLAY_WIDTH + logical_x];
                });
            }

            self.write_native_chunk(native_y_start, chunk, byte_chunk)?;
        }

        Ok(())
    }

    fn set_column_range(&mut self, start: usize, end: usize) -> Result<(), spi::Error> {
        let start = start as u16;
        let end = (end - 1) as u16;
        let params = [(start >> 8) as u8, start as u8, (end >> 8) as u8, end as u8];
        self.write_command(CASET, &params)
    }

    fn write_native_pixels(&mut self, y: usize, pixels: &[Rgb565Pixel]) -> Result<(), spi::Error> {
        let command = if y == 0 { RAMWR } else { RAMWRC };
        let address = Address::_32Bit(encode_qspi_command(WRITE_COLOR, command), DataMode::Single);
        let mut bytes = [0u8; NATIVE_LINE_BYTES];

        debug_assert!(pixels.len() <= NATIVE_WIDTH);
        for (index, pixel) in pixels.iter().enumerate() {
            let offset = index * 2;
            bytes[offset] = (pixel.0 >> 8) as u8;
            bytes[offset + 1] = pixel.0 as u8;
        }

        self.spi.half_duplex_write(
            DataMode::Quad,
            Command::None,
            address,
            0,
            &bytes[..pixels.len() * 2],
        )
    }

    fn write_native_chunk(
        &mut self,
        y: usize,
        pixels: &[Rgb565Pixel],
        byte_buffer: &mut [u8],
    ) -> Result<(), spi::Error> {
        let command = if y == 0 { RAMWR } else { RAMWRC };
        let address = Address::_32Bit(encode_qspi_command(WRITE_COLOR, command), DataMode::Single);

        // AXS15231B 这里使用 0x32 四线写颜色数据，像素仍按 RGB565 大端序发送。
        debug_assert!(pixels.len() <= NATIVE_CHUNK_PIXELS);
        debug_assert!(byte_buffer.len() >= pixels.len() * 2);
        for (index, pixel) in pixels.iter().enumerate() {
            let offset = index * 2;
            byte_buffer[offset] = (pixel.0 >> 8) as u8;
            byte_buffer[offset + 1] = pixel.0 as u8;
        }

        self.spi.half_duplex_write(
            DataMode::Quad,
            Command::None,
            address,
            0,
            &byte_buffer[..pixels.len() * 2],
        )
    }

    fn write_command(&mut self, command: u8, params: &[u8]) -> Result<(), spi::Error> {
        self.spi.half_duplex_write(
            DataMode::Single,
            Command::None,
            Address::_32Bit(encode_qspi_command(WRITE_CMD, command), DataMode::Single),
            0,
            params,
        )
    }
}

pub struct DisplayLines<'a> {
    pub frame: &'a mut [Rgb565Pixel],
    pub line_buffer: &'a mut [Rgb565Pixel],
}

impl LineBufferProvider for DisplayLines<'_> {
    type TargetPixel = Rgb565Pixel;

    fn process_line(
        &mut self,
        line: usize,
        range: Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        // Slint 只渲染本行的脏区；line_buffer 是临时行缓冲，frame 保存完整帧。
        render_fn(&mut self.line_buffer[range.clone()]);

        let frame_line = &mut self.frame[line * DISPLAY_WIDTH..(line + 1) * DISPLAY_WIDTH];
        frame_line[range.clone()].copy_from_slice(&self.line_buffer[range]);
    }
}

fn encode_qspi_command(opcode: u32, command: u8) -> u32 {
    (opcode << 24) | ((command as u32) << 8)
}
