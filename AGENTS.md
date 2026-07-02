# AGENTS.md

## Project Overview

This repository is a Rust embedded firmware project for the Waveshare
ESP32-S3-Touch-LCD-3.49 board.

Use the Waveshare documentation, hardware interface image, and schematic as the
source of truth for board-level hardware details:

- Product documentation: https://docs.waveshare.net/ESP32-S3-Touch-LCD-3.49/?variant=ESP32-S3-Touch-LCD-3.49
- Waveshare wiki mirror: https://www.waveshare.com/wiki/ESP32-S3-Touch-LCD-3.49
- Schematic link from Waveshare resources: https://www.waveshare.net/w/upload/2/20/ESP32-S3-Touch-LCD-3.49-Schematic.pdf

The Cargo target is `xtensa-esp32s3-none-elf`. Treat this as firmware for the
physical board, not as a desktop Slint application. `ui/main.slint` is compiled
into the firmware and rendered through Slint's software renderer onto the LCD.

## Hardware

The board is based on an `ESP32-S3R8` module-class design: dual-core Xtensa LX7
up to 240 MHz, 2.4 GHz Wi-Fi, Bluetooth 5 LE, onboard antenna, 8 MB PSRAM, and
external 16 MB SPI flash. The board also exposes an IPEX/U.FL antenna connector
that can be enabled by changing the documented resistor population.

### Display And Touch

- LCD panel: 3.49 inch IPS, `172 x 640` native resolution, 16.7M colors,
  350 cd/m2 brightness, 1200:1 contrast.
- LCD driver IC: `AXS15231B`.
- LCD data interface: QSPI-style 4-bit SPI bus.
- Current firmware logical orientation: `640 x 172` landscape.
- Touch type: capacitive touch.
- Touch interface: I2C.

LCD and touch signals:

| Signal | ESP32-S3 / EXIO | Notes |
| --- | --- | --- |
| `LCD_BL` | `GPIO8` | LCD backlight PWM/control input in the Waveshare hardware map. |
| `LCD_CS` | `GPIO9` | LCD chip select. |
| `LCD_SCL` | `GPIO10` | LCD QSPI/SPI clock. |
| `LCD_D0` | `GPIO11` | LCD QSPI data 0. |
| `LCD_D1` | `GPIO12` | LCD QSPI data 1. |
| `LCD_D2` | `GPIO13` | LCD QSPI data 2. |
| `LCD_D3` | `GPIO14` | LCD QSPI data 3. |
| `TP_SDA` | `GPIO17` | Touch I2C data. Current code uses I2C1 SDA here. |
| `TP_SCL` | `GPIO18` | Touch I2C clock. Current code uses I2C1 SCL here. |
| `LCD_RST` | `GPIO21` | LCD reset in the Waveshare interface map. Check the schematic before changing reset routing. |
| `TP_INT` | `EXIO0` | Touch interrupt through the TCA9554 I/O expander. |
| `BL_EN` | `EXIO1` | Backlight enable through the TCA9554 I/O expander. |
| `LCD_TE` | `EXIO5` | LCD tearing-effect signal. |

Current firmware details:

- `src/lcd/axs15231b.rs` drives the LCD as an `AXS15231B`.
- `src/board.rs` currently configures SPI3 at 40 MHz, SPI mode 3, with
  `GPIO9..GPIO14` and DMA channel `DMA_CH0`.
- `src/lcd/touch.rs` currently reads touch I2C address `0x3b`.
- `src/lcd/tca9554.rs` currently uses TCA9554 address `0x20` for backlight and
  reset-related control. Before changing EXIO behavior, compare the code bit
  assignments with the schematic and the actual board variant.

### Shared I2C Peripherals

The board uses `GPIO47` and `GPIO48` as the shared I2C bus for several onboard
peripherals:

| Peripheral | SDA | SCL | Extra Signals |
| --- | --- | --- | --- |
| `QMI8658` IMU | `GPIO47` / `IMU_SDA` | `GPIO48` / `IMU_SCL` | `IMU_INT1=EXIO2`, `IMU_INT2=EXIO3` |
| `PCF85063` RTC | `GPIO47` / `RTC_SDA` | `GPIO48` / `RTC_SCL` | `RTC_INT=EXIO4` |
| Audio codec control | `GPIO47` / `Audio_SDA` | `GPIO48` / `Audio_SCL` | Used with the I2S audio path. |
| `TCA9554PWR` I/O expander | shared I2C bus | shared I2C bus | `EXIO_INT=GPIO42` |

