use crate::{
    board::Board,
    lcd::{
        axs15231b::{
            DISPLAY_HEIGHT, DISPLAY_WIDTH, DisplayLines, NATIVE_CHUNK_BYTES, NATIVE_CHUNK_PIXELS,
        },
        platform::EspPlatform,
    },
    rtc::pcf85063::{ClockField, DateTime, Pcf85063},
    text,
};
use alloc::{boxed::Box, format, rc::Rc, vec, vec::Vec};
use core::cell::RefCell;
use esp_hal::time::{Duration, Instant};
use slint::{
    LogicalPosition, SharedString,
    platform::{PointerEventButton, WindowEvent, software_renderer::Rgb565Pixel},
};

slint::include_modules!();

const LONG_PRESS_DURATION: Duration = Duration::from_millis(800);
const ADJUST_IDLE_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Display,
    Adjust,
}

struct UiState {
    mode: UiMode,
    draft: Option<DateTime>,
}

pub fn run(mut board: Board) -> ! {
    // 应用层接管 Board 后进入永久轮询循环；这里不再返回 main。
    board
        .display
        .clear(Rgb565Pixel(0x1010))
        .expect("LCD clear failed");

    // Slint 在本项目中只提供软件渲染窗口；真正的事件循环由下面的固件
    // loop 手动驱动。
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
    let rtc = board.rtc;
    ui.set_adjusting(false);
    let mut wifi_connected = board.wireless.wifi_connected();
    let mut bluetooth_connected = board.wireless.bluetooth_connected();
    ui.set_wifi_connected(wifi_connected);
    ui.set_bluetooth_connected(bluetooth_connected);
    update_clock_ui(&ui, rtc);
    let state = Rc::new(RefCell::new(UiState {
        mode: UiMode::Display,
        draft: None,
    }));
    connect_adjust_callbacks(&ui, rtc, Rc::clone(&state));
    ui.show().expect("Slint UI show failed");

    // frame_buffer 是 Slint 的逻辑横屏画布；native_chunk/byte_chunk 是写入
    // AXS15231B 原生竖屏内存顺序前的转换缓冲。
    let mut frame_buffer: Vec<Rgb565Pixel> = vec![Rgb565Pixel(0); DISPLAY_WIDTH * DISPLAY_HEIGHT];
    let mut display_background: Vec<Rgb565Pixel> =
        vec![Rgb565Pixel(0); DISPLAY_WIDTH * DISPLAY_HEIGHT];
    let mut display_background_ready = false;
    let mut native_chunk: Vec<Rgb565Pixel> = vec![Rgb565Pixel(0); NATIVE_CHUNK_PIXELS];
    let mut byte_chunk: Vec<u8> = vec![0; NATIVE_CHUNK_BYTES];
    let mut line_buffer = [Rgb565Pixel(0); DISPLAY_WIDTH];
    let mut last_frame = Instant::now();
    let mut last_rtc_read = Instant::now();
    let mut press_started: Option<Instant> = None;
    let mut suppress_until_release = false;
    let mut pointer_down = false;
    let mut last_adjust_touch = Instant::now();

    loop {
        // 主循环不能长时间阻塞：BLE 轮询、触摸响应、RTC 更新时间和刷屏都
        // 依赖这里持续运行。
        board.wireless.poll();
        let current_wifi_connected = board.wireless.wifi_connected();
        let current_bluetooth_connected = board.wireless.bluetooth_connected();
        if current_wifi_connected != wifi_connected {
            wifi_connected = current_wifi_connected;
            ui.set_wifi_connected(wifi_connected);
            window.request_redraw();
        }
        if current_bluetooth_connected != bluetooth_connected {
            bluetooth_connected = current_bluetooth_connected;
            ui.set_bluetooth_connected(bluetooth_connected);
            window.request_redraw();
        }

        slint::platform::update_timers_and_animations();
        // 调时界面没有独立任务，空闲超时在帧循环中检查。
        if state.borrow().mode == UiMode::Adjust
            && last_adjust_touch.elapsed() >= ADJUST_IDLE_TIMEOUT
        {
            cancel_adjust(&ui, rtc, &state);
            press_started = None;
            suppress_until_release = false;
            pointer_down = false;
        }

        if state.borrow().mode == UiMode::Display
            && last_rtc_read.elapsed() >= Duration::from_secs(1)
        {
            update_clock_ui(&ui, rtc);
            last_rtc_read = Instant::now();
            window.request_redraw();
        }
        // 触摸在显示模式下只识别长按进入调时；进入调时后才把触摸点转成
        // Slint pointer 事件，避免普通显示页误触按钮。
        if let Ok(Some(point)) = board.touch.read() {
            if point.pressed {
                let mode = state.borrow().mode;
                match mode {
                    UiMode::Display => {
                        if let Some(started) = press_started {
                            if started.elapsed() >= LONG_PRESS_DURATION
                                && begin_adjust(&ui, rtc, &state)
                            {
                                suppress_until_release = true;
                                pointer_down = false;
                                last_adjust_touch = Instant::now();
                                press_started = None;
                            }
                        } else {
                            press_started = Some(Instant::now());
                        }
                    }
                    UiMode::Adjust => {
                        last_adjust_touch = Instant::now();
                        if !suppress_until_release {
                            if pointer_down {
                                dispatch_touch_move(&ui, point.x, point.y);
                            } else {
                                pointer_down = true;
                                dispatch_touch_press(&ui, point.x, point.y);
                            }
                        }
                    }
                }
            } else {
                press_started = None;
                if suppress_until_release {
                    suppress_until_release = false;
                    pointer_down = false;
                } else if state.borrow().mode == UiMode::Adjust {
                    last_adjust_touch = Instant::now();
                    if pointer_down {
                        pointer_down = false;
                        dispatch_touch_release(&ui, point.x, point.y);
                    }
                }
            }
        }

        let mut needs_flush = false;
        window.draw_if_needed(|renderer| {
            needs_flush = true;
            renderer.render_by_line(DisplayLines {
                frame: frame_buffer.as_mut_slice(),
                line_buffer: &mut line_buffer,
            });
            if state.borrow().mode == UiMode::Display {
                // 状态栏文本使用固件侧位图字体叠加。先恢复 Slint 渲染出的
                // 背景，避免每秒更新时间时旧字残留。
                if display_background_ready {
                    text::restore_status_background(
                        frame_buffer.as_mut_slice(),
                        display_background.as_slice(),
                        DISPLAY_WIDTH,
                        DISPLAY_HEIGHT,
                    );
                } else {
                    display_background.copy_from_slice(frame_buffer.as_slice());
                    display_background_ready = true;
                }

                let status_text = ui.get_status_text();
                text::draw_status_text(
                    frame_buffer.as_mut_slice(),
                    DISPLAY_WIDTH,
                    DISPLAY_HEIGHT,
                    status_text.as_str(),
                    Rgb565Pixel(0xffff),
                );
            }
        });

        if needs_flush {
            // LCD 驱动负责把横屏逻辑帧转换为面板原生方向，不在应用层做
            // QSPI 命令或坐标细节。
            board
                .display
                .flush_landscape_frame(
                    &frame_buffer,
                    native_chunk.as_mut_slice(),
                    byte_chunk.as_mut_slice(),
                )
                .expect("LCD frame flush failed");
        }

        // 简单限帧，避免空转过快；这里是忙等，后续若加入耗时任务要重新
        // 评估触摸和 BLE 的轮询延迟。
        while last_frame.elapsed() < Duration::from_millis(16) {}
        last_frame = Instant::now();
    }
}

