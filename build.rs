use flate2::read::GzDecoder;
use std::{
    env,
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

const ASSETS_FLASH_BASE: u32 = 0x800000;
const ASSETS_FLASH_CAPACITY: usize = 8 * 1024 * 1024;
const ASSET_MAGIC: &[u8; 8] = b"ESPAST01";
const ASSET_VERSION: u16 = 1;
const ASSET_TABLE_ENTRY_SIZE: usize = 48;
const FONT_ASSET_NAME: &str = "font/unifont16.bmf";
const UNIFONT_RECORD_SIZE: usize = 35;
const UNIFONT_SOURCE: &str = "ui/assets/unifont_all-17.0.04.hex.gz";

fn main() {
    linker_be_nice();
    // 先生成外部 flash 资源包，再编译 Slint UI；运行时字体表依赖该资源包。
    let assets_len = generate_display_assets();
    generate_assets_manifest(assets_len);
    slint_build::compile_with_config(
        "ui/main.slint",
        slint_build::CompilerConfiguration::new()
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer),
    )
    .unwrap();
    println!("cargo:rustc-link-arg-tests=-Tembedded-test.x");
    println!("cargo:rustc-link-arg=-Tdefmt.x");
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}

fn generate_display_assets() -> usize {
    // 资源包会被 runner 单独烧录到 ASSETS_FLASH_BASE，不会自动嵌入主 ELF。
    let font = generate_unifont_table();
    let package = pack_assets(&[(FONT_ASSET_NAME, font.as_slice())]);
    assert!(
        package.len() <= ASSETS_FLASH_CAPACITY,
        "display assets package is larger than reserved flash region"
    );

    let output_dir = target_dir().join("display-assets");
    fs::create_dir_all(&output_dir).expect("failed to create display assets output directory");
    let output_path = output_dir.join("assets.bin");
    fs::write(&output_path, &package).expect("failed to write display assets package");

    println!("cargo:rerun-if-changed={UNIFONT_SOURCE}");
    println!("cargo:warning=display assets: {}", output_path.display());

    package.len()
}

fn generate_unifont_table() -> Vec<u8> {
    let source = File::open(UNIFONT_SOURCE).expect("failed to open Unifont hex gzip");
    let reader = BufReader::new(GzDecoder::new(source));
    let mut output = Vec::with_capacity(65_536 * UNIFONT_RECORD_SIZE);

    for line in reader.lines() {
        let line = line.expect("failed to read Unifont line");
        let Some((codepoint, bitmap)) = line.split_once(':') else {
            continue;
        };
        let codepoint = u32::from_str_radix(codepoint, 16).expect("invalid Unifont codepoint");
        if codepoint > 0xffff {
            continue;
        }

        let width = match bitmap.len() {
            32 => 8u8,
            64 => 16u8,
            _ => panic!("invalid Unifont bitmap width for U+{codepoint:04X}"),
        };

        // 每个 BMP 字符固定 35 字节：码点、宽度、16 行 x 2 字节位图。
        output.extend_from_slice(&(codepoint as u16).to_be_bytes());
        output.push(width);

        if width == 8 {
            for row in 0..16 {
                let byte = parse_hex_byte(&bitmap[row * 2..row * 2 + 2]);
                output.extend_from_slice(&[byte, 0]);
            }
        } else {
            for row in 0..16 {
                let offset = row * 4;
                let high = parse_hex_byte(&bitmap[offset..offset + 2]);
                let low = parse_hex_byte(&bitmap[offset + 2..offset + 4]);
                output.extend_from_slice(&[high, low]);
            }
        }
    }

    output
}

fn pack_assets(assets: &[(&str, &[u8])]) -> Vec<u8> {
    assert!(assets.len() <= u16::MAX as usize, "too many display assets");

    // 简单的只读资源包格式：header + 固定长度表项 + 连续数据区。
    let table_offset = 20usize;
    let data_offset = table_offset + assets.len() * ASSET_TABLE_ENTRY_SIZE;
    let mut package =
        Vec::with_capacity(data_offset + assets.iter().map(|(_, data)| data.len()).sum::<usize>());

    package.extend_from_slice(ASSET_MAGIC);
    package.extend_from_slice(&ASSET_VERSION.to_le_bytes());
    package.extend_from_slice(&(assets.len() as u16).to_le_bytes());
    package.extend_from_slice(&(table_offset as u32).to_le_bytes());
    package.extend_from_slice(&(data_offset as u32).to_le_bytes());

    let mut cursor = 0u32;
    for (name, data) in assets {
        let name_bytes = name.as_bytes();
        assert!(
            name_bytes.len() <= 31,
            "asset name must fit in 31 bytes plus nul terminator"
        );

        let mut name_field = [0u8; 32];
        name_field[..name_bytes.len()].copy_from_slice(name_bytes);
        package.extend_from_slice(&name_field);
        package.extend_from_slice(&cursor.to_le_bytes());
        package.extend_from_slice(&(data.len() as u32).to_le_bytes());
        package.extend_from_slice(&crc32(data).to_le_bytes());
        package.extend_from_slice(&0u32.to_le_bytes());

        cursor = cursor
            .checked_add(data.len() as u32)
            .expect("display asset package is too large");
    }

    for (_, data) in assets {
        package.extend_from_slice(data);
    }

    package
}

fn generate_assets_manifest(assets_len: usize) {
    // 固件侧通过 include!(OUT_DIR/assets_manifest.rs) 获取 flash 基地址和包长度。
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    let output = File::create(out_dir.join("assets_manifest.rs"))
        .expect("failed to create display assets manifest");
    let mut output = BufWriter::new(output);

    writeln!(
        output,
        "pub const ASSETS_FLASH_BASE: u32 = 0x{ASSETS_FLASH_BASE:08x};"
    )
    .expect("failed to write display assets manifest");
    writeln!(
        output,
        "pub const ASSETS_FLASH_CAPACITY: usize = {ASSETS_FLASH_CAPACITY};"
    )
    .expect("failed to write display assets manifest");
    writeln!(
        output,
        "pub const ASSETS_PACKAGE_LEN: usize = {assets_len};"
    )
    .expect("failed to write display assets manifest");
}

fn target_dir() -> PathBuf {
    if let Some(target_dir) = env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(target_dir);
    }

    Path::new(&env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"))
        .join("target")
}

fn parse_hex_byte(hex: &str) -> u8 {
    u8::from_str_radix(hex, 16).expect("invalid Unifont bitmap byte")
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                what if what.starts_with("_defmt_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler."
                    );
                    eprintln!();
                }
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!(
                        "💡 Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    println!(
        "cargo:rustc-link-arg=-Wl,--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}
