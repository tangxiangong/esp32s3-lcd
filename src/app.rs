use crate::{
    board::Board,
    lcd::{
        axs15231b::{
            DISPLAY_HEIGHT, DISPLAY_WIDTH, DisplayLines, NATIVE_CHUNK_BYTES, NATIVE_CHUNK_PIXELS,
        },
        platform::EspPlatform,
    },
    radio::{BleActionError, WifiActionError, WifiAuthMethod, Wireless},
    rtc::pcf85063::{ClockField, DateTime, Pcf85063},
    text,
};
use alloc::{boxed::Box, format, rc::Rc, string::String, vec, vec::Vec};
use core::cell::RefCell;
use esp_hal::time::{Duration, Instant};
use slint::{
    LogicalPosition, SharedString,
    platform::{PointerEventButton, WindowEvent, software_renderer::Rgb565Pixel},
};

slint::include_modules!();

const ADJUST_IDLE_TIMEOUT: Duration = Duration::from_secs(12);
const SWIPE_THRESHOLD: u16 = 72;
const WIFI_KEY_PAGE_COUNT: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Display,
    Settings,
    Wifi,
    WifiAdvanced,
    Bluetooth,
    Adjust,
    About,
}

struct UiState {
    mode: UiMode,
    draft: Option<DateTime>,
    wifi_enabled: bool,
    wifi_selected: usize,
    wifi_edit_field: WifiEditField,
    wifi_manual_ssid: String,
    wifi_password: String,
    wifi_key_page: usize,
}

#[derive(Clone, Copy)]
enum PendingAction {
    WifiToggle,
    WifiScan,
    WifiPrevious,
    WifiNext,
    WifiConnect,
    WifiDisconnect,
    BluetoothRefresh,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WifiEditField {
    Ssid,
    Password,
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
    let mut wifi_connected = board.wireless.wifi_connected();
    let mut bluetooth_connected = board.wireless.bluetooth_connected();
    ui.set_wifi_connected(wifi_connected);
    ui.set_bluetooth_connected(bluetooth_connected);
    ui.set_about_detail(
        format!(
            "ESP32-S3 Touch LCD 3.49\n固件 v{}",
            env!("CARGO_PKG_VERSION")
        )
        .into(),
    );
    update_clock_ui(&ui, rtc);
    let state = Rc::new(RefCell::new(UiState {
        mode: UiMode::Display,
        draft: None,
        wifi_enabled: true,
        wifi_selected: 0,
        wifi_edit_field: WifiEditField::Ssid,
        wifi_manual_ssid: String::new(),
        wifi_password: String::new(),
        wifi_key_page: 0,
    }));
    let pending = Rc::new(RefCell::new(None));
    let mut deferred_action: Option<PendingAction> = None;
    update_wifi_keyboard(&ui, &state.borrow());
    refresh_wifi_ui(&ui, &board.wireless, &state.borrow());
    refresh_bluetooth_ui(&ui, &board.wireless);
    connect_adjust_callbacks(&ui, rtc, Rc::clone(&state));
    connect_navigation_callbacks(&ui, rtc, Rc::clone(&state), Rc::clone(&pending));
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
    let mut pointer_down = false;
    let mut touch_start: Option<(u16, u16)> = None;
    let mut deferred_touch = false;
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
            if !wifi_connected && matches!(state.borrow().mode, UiMode::Wifi | UiMode::WifiAdvanced)
            {
                ui.set_wifi_status("WiFi 已断开".into());
                refresh_wifi_ui(&ui, &board.wireless, &state.borrow());
            }
            window.request_redraw();
        }
        if current_bluetooth_connected != bluetooth_connected {
            bluetooth_connected = current_bluetooth_connected;
            ui.set_bluetooth_connected(bluetooth_connected);
            if state.borrow().mode == UiMode::Bluetooth {
                refresh_bluetooth_ui(&ui, &board.wireless);
            }
            window.request_redraw();
        } else if state.borrow().mode == UiMode::Bluetooth {
            refresh_bluetooth_ui(&ui, &board.wireless);
        }

        slint::platform::update_timers_and_animations();
        // 调时界面没有独立任务，空闲超时在帧循环中检查。
        if state.borrow().mode == UiMode::Adjust
            && last_adjust_touch.elapsed() >= ADJUST_IDLE_TIMEOUT
        {
            cancel_adjust(&ui, rtc, &state);
            pointer_down = false;
            touch_start = None;
        }

        if state.borrow().mode == UiMode::Display
            && last_rtc_read.elapsed() >= Duration::from_secs(1)
        {
            update_clock_ui(&ui, rtc);
            last_rtc_read = Instant::now();
            window.request_redraw();
        }