Current firmware initializes the shared I2C bus as I2C0 with
`SDA=GPIO47`, `SCL=GPIO48`, 400 kHz, and currently uses it for `TCA9554` and
`PCF85063`.

### IMU

- Device: `QMI8658`.
- Function: 6-axis sensor, 3-axis accelerometer plus 3-axis gyroscope.
- Interface: shared I2C on `GPIO47`/`GPIO48`.
- Interrupts: `IMU_INT1=EXIO2`, `IMU_INT2=EXIO3`.

The IMU is documented by Waveshare and present in the hardware map, but this
repository does not currently contain a QMI8658 driver. Add one only after
checking the schematic, address selection, and EXIO interrupt routing.

### RTC

- Device: `PCF85063`.
- Interface: shared I2C on `GPIO47`/`GPIO48`.
- Interrupt: `RTC_INT=EXIO4`.
- Current firmware address: `0x51`.
- Current driver: `src/rtc/pcf85063.rs`.

### SD Card

The board includes a TF/microSD card slot. Waveshare's hardware map names the
signals as SPI-style SD lines:

| Signal | ESP32-S3 GPIO |
| --- | --- |
| `SD_CS` | `GPIO38` |
| `SD_MOSI` | `GPIO39` |
| `SD_MISO` | `GPIO40` |
| `SD_SCLK` | `GPIO41` |

The SD card slot is not currently initialized by this firmware. If adding SD
support, check whether the selected Rust driver should use SPI mode with these
signals or an SD/MMC abstraction compatible with the board support code.

### Audio

The board includes an audio input/output subsystem:

- Output codec/DAC: `ES8311`.
- Input codec/ADC: `ES7210`.
- Microphones: onboard dual-microphone array.
- Speaker output: MX1.25 2-pin speaker connector.
- Echo/noise-support circuitry is present on the board per Waveshare's feature
  description.

Audio I2S/control signals:

| Signal | ESP32-S3 / EXIO | Notes |
| --- | --- | --- |
| `I2S_DSOUT` | `GPIO6` | I2S data from codec/ADC toward ESP32-S3. |
| `I2S_MCLK` | `GPIO7` | I2S master clock. |
| `I2S_SCLK` | `GPIO15` | I2S bit clock. |
| `I2S_DSDIN` | `GPIO45` | I2S data from ESP32-S3 toward codec/DAC. |
| `I2S_LRCK` | `GPIO46` | I2S word select / LR clock. |
| `Audio_SDA` | `GPIO47` | Audio codec control I2C data. |
| `Audio_SCL` | `GPIO48` | Audio codec control I2C clock. |
| `NS_MODE` | `EXIO7` | Noise-suppression / audio mode control in the Waveshare map. |

This repository does not currently initialize the audio codecs, microphones, or
speaker path.

### USB, UART, And Debug

- USB Type-C port: used for firmware flashing and log/serial access.
- USB D-/D+ nets: `U_N=GPIO19`, `U_P=GPIO20`.
- UART signals in the hardware map: `TXD=GPIO43`, `RXD=GPIO44`.
- RESET button: connected to reset.
- BOOT button: `BOOT0=GPIO0`; press BOOT and RESET together to enter download
  mode when needed.

Current firmware uses RTT/defmt logging. Do not assume UART logging is enabled
unless the firmware is explicitly changed to use it.

### Buttons, Battery, And Power Control

- BOOT button: `GPIO0`.
- RESET button: reset line.
- PWR button: board power-control path, especially for lithium-battery use.
- Battery connector: MX1.25 2-pin 3.7 V lithium battery charge/discharge
  header.
- Battery ADC: `BAT_ADC=GPIO4`.
- System output/control nets: `SYS_OUT=GPIO16`, `SYS_EN=EXIO6`.
- Backlight power: AP3032-based LCD backlight circuit, controlled by `LCD_BL`
  and `BL_EN`.
- Power management and charging circuitry are present on the schematic; check
  the schematic before changing any battery, SYS_EN, SYS_OUT, or backlight
  behavior.

### I/O Expander And EXIO Map

The board uses a `TCA9554PWR` 8-bit I2C GPIO expander. The hardware map exposes
these EXIO functions:

