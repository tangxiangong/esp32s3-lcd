extern crate alloc;

use crate::lcd::{
    axs15231b::{
        DISPLAY_HEIGHT, DISPLAY_WIDTH, DisplayLines, NATIVE_CHUNK_BYTES, NATIVE_CHUNK_PIXELS,
    },
    platform::EspPlatform,
};
use alloc::{boxed::Box, vec, vec::Vec};
use esp_hal::time::{Duration, Instant};
use slint::platform::software_renderer::Rgb565Pixel;

use crate::board::Board;

slint::include_modules!();

pub fn run(mut board: Board) -> ! {
    board
        .display
        .clear(Rgb565Pixel(0x1010))
        .expect("LCD clear failed");

    let window = slint::platform::software_renderer::MinimalSoftwareWindow::new(
        slint::platform::software_renderer::RepaintBufferType::ReusedBuffer,
    );
    window.set_size(slint::PhysicalSize::new(
        DISPLAY_WIDTH as u32,
        DISPLAY_HEIGHT as u32,
    ));
    slint::platform::set_platform(Box::new(EspPlatform::new(window.clone())))
        .expect("Slint platform setup failed");

    let ui = AppWindow::new().expect("Slint UI setup failed");
    ui.show().expect("Slint UI show failed");

    let mut frame_buffer: Vec<Rgb565Pixel> = vec![Rgb565Pixel(0); DISPLAY_WIDTH * DISPLAY_HEIGHT];
    let mut native_chunk: Vec<Rgb565Pixel> = vec![Rgb565Pixel(0); NATIVE_CHUNK_PIXELS];
    let mut byte_chunk: Vec<u8> = vec![0; NATIVE_CHUNK_BYTES];
    let mut line_buffer = [Rgb565Pixel(0); DISPLAY_WIDTH];
    let mut last_frame = Instant::now();

    loop {
        slint::platform::update_timers_and_animations();
        let mut needs_flush = false;
        window.draw_if_needed(|renderer| {
            needs_flush = true;
            renderer.render_by_line(DisplayLines {
                frame: frame_buffer.as_mut_slice(),
                line_buffer: &mut line_buffer,
            });
        });

        if needs_flush {
            board
                .display
                .flush_landscape_frame(
                    &frame_buffer,
                    native_chunk.as_mut_slice(),
                    byte_chunk.as_mut_slice(),
                )
                .expect("LCD frame flush failed");
        }

        while last_frame.elapsed() < Duration::from_millis(16) {}
        last_frame = Instant::now();
    }
}