        // 所有页面都走 Slint pointer 事件；主页不再通过长按调时，时间设置
        // 只从设置菜单进入。
        if let Ok(Some(point)) = board.touch.read() {
            let current_mode = state.borrow().mode;
            if point.pressed {
                if current_mode == UiMode::Adjust {
                    last_adjust_touch = Instant::now();
                }
                if pointer_down {
                    if !deferred_touch {
                        dispatch_touch_move(&ui, point.x, point.y);
                    }
                } else {
                    pointer_down = true;
                    touch_start = Some((point.x, point.y));
                    deferred_touch = matches!(current_mode, UiMode::Display | UiMode::Wifi);
                    if !deferred_touch {
                        dispatch_touch_press(&ui, point.x, point.y);
                    }
                }
            } else {
                if current_mode == UiMode::Adjust {
                    last_adjust_touch = Instant::now();
                }
                if pointer_down {
                    pointer_down = false;
                    if let Some((start_x, start_y)) = touch_start.take() {
                        if is_horizontal_swipe(start_x, start_y, point.x, point.y) {
                            handle_swipe(
                                &ui,
                                rtc,
                                &board.wireless,
                                &state,
                                (start_x, start_y),
                                (point.x, point.y),
                            );
                        } else if deferred_touch {
                            dispatch_touch_press(&ui, point.x, point.y);
                            dispatch_touch_release(&ui, point.x, point.y);
                        } else {
                            dispatch_touch_release(&ui, point.x, point.y);
                        }
                    } else if !deferred_touch {
                        dispatch_touch_release(&ui, point.x, point.y);
                    }
                    deferred_touch = false;
                }
            }
        }
        process_pending_action(&ui, &board.wireless, &state, &pending, &mut deferred_action);
        if deferred_action.is_some() {
            window.request_redraw();
        }