| EXIO | Mapped Function |
| --- | --- |
| `EXIO0` | `TP_INT` |
| `EXIO1` | `BL_EN` |
| `EXIO2` | `IMU_INT1` |
| `EXIO3` | `IMU_INT2` |
| `EXIO4` | `RTC_INT` |
| `EXIO5` | `LCD_TE` |
| `EXIO6` | `SYS_EN` |
| `EXIO7` | `NS_MODE` |

The expander interrupt is mapped to `GPIO42` as `EXIO_INT`.

### Expansion Pads And General GPIO

The board reserves a 22-pin 2.54 mm through-hole expansion area. The Waveshare
hardware map also marks many GPIO and EXIO signals as available outputs or
expansion signals. Before using a pad as free GPIO, check the interface map for
conflicts with LCD, touch, SD, IMU, RTC, audio, USB, buttons, power, and EXIO.

Signals that are especially not free in the default hardware map include:

- `GPIO0`: BOOT0.
- `GPIO4`: `BAT_ADC`.
- `GPIO6`, `GPIO7`, `GPIO15`, `GPIO45`, `GPIO46`: I2S audio.
- `GPIO8` through `GPIO14`, `GPIO17`, `GPIO18`, `GPIO21`: LCD/touch.
- `GPIO19`, `GPIO20`: USB D-/D+.
- `GPIO38` through `GPIO41`: SD card.
- `GPIO42`: EXIO interrupt.
- `GPIO43`, `GPIO44`: UART TX/RX.
- `GPIO47`, `GPIO48`: shared I2C.
- `EXIO0` through `EXIO7`: touch/backlight/IMU/RTC/LCD_TE/system/audio mode
  functions.

## Software Architecture

This is a `no_std` / `no_main` firmware with `alloc` enabled. It uses
`esp-hal`, `esp-rtos`, `esp-radio`, `slint` with the software renderer, and
`defmt` over RTT. The configured target is `xtensa-esp32s3-none-elf`; the
project relies on `.cargo/config.toml` for target selection, `DEFMT_LOG=info`,
`-nostartfiles`, and `build-std = ["alloc", "core"]`.

### Boot And Ownership Model

`src/main.rs` owns only bootstrapping:

1. Initializes RTT/defmt logging.
2. Creates the ESP-IDF-compatible app descriptor required by the ESP bootloader.
3. Starts ESP-HAL at maximum CPU clock.
4. Takes ESP32-S3 peripherals and passes them into `Board::init`.
5. Hands the fully initialized board to `app::run`, which never returns.

`src/board.rs` is the hardware ownership boundary. Peripheral ownership should
be acquired there and moved into a driver or service. Avoid taking raw
peripherals in application code. `Board::init` currently initializes:

- two heap regions plus PSRAM allocation,
- flash-backed display assets,
- `esp-rtos`,
- shared I2C0 for TCA9554 and PCF85063,
- touch I2C1,
- SPI3 + DMA for the LCD,
- LCD reset/backlight setup,
- Wi-Fi and BLE radio wrappers.

### Main Runtime Loop

`app::run` is a single-threaded polling loop. Slint does not run its own native
event loop here; `EspPlatform::run_event_loop` is intentionally a no-op.
Instead, the firmware loop explicitly performs all work:

1. Poll BLE controller events and refresh Wi-Fi/BLE connection indicators.
2. Run Slint timers and animations.
3. Read the RTC once per second while in display mode.
4. Poll the touch controller and translate touch points into Slint pointer
   pressed/moved/released events.
5. Render the Slint window by line into a `640 x 172` RGB565 frame buffer.
6. Patch the status text overlay from firmware-side font drawing when in
   display mode.
7. Rotate/copy the logical landscape frame into the LCD native `172 x 640`
   memory order and flush it through the AXS15231B QSPI path.
8. Pace the loop to roughly 16 ms per frame with a busy wait.

Do not introduce blocking operations in this loop without accounting for touch
latency, display refresh, BLE polling, and RTC updates.

### UI And Rendering

`ui/main.slint` defines the embedded UI. `build.rs` compiles it with
`slint-build` and embeds resources for the software renderer. Runtime rendering
uses `MinimalSoftwareWindow` with `RepaintBufferType::ReusedBuffer`; drawing is
performed line-by-line through `DisplayLines`.

The LCD driver accepts the logical landscape frame as `640 x 172`, then
`flush_landscape_frame` converts it into the panel's native `172 x 640`
orientation. Keep logical UI coordinates in the landscape coordinate system
unless you are deliberately changing the display orientation contract.