fn connect_adjust_callbacks(ui: &AppWindow, rtc: Pcf85063, state: Rc<RefCell<UiState>>) {
    macro_rules! connect_adjust {
        ($callback:ident, $field:expr, $delta:expr) => {{
            let ui_weak = ui.as_weak();
            let state = Rc::clone(&state);
            ui.$callback(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    adjust_draft(&ui, &state, $field, $delta);
                }
            });
        }};
    }

    connect_adjust!(on_adjust_year_down, ClockField::Year, -1);
    connect_adjust!(on_adjust_year_up, ClockField::Year, 1);
    connect_adjust!(on_adjust_month_down, ClockField::Month, -1);
    connect_adjust!(on_adjust_month_up, ClockField::Month, 1);
    connect_adjust!(on_adjust_day_down, ClockField::Day, -1);
    connect_adjust!(on_adjust_day_up, ClockField::Day, 1);
    connect_adjust!(on_adjust_hour_down, ClockField::Hour, -1);
    connect_adjust!(on_adjust_hour_up, ClockField::Hour, 1);
    connect_adjust!(on_adjust_minute_down, ClockField::Minute, -1);
    connect_adjust!(on_adjust_minute_up, ClockField::Minute, 1);
    connect_adjust!(on_adjust_second_down, ClockField::Second, -1);
    connect_adjust!(on_adjust_second_up, ClockField::Second, 1);

    let ui_weak = ui.as_weak();
    let state_for_confirm = Rc::clone(&state);
    ui.on_confirm_adjust(move || {
        if let Some(ui) = ui_weak.upgrade() {
            confirm_adjust(&ui, rtc, &state_for_confirm);
        }
    });

    let ui_weak = ui.as_weak();
    ui.on_cancel_adjust(move || {
        if let Some(ui) = ui_weak.upgrade() {
            cancel_adjust(&ui, rtc, &state);
        }
    });
}