        let mut needs_flush = false;
        window.draw_if_needed(|renderer| {
            needs_flush = true;
            renderer.render_by_line(DisplayLines {
                frame: frame_buffer.as_mut_slice(),
                line_buffer: &mut line_buffer,
            });
            if state.borrow().mode == UiMode::Display {
                // 主屏保留原来的固件侧日期/时间叠加，避免设置入口改动影响时钟显示。
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
                draw_centered_text(
                    frame_buffer.as_mut_slice(),
                    440,
                    94,
                    116,
                    "设置",
                    Rgb565Pixel(0xffff),
                );
            } else {
                let mode = state.borrow().mode;
                draw_ui_text_overlays(&ui, mode, frame_buffer.as_mut_slice());
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

        if let Some(action) = deferred_action.take() {
            let display = &mut board.display;
            let touch = &mut board.touch;
            let wireless = &mut board.wireless;
            execute_deferred_action(&ui, wireless, &state, action, || {
                slint::platform::update_timers_and_animations();
                window.request_redraw();

                if let Ok(Some(point)) = touch.read() {
                    let current_mode = state.borrow().mode;
                    if point.pressed {
                        if current_mode == UiMode::Adjust {
                            last_adjust_touch = Instant::now();
                        }
                        if pointer_down {
                            if !deferred_touch {
                                dispatch_touch_move(&ui, point.x, point.y);
                            }
                        } else {
                            pointer_down = true;
                            touch_start = Some((point.x, point.y));
                            deferred_touch = matches!(current_mode, UiMode::Display | UiMode::Wifi);
                            if !deferred_touch {
                                dispatch_touch_press(&ui, point.x, point.y);
                            }
                        }
                    } else {
                        if current_mode == UiMode::Adjust {
                            last_adjust_touch = Instant::now();
                        }
                        if pointer_down {
                            pointer_down = false;
                            if let Some((start_x, start_y)) = touch_start.take() {
                                if !is_horizontal_swipe(start_x, start_y, point.x, point.y)
                                    && deferred_touch
                                {
                                    dispatch_touch_press(&ui, point.x, point.y);
                                    dispatch_touch_release(&ui, point.x, point.y);
                                } else if !deferred_touch {
                                    dispatch_touch_release(&ui, point.x, point.y);
                                }
                            } else if !deferred_touch {
                                dispatch_touch_release(&ui, point.x, point.y);
                            }
                            deferred_touch = false;
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
                        draw_centered_text(
                            frame_buffer.as_mut_slice(),
                            440,
                            94,
                            116,
                            "设置",
                            Rgb565Pixel(0xffff),
                        );
                    } else {
                        let mode = state.borrow().mode;
                        draw_ui_text_overlays(&ui, mode, frame_buffer.as_mut_slice());
                    }
                });

                if needs_flush {
                    display
                        .flush_landscape_frame(
                            &frame_buffer,
                            native_chunk.as_mut_slice(),
                            byte_chunk.as_mut_slice(),
                        )
                        .expect("LCD frame flush failed");
                }
            });
            window.request_redraw();
        }

        // 简单限帧，避免空转过快；这里是忙等，后续若加入耗时任务要重新
        // 评估触摸和 BLE 的轮询延迟。
        while last_frame.elapsed() < Duration::from_millis(16) {}
        last_frame = Instant::now();
    }
}

fn draw_ui_text_overlays(ui: &AppWindow, mode: UiMode, frame: &mut [Rgb565Pixel]) {
    match mode {
        UiMode::Display => {}
        UiMode::Settings => draw_settings_text(frame),
        UiMode::Wifi => draw_wifi_text(ui, frame),
        UiMode::WifiAdvanced => draw_wifi_advanced_text(ui, frame),
        UiMode::Bluetooth => draw_bluetooth_text(ui, frame),
        UiMode::Adjust => draw_time_text(ui, frame),
        UiMode::About => draw_about_text(ui, frame),
    }
}

fn draw_settings_text(frame: &mut [Rgb565Pixel]) {
    let white = Rgb565Pixel(0xffff);
    draw_centered_text(frame, 18, 32, 86, "返回", white);
    draw_centered_text(frame, 126, 96, 108, "WiFi", white);
    draw_centered_text(frame, 250, 96, 108, "蓝牙", white);
    draw_centered_text(frame, 374, 96, 108, "时间", white);
    draw_centered_text(frame, 498, 96, 108, "关于", white);
}

fn draw_wifi_text(ui: &AppWindow, frame: &mut [Rgb565Pixel]) {
    let white = Rgb565Pixel(0xffff);
    let muted = Rgb565Pixel(0x9d17);
    let warning = if ui.get_wifi_connected() {
        Rgb565Pixel(0x8ff7)
    } else {
        Rgb565Pixel(0xfe51)
    };

    draw_centered_text(frame, 10, 24, 72, "返回", white);
    draw_text_at(frame, 96, 24, "WiFi", white);
    draw_centered_text(
        frame,
        184,
        24,
        86,
        if ui.get_wifi_enabled() {
            "关闭"
        } else {
            "打开"
        },
        white,
    );
    draw_centered_text(frame, 282, 24, 86, "扫描", white);
    draw_centered_text(frame, 380, 24, 86, "连接", white);
    draw_centered_text(frame, 478, 24, 86, "断开", white);
    draw_centered_text(frame, 574, 24, 56, "高级", white);

    draw_wifi_row_text(
        frame,
        30,
        64,
        344,
        ui.get_wifi_row0_title().as_str(),
        ui.get_wifi_row0_detail().as_str(),
        ui.get_wifi_row0_selected(),
    );
    draw_wifi_row_text(
        frame,
        30,
        112,
        344,
        ui.get_wifi_row1_title().as_str(),
        ui.get_wifi_row1_detail().as_str(),
        ui.get_wifi_row1_selected(),
    );
    draw_wifi_row_text(
        frame,
        416,
        64,
        186,
        ui.get_wifi_row2_title().as_str(),
        ui.get_wifi_row2_detail().as_str(),
        ui.get_wifi_row2_selected(),
    );

    draw_text_at(frame, 402, 117, ui.get_wifi_status().as_str(), warning);
    draw_text_at(
        frame,
        402,
        139,
        ui.get_wifi_network_detail().as_str(),
        muted,
    );
}

fn draw_wifi_row_text(
    frame: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    width: usize,
    title: &str,
    detail: &str,
    selected: bool,
) {
    if title.is_empty() {
        return;
    }

    let title_color = Rgb565Pixel(0xffff);
    let detail_color = if selected {
        Rgb565Pixel(0xbf7f)
    } else {
        Rgb565Pixel(0x8d37)
    };
    draw_clipped_text(frame, x, y, width, title, title_color);
    draw_clipped_text(frame, x, y + 19, width, detail, detail_color);
}

fn draw_wifi_advanced_text(ui: &AppWindow, frame: &mut [Rgb565Pixel]) {
    let white = Rgb565Pixel(0xffff);
    let muted = Rgb565Pixel(0x8d37);
    let accent = Rgb565Pixel(0xbf7f);

    draw_centered_text(frame, 10, 24, 72, "返回", white);
    draw_centered_text(frame, 94, 24, 82, "SSID", white);
    draw_centered_text(frame, 188, 24, 82, "密码", white);
    draw_centered_text(frame, 282, 24, 82, "清空", white);
    draw_centered_text(frame, 376, 24, 82, "连接", white);
    draw_text_at(frame, 472, 20, "WiFi 高级", white);

    let ssid = format!("SSID {}", ui.get_wifi_manual_ssid());
    let password = format!("密码 {}", ui.get_wifi_password());
    let edit = format!("{} {}", ui.get_wifi_edit_label(), ui.get_wifi_key_page());
    draw_clipped_text(frame, 16, 56, 260, &ssid, accent);
    draw_clipped_text(frame, 16, 80, 260, &password, accent);
    draw_clipped_text(frame, 16, 104, 260, &edit, muted);
    draw_centered_text(frame, 526, 68, 44, "删", white);
    draw_centered_text(frame, 526, 118, 44, "换", white);
}

fn draw_bluetooth_text(ui: &AppWindow, frame: &mut [Rgb565Pixel]) {
    let white = Rgb565Pixel(0xffff);
    draw_centered_text(frame, 18, 32, 84, "返回", white);
    draw_centered_text(frame, 126, 32, 112, "重新广播", white);
    draw_text_at(frame, 266, 32, "蓝牙", white);
    draw_clipped_text(
        frame,
        266,
        77,
        300,
        ui.get_bluetooth_status().as_str(),
        white,
    );
}

fn draw_time_text(ui: &AppWindow, frame: &mut [Rgb565Pixel]) {
    let white = Rgb565Pixel(0xffff);
    let muted = Rgb565Pixel(0x9d17);
    draw_centered_text(frame, 8, 28, 76, "年", muted);
    draw_centered_text(frame, 92, 28, 66, "月", muted);
    draw_centered_text(frame, 166, 28, 66, "日", muted);
    draw_centered_text(frame, 254, 28, 66, "时", muted);
    draw_centered_text(frame, 328, 28, 66, "分", muted);
    draw_centered_text(frame, 402, 28, 66, "秒", muted);
    draw_centered_text(frame, 488, 53, 94, "保存", white);
    draw_centered_text(frame, 488, 105, 94, "取消", white);
    draw_clipped_text(frame, 486, 134, 120, ui.get_time_status().as_str(), muted);
}

fn draw_about_text(ui: &AppWindow, frame: &mut [Rgb565Pixel]) {
    let white = Rgb565Pixel(0xffff);
    draw_centered_text(frame, 18, 32, 84, "返回", white);
    draw_text_at(frame, 126, 34, "关于", white);
    draw_text_at(frame, 126, 70, ui.get_about_detail().as_str(), white);
}

fn draw_centered_text(
    frame: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    width: usize,
    text: &str,
    color: Rgb565Pixel,
) {
    let text_width = text::text_width(text);
    let text_x = x + width.saturating_sub(text_width) / 2;
    draw_text_at(frame, text_x, y, text, color);
}

fn draw_clipped_text(
    frame: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    max_width: usize,
    value: &str,
    color: Rgb565Pixel,
) {
    let mut output = String::new();
    let mut width = 0;
    for ch in value.chars() {
        let mut buffer = [0; 4];
        let char_width = text::text_width(ch.encode_utf8(&mut buffer));
        if width + char_width > max_width {
            break;
        }
        output.push(ch);
        width += char_width;
    }
    draw_text_at(frame, x, y, &output, color);
}

fn draw_text_at(frame: &mut [Rgb565Pixel], x: usize, y: usize, value: &str, color: Rgb565Pixel) {
    text::draw_text(frame, DISPLAY_WIDTH, DISPLAY_HEIGHT, x, y, value, color);
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

fn connect_navigation_callbacks(
    ui: &AppWindow,
    rtc: Pcf85063,
    state: Rc<RefCell<UiState>>,
    pending: Rc<RefCell<Option<PendingAction>>>,
) {
    let ui_weak = ui.as_weak();
    let state_for_open_settings = Rc::clone(&state);
    ui.on_open_settings(move || {
        if let Some(ui) = ui_weak.upgrade() {
            set_mode(&ui, &state_for_open_settings, UiMode::Settings);
        }
    });

    let ui_weak = ui.as_weak();
    let state_for_wifi = Rc::clone(&state);
    ui.on_open_wifi(move || {
        if let Some(ui) = ui_weak.upgrade() {
            set_mode(&ui, &state_for_wifi, UiMode::Wifi);
        }
    });

    let ui_weak = ui.as_weak();
    let state_for_wifi_advanced = Rc::clone(&state);
    ui.on_open_wifi_advanced(move || {
        if let Some(ui) = ui_weak.upgrade() {
            set_mode(&ui, &state_for_wifi_advanced, UiMode::WifiAdvanced);
        }
    });

    let ui_weak = ui.as_weak();
    let state_for_bluetooth = Rc::clone(&state);
    ui.on_open_bluetooth(move || {
        if let Some(ui) = ui_weak.upgrade() {
            set_mode(&ui, &state_for_bluetooth, UiMode::Bluetooth);
        }
    });

    let ui_weak = ui.as_weak();
    let state_for_time = Rc::clone(&state);
    ui.on_open_time(move || {
        if let Some(ui) = ui_weak.upgrade() {
            let _ = begin_adjust(&ui, rtc, &state_for_time);
        }
    });

    let ui_weak = ui.as_weak();
    let state_for_about = Rc::clone(&state);
    ui.on_open_about(move || {
        if let Some(ui) = ui_weak.upgrade() {
            set_mode(&ui, &state_for_about, UiMode::About);
        }
    });

    let ui_weak = ui.as_weak();
    let state_for_back_home = Rc::clone(&state);
    ui.on_back_home(move || {
        if let Some(ui) = ui_weak.upgrade() {
            set_mode(&ui, &state_for_back_home, UiMode::Display);
        }
    });

    let ui_weak = ui.as_weak();
    let state_for_back_settings = Rc::clone(&state);
    ui.on_back_settings(move || {
        if let Some(ui) = ui_weak.upgrade() {
            set_mode(&ui, &state_for_back_settings, UiMode::Settings);
        }
    });

    let pending_for_scan = Rc::clone(&pending);
    ui.on_wifi_toggle(move || {
        *pending_for_scan.borrow_mut() = Some(PendingAction::WifiToggle);
    });

    let pending_for_scan = Rc::clone(&pending);
    ui.on_wifi_scan(move || {
        *pending_for_scan.borrow_mut() = Some(PendingAction::WifiScan);
    });

    let ui_weak = ui.as_weak();
    let state_for_select = Rc::clone(&state);
    ui.on_wifi_select_network(move |offset| {
        if let Some(ui) = ui_weak.upgrade() {
            let mut ui_state = state_for_select.borrow_mut();
            let start = visible_wifi_start(ui_state.wifi_selected);
            ui_state.wifi_selected = start + offset.max(0) as usize;
            let selected = ui_state.wifi_selected;
            drop(ui_state);
            update_wifi_row_selection(&ui, selected);
            update_wifi_input_ui(&ui, &state_for_select.borrow());
        }
    });

    let pending_for_prev = Rc::clone(&pending);
    ui.on_wifi_prev_network(move || {
        *pending_for_prev.borrow_mut() = Some(PendingAction::WifiPrevious);
    });

    let pending_for_next = Rc::clone(&pending);
    ui.on_wifi_next_network(move || {
        *pending_for_next.borrow_mut() = Some(PendingAction::WifiNext);
    });

    let pending_for_connect = Rc::clone(&pending);
    ui.on_wifi_connect(move || {
        *pending_for_connect.borrow_mut() = Some(PendingAction::WifiConnect);
    });

    let pending_for_disconnect = Rc::clone(&pending);
    ui.on_wifi_disconnect(move || {
        *pending_for_disconnect.borrow_mut() = Some(PendingAction::WifiDisconnect);
    });

    ui.on_bluetooth_refresh(move || {
        *pending.borrow_mut() = Some(PendingAction::BluetoothRefresh);
    });

    let ui_weak = ui.as_weak();
    let state_for_key = Rc::clone(&state);
    ui.on_wifi_key(move |key| {
        if let Some(ui) = ui_weak.upgrade() {
            let mut state = state_for_key.borrow_mut();
            push_wifi_input(&mut state, key.as_str());
            update_wifi_input_ui(&ui, &state);
        }
    });

    let ui_weak = ui.as_weak();
    let state_for_backspace = Rc::clone(&state);
    ui.on_wifi_backspace(move || {
        if let Some(ui) = ui_weak.upgrade() {
            let mut state = state_for_backspace.borrow_mut();
            pop_wifi_input(&mut state);
            update_wifi_input_ui(&ui, &state);
        }
    });

    let ui_weak = ui.as_weak();
    let state_for_key_page = Rc::clone(&state);
    ui.on_wifi_next_key_page(move || {
        if let Some(ui) = ui_weak.upgrade() {
            let mut state = state_for_key_page.borrow_mut();
            state.wifi_key_page = (state.wifi_key_page + 1) % WIFI_KEY_PAGE_COUNT;
            update_wifi_keyboard(&ui, &state);
        }
    });

    let ui_weak = ui.as_weak();
    let state_for_clear = Rc::clone(&state);
    ui.on_wifi_clear_input(move || {
        if let Some(ui) = ui_weak.upgrade() {
            let mut state = state_for_clear.borrow_mut();
            clear_wifi_input(&mut state);
            update_wifi_input_ui(&ui, &state);
        }
    });

    let ui_weak = ui.as_weak();
    let state_for_ssid = Rc::clone(&state);
    ui.on_wifi_edit_ssid(move || {
        if let Some(ui) = ui_weak.upgrade() {
            let mut state = state_for_ssid.borrow_mut();
            state.wifi_edit_field = WifiEditField::Ssid;
            update_wifi_input_ui(&ui, &state);
        }
    });

    let ui_weak = ui.as_weak();
    ui.on_wifi_edit_password(move || {
        if let Some(ui) = ui_weak.upgrade() {
            let mut state = state.borrow_mut();
            state.wifi_edit_field = WifiEditField::Password;
            update_wifi_input_ui(&ui, &state);
        }
    });
}

fn set_mode(ui: &AppWindow, state: &RefCell<UiState>, mode: UiMode) {
    let screen = match mode {
        UiMode::Display => 0,
        UiMode::Settings => 1,
        UiMode::Wifi => 2,
        UiMode::WifiAdvanced => 6,
        UiMode::Bluetooth => 3,
        UiMode::Adjust => 4,
        UiMode::About => 5,
    };
    state.borrow_mut().mode = mode;
    ui.set_screen(screen);
}

fn update_wifi_keyboard(ui: &AppWindow, state: &UiState) {
    let keys = match state.wifi_key_page {
        0 => ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"],
        1 => ["k", "l", "m", "n", "o", "p", "q", "r", "s", "t"],
        2 => ["u", "v", "w", "x", "y", "z", "A", "B", "C", "D"],
        3 => ["E", "F", "G", "H", "I", "J", "K", "L", "M", "N"],
        4 => ["O", "P", "Q", "R", "S", "T", "U", "V", "W", "X"],
        5 => ["Y", "Z", "0", "1", "2", "3", "4", "5", "6", "7"],
        6 => ["8", "9", "_", "-", ".", "@", "!", "?", "#", "$"],
        _ => ["%", "&", "*", "+", "/", "=", ":", ";", "'", "\""],
    };

    ui.set_wifi_key0(keys[0].into());
    ui.set_wifi_key1(keys[1].into());
    ui.set_wifi_key2(keys[2].into());
    ui.set_wifi_key3(keys[3].into());
    ui.set_wifi_key4(keys[4].into());
    ui.set_wifi_key5(keys[5].into());
    ui.set_wifi_key6(keys[6].into());
    ui.set_wifi_key7(keys[7].into());
    ui.set_wifi_key8(keys[8].into());
    ui.set_wifi_key9(keys[9].into());
    ui.set_wifi_key_page(format!("{}/{}", state.wifi_key_page + 1, WIFI_KEY_PAGE_COUNT).into());
}

fn handle_swipe(
    ui: &AppWindow,
    rtc: Pcf85063,
    wireless: &Wireless,
    state: &RefCell<UiState>,
    start: (u16, u16),
    end: (u16, u16),
) {
    let (start_x, start_y) = start;
    let (end_x, end_y) = end;
    if !is_horizontal_swipe(start_x, start_y, end_x, end_y) {
        return;
    }

    match state.borrow().mode {
        UiMode::Display => {}
        UiMode::Settings => {}
        UiMode::Wifi => {
            if end_x < start_x {
                state.borrow_mut().wifi_selected += 1;
            } else {
                let mut ui_state = state.borrow_mut();
                ui_state.wifi_selected = ui_state.wifi_selected.saturating_sub(1);
            }
            refresh_wifi_ui(ui, wireless, &state.borrow());
        }
        UiMode::WifiAdvanced => {}
        UiMode::Adjust => {
            let _ = rtc;
        }
        UiMode::Bluetooth | UiMode::About => {}
    }
}

fn process_pending_action(
    ui: &AppWindow,
    wireless: &Wireless,
    state: &RefCell<UiState>,
    pending: &RefCell<Option<PendingAction>>,
    deferred_action: &mut Option<PendingAction>,
) {
    let Some(action) = pending.borrow_mut().take() else {
        return;
    };

    match action {
        PendingAction::WifiToggle => {
            let mut ui_state = state.borrow_mut();
            ui_state.wifi_enabled = !ui_state.wifi_enabled;
            let enabled = ui_state.wifi_enabled;
            ui.set_wifi_enabled(enabled);
            if enabled {
                ui.set_wifi_status("WiFi 已打开".into());
                drop(ui_state);
                refresh_wifi_ui(ui, wireless, &state.borrow());
            } else {
                ui.set_wifi_status("WiFi 已关闭".into());
                *deferred_action = Some(action);
            }
        }
        PendingAction::WifiScan => {
            if !state.borrow().wifi_enabled {
                ui.set_wifi_status("WiFi 已关闭".into());
                refresh_wifi_ui(ui, wireless, &state.borrow());
                return;
            }
            ui.set_wifi_status("正在扫描".into());
            *deferred_action = Some(action);
        }
        PendingAction::WifiPrevious => {
            let mut ui_state = state.borrow_mut();
            ui_state.wifi_selected = ui_state.wifi_selected.saturating_sub(1);
            drop(ui_state);
            refresh_wifi_ui(ui, wireless, &state.borrow());
        }
        PendingAction::WifiNext => {
            state.borrow_mut().wifi_selected += 1;
            refresh_wifi_ui(ui, wireless, &state.borrow());
        }
        PendingAction::WifiConnect => {
            if !state.borrow().wifi_enabled {
                ui.set_wifi_status("WiFi 已关闭".into());
                refresh_wifi_ui(ui, wireless, &state.borrow());
                return;
            }
            let ssid = selected_wifi_ssid(wireless, &state.borrow());
            if ssid.is_empty() {
                ui.set_wifi_status("请先扫描".into());
                return;
            }
            if selected_wifi_requires_password(wireless, &state.borrow())
                && state.borrow().wifi_password.is_empty()
            {
                {
                    let mut ui_state = state.borrow_mut();
                    ui_state.mode = UiMode::WifiAdvanced;
                    ui_state.wifi_edit_field = WifiEditField::Password;
                }
                update_wifi_input_ui(ui, &state.borrow());
                ui.set_screen(6);
                ui.set_wifi_status("输入密码".into());
                return;
            }
            ui.set_wifi_status(format!("正在连接 {}", ssid).into());
            *deferred_action = Some(action);
        }
        PendingAction::WifiDisconnect => {
            ui.set_wifi_status("正在断开".into());
            *deferred_action = Some(action);
        }
        PendingAction::BluetoothRefresh => {
            ui.set_bluetooth_status("正在广播".into());
            *deferred_action = Some(action);
        }
    }
}

fn execute_deferred_action(
    ui: &AppWindow,
    wireless: &mut Wireless,
    state: &RefCell<UiState>,
    action: PendingAction,
    mut on_wait: impl FnMut(),
) {
    match action {
        PendingAction::WifiToggle => {
            if !state.borrow().wifi_enabled {
                let _ = wireless.disconnect_station();
                ui.set_wifi_connected(false);
            }
            refresh_wifi_ui(ui, wireless, &state.borrow());
        }
        PendingAction::WifiScan => {
            match wireless.scan_networks_cooperative(&mut on_wait) {
                Ok(networks) => {
                    state.borrow_mut().wifi_selected = 0;
                    ui.set_wifi_status(format!("找到 {} 个网络", networks.len()).into());
                }
                Err(error) => ui.set_wifi_status(format!("扫描失败：{}", wifi_error(error)).into()),
            }
            refresh_wifi_ui(ui, wireless, &state.borrow());
        }
        PendingAction::WifiConnect => {
            let ssid = selected_wifi_ssid(wireless, &state.borrow());
            let password = state.borrow().wifi_password.clone();
            match wireless.connect_station_cooperative(&ssid, &password, &mut on_wait) {
                Ok(status) => {
                    let name = status.ssid.unwrap_or(ssid);
                    ui.set_wifi_status(format!("已连接 {}", name).into());
                    ui.set_wifi_connected(true);
                    set_mode(ui, state, UiMode::Wifi);
                }
                Err(error) => {
                    ui.set_wifi_status(format!("连接失败：{}", wifi_error(error)).into());
                    ui.set_wifi_connected(false);
                }
            }
            refresh_wifi_ui(ui, wireless, &state.borrow());
        }
        PendingAction::WifiDisconnect => {
            let disconnected = wireless.disconnect_station();
            {
                let mut state = state.borrow_mut();
                state.wifi_manual_ssid.clear();
                state.wifi_password.clear();
            }
            match disconnected {
                Ok(_) => ui.set_wifi_status("已断开".into()),
                Err(error) => {
                    ui.set_wifi_status(format!("断开失败：{}", wifi_error(error)).into());
                }
            }
            refresh_wifi_ui(ui, wireless, &state.borrow());
        }
        PendingAction::BluetoothRefresh => {
            match wireless.restart_bluetooth_advertising() {
                Ok(()) => ui.set_bluetooth_status("正在作为从机广播".into()),
                Err(BleActionError::Connected) => {
                    ui.set_bluetooth_status("从机已连接".into());
                }
                Err(BleActionError::NotInitialized) => ui.set_bluetooth_status("蓝牙不可用".into()),
                Err(BleActionError::Advertise) => ui.set_bluetooth_status("广播失败".into()),
            }
            ui.set_bluetooth_connected(wireless.bluetooth_connected());
        }
        PendingAction::WifiPrevious | PendingAction::WifiNext => {}
    }
}

fn refresh_wifi_ui(ui: &AppWindow, wireless: &Wireless, state: &UiState) {
    let networks = wireless.latest_scan();
    ui.set_wifi_enabled(state.wifi_enabled);

    if !state.wifi_enabled {
        ui.set_wifi_network_title("WiFi 已关闭".into());
        ui.set_wifi_network_detail("".into());
        set_wifi_row(ui, 0, "打开后扫描", "", false);
        set_wifi_row(ui, 1, "", "", false);
        set_wifi_row(ui, 2, "", "", false);
        ui.set_wifi_connected(false);
        update_wifi_input_ui(ui, state);
        return;
    }

    let selected = state.wifi_selected.min(networks.len().saturating_sub(1));
    let start = visible_wifi_start(selected);
    if networks.is_empty() {
        ui.set_wifi_network_title("未扫描".into());
        ui.set_wifi_network_detail("点击扫描".into());
        set_wifi_row(ui, 0, "未扫描", "点击扫描", false);
        set_wifi_row(ui, 1, "", "", false);
        set_wifi_row(ui, 2, "", "", false);
    } else {
        let selected_network = &networks[selected];
        ui.set_wifi_network_title(selected_network.ssid.clone().into());
        ui.set_wifi_network_detail(format!("{}/{}", selected + 1, networks.len()).into());
        for row in 0..3 {
            if let Some(network) = networks.get(start + row) {
                let detail = format!(
                    "{} dBm · {}",
                    network.signal_strength,
                    auth_label(network.auth_method)
                );
                set_wifi_row(ui, row, &network.ssid, &detail, start + row == selected);
            } else {
                set_wifi_row(ui, row, "", "", false);
            }
        }
    }

    let status = wireless.wifi_status();
    ui.set_wifi_connected(status.connected);
    ui.set_wifi_status_detail(format_wifi_status_detail(&status).into());
    update_wifi_input_ui(ui, state);
}

fn refresh_bluetooth_ui(ui: &AppWindow, wireless: &Wireless) {
    if wireless.bluetooth_connected() {
        if let Some(len) = wireless.bluetooth_last_write_len().filter(|len| *len > 0) {
            ui.set_bluetooth_status(format!("从机已连接 · 收到 {} 字节", len).into());
        } else {
            ui.set_bluetooth_status("从机已连接".into());
        }
    } else {
        ui.set_bluetooth_status("正在作为从机广播".into());
    }
    ui.set_bluetooth_connected(wireless.bluetooth_connected());
}

fn visible_wifi_start(selected: usize) -> usize {
    selected.saturating_sub(1)
}

fn set_wifi_row(ui: &AppWindow, row: usize, title: &str, detail: &str, selected: bool) {
    match row {
        0 => {
            ui.set_wifi_row0_title(title.into());
            ui.set_wifi_row0_detail(detail.into());
            ui.set_wifi_row0_selected(selected);
        }
        1 => {
            ui.set_wifi_row1_title(title.into());
            ui.set_wifi_row1_detail(detail.into());
            ui.set_wifi_row1_selected(selected);
        }
        2 => {
            ui.set_wifi_row2_title(title.into());
            ui.set_wifi_row2_detail(detail.into());
            ui.set_wifi_row2_selected(selected);
        }
        _ => {}
    }
}

fn update_wifi_row_selection(ui: &AppWindow, selected: usize) {
    let start = visible_wifi_start(selected);
    ui.set_wifi_row0_selected(start == selected);
    ui.set_wifi_row1_selected(start + 1 == selected);
    ui.set_wifi_row2_selected(start + 2 == selected);
}

fn is_horizontal_swipe(start_x: u16, start_y: u16, end_x: u16, end_y: u16) -> bool {
    let dx = start_x.abs_diff(end_x);
    let dy = start_y.abs_diff(end_y);
    dx >= SWIPE_THRESHOLD && dx > dy
}

fn selected_wifi_ssid(wireless: &Wireless, state: &UiState) -> String {
    if !state.wifi_manual_ssid.is_empty() {
        return state.wifi_manual_ssid.clone();
    }

    let networks = wireless.latest_scan();
    networks
        .get(state.wifi_selected.min(networks.len().saturating_sub(1)))
        .map(|network| network.ssid.clone())
        .unwrap_or_default()
}

fn selected_wifi_requires_password(wireless: &Wireless, state: &UiState) -> bool {
    if !state.wifi_manual_ssid.is_empty() {
        return !state.wifi_password.is_empty();
    }

    let networks = wireless.latest_scan();
    networks
        .get(state.wifi_selected.min(networks.len().saturating_sub(1)))
        .map(|network| network.auth_method != WifiAuthMethod::Open)
        .unwrap_or(false)
}

fn push_wifi_input(state: &mut UiState, key: &str) {
    match state.wifi_edit_field {
        WifiEditField::Ssid => {
            if state.wifi_manual_ssid.len() + key.len() <= 32 {
                state.wifi_manual_ssid.push_str(key);
            }
        }
        WifiEditField::Password => {
            if state.wifi_password.len() + key.len() <= 64 {
                state.wifi_password.push_str(key);
            }
        }
    }
}

fn pop_wifi_input(state: &mut UiState) {
    match state.wifi_edit_field {
        WifiEditField::Ssid => {
            state.wifi_manual_ssid.pop();
        }
        WifiEditField::Password => {
            state.wifi_password.pop();
        }
    }
}

fn clear_wifi_input(state: &mut UiState) {
    match state.wifi_edit_field {
        WifiEditField::Ssid => state.wifi_manual_ssid.clear(),
        WifiEditField::Password => state.wifi_password.clear(),
    }
}

fn update_wifi_input_ui(ui: &AppWindow, state: &UiState) {
    let ssid = if state.wifi_manual_ssid.is_empty() {
        "<列表>".into()
    } else {
        truncate_with_len(&state.wifi_manual_ssid, 20).into()
    };
    ui.set_wifi_manual_ssid(ssid);
    ui.set_wifi_password(mask_password(&state.wifi_password));
    ui.set_wifi_edit_label(match state.wifi_edit_field {
        WifiEditField::Ssid => "编辑 SSID".into(),
        WifiEditField::Password => "编辑密码".into(),
    });
}

fn format_wifi_status_detail(status: &crate::radio::WifiStatus) -> String {
    if !status.initialized {
        return "WiFi 不可用".into();
    }

    if status.connected {
        let rssi = status
            .rssi
            .map(|value| format!("{} dBm", value))
            .unwrap_or_else(|| "-- dBm".into());
        let channel = status
            .channel
            .map(|value| format!("ch {}", value))
            .unwrap_or_else(|| "ch --".into());
        format!("{} · {}", channel, rssi)
    } else {
        "未连接".into()
    }
}

fn wifi_error(error: WifiActionError) -> &'static str {
    match error {
        WifiActionError::NotInitialized => "未就绪",
        WifiActionError::InvalidInput => "输入错误",
        WifiActionError::Configure => "配置失败",
        WifiActionError::Scan => "扫描失败",
        WifiActionError::Connect => "连接失败",
        WifiActionError::Disconnect => "断开失败",
    }
}

fn auth_label(method: WifiAuthMethod) -> &'static str {
    match method {
        WifiAuthMethod::Open => "开放",
        WifiAuthMethod::Wep => "WEP",
        WifiAuthMethod::Wpa => "WPA",
        WifiAuthMethod::Wpa2Personal => "WPA2",
        WifiAuthMethod::WpaWpa2Personal => "WPA/WPA2",
        WifiAuthMethod::Wpa2Enterprise => "WPA2-E",
        WifiAuthMethod::Wpa3Personal => "WPA3",
        WifiAuthMethod::Wpa2Wpa3Personal => "WPA2/WPA3",
        WifiAuthMethod::Other => "加密",
        WifiAuthMethod::Unknown => "未知",
    }
}

fn mask_password(password: &str) -> SharedString {
    if password.is_empty() {
        return "".into();
    }

    let visible_len = password.len().min(18);
    let mut masked = "*".repeat(visible_len);
    if password.len() > visible_len {
        masked.push_str("...");
    }
    masked.push_str(&format!("({}/64)", password.len()));
    masked.into()
}

fn truncate_with_len(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return format!("{}({}/32)", value, value.len());
    }

    let mut output = value.chars().take(max_chars).collect::<String>();
    output.push_str("...");
    output.push_str(&format!("({}/32)", value.len()));
    output
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
        ui.set_time_status("RTC 读取失败".into());
        return false;
    };

    {
        let mut state = state.borrow_mut();
        state.mode = UiMode::Adjust;
        state.draft = Some(datetime);
    }
    apply_clock_ui(ui, datetime);
    ui.set_time_status("调整 RTC 时间".into());
    ui.set_screen(4);
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

    match rtc.write_datetime(datetime) {
        Ok(()) => {
            ui.set_time_status("已保存".into());
            exit_adjust(ui, state);
            apply_clock_ui(ui, datetime);
        }
        Err(_) => ui.set_time_status("RTC 写入失败".into()),
    }
}

fn cancel_adjust(ui: &AppWindow, rtc: Pcf85063, state: &RefCell<UiState>) {
    exit_adjust(ui, state);
    update_clock_ui(ui, rtc);
}

fn exit_adjust(ui: &AppWindow, state: &RefCell<UiState>) {
    let mut state = state.borrow_mut();
    state.mode = UiMode::Settings;
    state.draft = None;
    ui.set_screen(1);
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