The status text overlay is not pure Slint text: `src/text/` reads a generated
Unifont bitmap table from flash assets and draws status text into the frame
buffer after Slint rendering. When changing fonts, text placement, or status
rendering, check both `src/text/` and the asset pipeline in `build.rs`.

### Touch And Clock Behavior

Touch is polled from `src/lcd/touch.rs`. The driver reports logical landscape
coordinates and emits a final release point after the controller reports no
active touch. `app::run` uses those events differently by mode:

- Display mode: a long press enters clock adjustment mode.
- Adjustment mode: touch points are dispatched into Slint controls as pointer
  events.
- Adjustment mode exits on confirm/cancel or idle timeout.

The RTC driver in `src/rtc/pcf85063.rs` stores and reads BCD date/time fields.
RTC read errors intentionally surface as unknown UI text such as `--:--:--`
instead of fake clock values.

### Wireless Behavior

`src/radio/wireless.rs` wraps Wi-Fi and BLE initialization so either subsystem
can fail independently without preventing the board from booting. The main loop
currently polls BLE events and reflects Wi-Fi/BLE connection state into the UI.
Wi-Fi scanning and station connection APIs exist, but this firmware does not
yet provide a complete network configuration UI or persistence layer.

### Asset Pipeline And Flash Layout

`build.rs` generates display assets before compiling the firmware:

- converts `ui/assets/unifont_all-17.0.04.hex.gz` into a compact bitmap font
  table,
- packs named assets into a custom `ESPAST01` package,
- writes `target/display-assets/assets.bin`,
- exports the asset package length for the firmware build.

The asset package is stored in a separate flash area:

- base address: `0x800000`,
- maximum reserved capacity in the current code: 8 MiB,
- reader module: `src/assets/`.

The firmware expects this package to exist in flash. That is why `cargo run`
uses the project runner to download the asset binary before running the ELF.

### Change Boundaries

- Hardware initialization belongs in `Board::init`; application logic should
  receive initialized drivers through `Board`.
- Slint UI state and touch dispatch belong in `src/app.rs`; low-level touch I2C
  parsing belongs in `src/lcd/touch.rs`.
- LCD command, rotation, chunking, and DMA write behavior belong in
  `src/lcd/axs15231b.rs`; do not duplicate panel writes from the app layer.
- Flash asset package format changes must update `build.rs`,
  `src/assets/package.rs`, and the reader validation in `src/assets/mod.rs`
  together.
- Wireless initialization failures should remain non-fatal unless the product
  requirement changes; the current design allows display/clock functionality to
  continue without Wi-Fi or BLE.

## Development Commands

Use this command to check the code:

```sh
cargo clippy
```

Use this command to flash and run firmware:

```sh
cargo run
```

`cargo run` is configured by `.cargo/config.toml` to call
`scripts/cargo-run-with-assets.sh`. That runner first downloads
`target/display-assets/assets.bin` to flash address `0x800000` with
`probe-rs download --chip=esp32s3`, then runs the main ELF with
`probe-rs run --chip=esp32s3`.

Do not bypass `cargo run` and flash only the ELF unless you also handle the
display asset flash region.

## Coding Guidelines

- Use `ast-outline` to inspect structure before opening full source files.
- Keep hardware facts and firmware state separate: the board may have hardware
  that this firmware does not yet initialize.
- Before changing pins, clocks, DMA buffers, flash layout, or EXIO behavior,
  check the Waveshare schematic, the hardware interface map, `src/board.rs`,
  the relevant driver, and `build.rs` when assets are involved.
- LCD changes should check `src/lcd/axs15231b.rs`, `src/lcd/platform.rs`,
  `ui/main.slint`, and `build.rs`.
- Touch changes should check `src/lcd/touch.rs` and the touch dispatch paths in
  `src/app.rs`.
- RTC or time UI changes should check `src/rtc/pcf85063.rs` and `src/app.rs`.
- Wireless changes should check `src/radio/wireless.rs`; do not assume Wi-Fi or
  BLE has complete UI or persistent configuration.
- Do not replace unknown hardware state with fake zero values. Handle errors or
  unknown state explicitly.
- Before handing off code changes, run `cargo clippy`. When hardware behavior
  or flashing is part of the change, verify with `cargo run`.