fn dispatch_touch_press(ui: &AppWindow, x: u16, y: u16) {
    let position = LogicalPosition::new(x as f32, y as f32);
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
}

fn dispatch_touch_move(ui: &AppWindow, x: u16, y: u16) {
    let position = LogicalPosition::new(x as f32, y as f32);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
}

fn dispatch_touch_release(ui: &AppWindow, x: u16, y: u16) {
    let position = LogicalPosition::new(x as f32, y as f32);
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn begin_adjust(ui: &AppWindow, rtc: Pcf85063, state: &RefCell<UiState>) -> bool {
    let Ok(datetime) = rtc.read_datetime() else {
        return false;
    };

    {
        let mut state = state.borrow_mut();
        state.mode = UiMode::Adjust;
        state.draft = Some(datetime);
    }
    apply_clock_ui(ui, datetime);
    ui.set_adjusting(true);
    true
}

fn adjust_draft(ui: &AppWindow, state: &RefCell<UiState>, field: ClockField, delta: i32) {
    let mut state = state.borrow_mut();
    let Some(datetime) = state.draft else {
        return;
    };

    let datetime = datetime.adjusted(field, delta);
    state.draft = Some(datetime);
    drop(state);
    apply_clock_ui(ui, datetime);
}

fn confirm_adjust(ui: &AppWindow, rtc: Pcf85063, state: &RefCell<UiState>) {
    let Some(datetime) = state.borrow().draft else {
        return;
    };

    if rtc.write_datetime(datetime).is_ok() {
        exit_adjust(ui, state);
        apply_clock_ui(ui, datetime);
    }
}

fn cancel_adjust(ui: &AppWindow, rtc: Pcf85063, state: &RefCell<UiState>) {
    exit_adjust(ui, state);
    update_clock_ui(ui, rtc);
}

fn exit_adjust(ui: &AppWindow, state: &RefCell<UiState>) {
    let mut state = state.borrow_mut();
    state.mode = UiMode::Display;
    state.draft = None;
    ui.set_adjusting(false);
}

fn update_clock_ui(ui: &AppWindow, rtc: Pcf85063) {
    match rtc.read_datetime() {
        Ok(datetime) => {
            apply_clock_ui(ui, datetime);
        }
        Err(_) => {
            let status: SharedString = "--月--日 --:--".into();
            ui.set_time_text("--:--:--".into());
            ui.set_date_text("----/--/--".into());
            ui.set_status_text(status);
            ui.set_day_progress(0.0);
            ui.set_second_progress(0.0);
        }
    }
}

fn apply_clock_ui(ui: &AppWindow, datetime: DateTime) {
    let status = format_status(datetime);
    ui.set_time_text(format_time(datetime));
    ui.set_date_text(format_date(datetime));
    ui.set_status_text(status);
    ui.set_year_text(format!("{:04}", datetime.year).into());
    ui.set_month_text(format!("{:02}", datetime.month).into());
    ui.set_day_text(format!("{:02}", datetime.day).into());
    ui.set_hour_text(format!("{:02}", datetime.hour).into());
    ui.set_minute_text(format!("{:02}", datetime.minute).into());
    ui.set_second_text(format!("{:02}", datetime.second).into());
    ui.set_day_progress(day_progress(datetime));
    ui.set_second_progress(second_progress(datetime));
}

fn format_time(datetime: DateTime) -> SharedString {
    format!(
        "{:02}:{:02}:{:02}",
        datetime.hour, datetime.minute, datetime.second
    )
    .into()
}

fn format_date(datetime: DateTime) -> SharedString {
    format!(
        "{:04}/{:02}/{:02}",
        datetime.year, datetime.month, datetime.day
    )
    .into()
}

fn format_status(datetime: DateTime) -> SharedString {
    format!(
        "{}月{}日 {:02}:{:02}",
        datetime.month, datetime.day, datetime.hour, datetime.minute
    )
    .into()
}

fn day_progress(datetime: DateTime) -> f32 {
    let seconds = u32::from(datetime.hour) * 3600
        + u32::from(datetime.minute) * 60
        + u32::from(datetime.second);

    seconds as f32 / 86_400.0
}

fn second_progress(datetime: DateTime) -> f32 {
    f32::from(datetime.second) / 60.0
}
