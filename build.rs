//! Build script: turns `board.toml` into everything else, and wires the linker.
//!
//! Two jobs.
//!
//! **1. Generate from one source of truth.** `board.toml` describes the
//! keyboard; this script emits
//!
//!   * `$OUT_DIR/board_generated.rs` - Rust constants for geometry, radio
//!     parameters, USB IDs/strings and storage
//!   * `$OUT_DIR/vial.json`          - the Vial definition, with its `matrix`
//!     block and layout derived from the same numbers
//!   * `$OUT_DIR/config_generated.rs`- the compressed Vial blob + keyboard ID
//!
//! so the things that previously had to agree by hand (keymap array dimensions,
//! the Gazell packing, vial.json's matrix block, the USB descriptors) cannot
//! drift. Everything is validated here, so a bad number is a build error with a
//! message rather than firmware that silently misbehaves over the air.
//!
//! **2. Linker wiring** - copied from the working `keypoint-rmk-dongle` build:
//! put `memory.x` where flip-link and the linker can find it, select
//! cortex-m-rt's and defmt's scripts, `--nmagic` (our FLASH origin is 0x1000,
//! not 64K-aligned), and link the closed-source Nordic Gazell archive.

use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use const_gen::*;
use xz2::read::XzEncoder;

// ---------------------------------------------------------------------------
// board.toml model
// ---------------------------------------------------------------------------

struct Board {
    chip: String,
    flash_origin: u32,
    flash_length: u32,
    ram_origin: u32,
    ram_length: u32,

    vid: u16,
    pid: u16,
    manufacturer: String,
    product_name: String,
    vial_name: String,
    vial_keyboard_id: Vec<u8>,

    rows_per_half: u32,
    cols_per_half: u32,
    halves: u32,

    channel_table: Vec<u8>,
    base_address_0: u32,
    base_address_1: u32,
    /// Host pipe serving the right half; the left half is always on pipe 0.
    /// Diagnostic: serve each half through the other pipe.
    /// Diagnostic: report link quality as self-typed keypresses.
    datarate: u32,
    timeslot_period_us: u32,
    link_timeout_ms: u32,
    poll_ms: u32,

    storage_start_addr: u32,
    storage_num_sectors: u32,
    /// Start of the region owned by the factory USB bootloader; storage must stay below it.
    flash_reserved_top: u32,
    /// `[power] reg0 = "ldo"` / `"dcdc"`: force REG0 (high-voltage regulator).
    reg0_ldo: bool,
    reg0_dcdc: bool,
    /// Same for REG1 (main regulator). See `reg_mode`.
    reg1_ldo: bool,
    reg1_dcdc: bool,
}

/// `toml::Value` accessors that fail with the field name in the message.
fn get<'a>(t: &'a toml::Value, table: &str, key: &str) -> &'a toml::Value {
    t.get(table)
        .and_then(|s| s.get(key))
        .unwrap_or_else(|| panic!("board.toml: missing [{table}] {key}"))
}

fn get_int(t: &toml::Value, table: &str, key: &str) -> i64 {
    get(t, table, key)
        .as_integer()
        .unwrap_or_else(|| panic!("board.toml: [{table}] {key} must be an integer"))
}

fn get_str(t: &toml::Value, table: &str, key: &str) -> String {
    get(t, table, key)
        .as_str()
        .unwrap_or_else(|| panic!("board.toml: [{table}] {key} must be a string"))
        .to_string()
}

fn as_u32(v: i64, what: &str) -> u32 {
    u32::try_from(v).unwrap_or_else(|_| panic!("board.toml: {what} must fit in u32 (got {v})"))
}

/// `[power] reg0` / `[power] reg1` - optional, one of `"keep"` (the default),
/// `"ldo"` or `"dcdc"`.
///
/// These are PER-BOARD decisions, not per-chip ones: what matters is the mode in
/// effect when the application starts, and that is not the chip's reset default.
/// The Adafruit UF2 bootloader switches REG1 to DC/DC before jumping into the
/// application, and DC/DC needs the external inductor that not every board
/// revision populates - so a firmware that never touches the register inherits a
/// mode it never chose, and on a board without that inductor the supply can
/// collapse as soon as the radio or USB draws current (the board looks written
/// and then completely dead).
///
/// The values below follow RMK's own chip defaults, which are what is known to
/// work on these boards (`[chip.nrf52840]` in
/// rmk/examples/use_config/nrf52840_ble/keyboard.toml):
///
///     dcdc_reg0 = true      dcdc_reg1 = true      dcdc_reg0_voltage = "3V3"
///
/// `"keep"` leaves the register exactly as the bootloader left it (what every
/// profile here did before the option existed); LDO is always safe, it just
/// costs idle current. The REG0 output *voltage* (UICR.REGOUT0) is deliberately
/// not touched: RMK warns that changing it needs bootloader >= 0.10.0, and 3V3
/// is what these boards are shipped with.
fn reg_mode(t: &toml::Value, key: &str) -> (bool, bool) {
    let mode = t
        .get("power")
        .and_then(|p| p.get(key))
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("board.toml: [power] {key} must be a string"))
        })
        .unwrap_or("keep");
    match mode {
        "keep" => (false, false),
        "ldo" => (true, false),
        "dcdc" => (false, true),
        other => panic!(
            "board.toml: [power] {key} = {other:?} must be \"keep\", \"ldo\" or \"dcdc\""
        ),
    }
}

/// Parse `0x1000`, `636K`, `1M` or a plain decimal into a byte count.
fn parse_num(s: &str) -> Option<u32> {
    let s = s.trim();
    let (digits, mult) = if let Some(d) = s.strip_suffix(['K', 'k']) {
        (d, 1024u32)
    } else if let Some(d) = s.strip_suffix(['M', 'm']) {
        (d, 1024 * 1024)
    } else {
        (s, 1)
    };
    let digits = digits.trim();
    let base = if digits.starts_with("0x") || digits.starts_with("0X") {
        16
    } else {
        10
    };
    let digits = digits.trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(digits, base).ok().map(|v| v * mult)
}

/// A byte count that may be written as a plain integer or as `"636K"` / `"256K"`.
fn get_bytes(t: &toml::Value, table: &str, key: &str) -> u32 {
    match get(t, table, key) {
        toml::Value::Integer(v) => as_u32(*v, &format!("{table}.{key}")),
        toml::Value::String(s) => parse_num(s).unwrap_or_else(|| {
            panic!("board.toml: [{table}] {key} = {s:?} is not a byte count (use 0x1000 or \"636K\")")
        }),
        _ => panic!("board.toml: [{table}] {key} must be an integer or a string like \"636K\""),
    }
}

/// Deep-merge `over` into `base`: a key present in `over` replaces the same key
/// in `base`, recursing into tables, so an override file states ONLY what
/// differs and every unstated value keeps coming from the single source.
///
/// This is how the board variants are supported without a copy of board.toml
/// per board. The keyboard/protocol half of board.toml (matrix, channels, USB
/// identity, Gazell parameters) is neither duplicated nor overridable by
/// accident: the variant files under boards/ carry `[chip]`, `[memory]`,
/// `[flash]` and `[storage]` - the things that depend on the bootloader and the
/// chip - and nothing else.
fn merge_toml(base: &mut toml::Value, over: &toml::Value) {
    match (base, over) {
        (toml::Value::Table(b), toml::Value::Table(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(slot) => merge_toml(slot, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (slot, v) => *slot = v.clone(),
    }
}

fn load_board() -> Board {
    let path = env::var("BOARD_TOML").unwrap_or_else(|_| "board.toml".to_string());
    println!("cargo:rerun-if-env-changed=BOARD_TOML");
    println!("cargo:rerun-if-changed={path}");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read board config {path}: {e}"));
    let mut t: toml::Value = text.parse().expect("board.toml is not valid TOML");

    // ---- per-board override -------------------------------------------------
    // board.toml describes THIS keyboard (matrix, link, USB identity) and the
    // dongle's layout. A variant that differs only in flash/RAM layout - a
    // nice!nano instead of the dongle, an nRF52833 instead of an nRF52840 -
    // names a file here instead of copying board.toml and hoping the four
    // copies stay in sync.
    //
    // Set by tools\build-variants.cmd. Everything downstream (the chip <-> cargo
    // feature cross-check, the storage-versus-application check, memory.x) runs
    // on the MERGED result, so an override cannot smuggle in a layout that the
    // existing validation does not understand.
    if let Ok(over) = env::var("BOARD_OVERRIDE") {
        println!("cargo:rerun-if-env-changed=BOARD_OVERRIDE");
        println!("cargo:rerun-if-changed={over}");
        let otext = fs::read_to_string(&over)
            .unwrap_or_else(|e| panic!("cannot read board override {over}: {e}"));
        let ov: toml::Value = otext
            .parse()
            .unwrap_or_else(|e| panic!("board override {over} is not valid TOML: {e}"));
        merge_toml(&mut t, &ov);
    }

    let rows_per_half = as_u32(get_int(&t, "matrix", "rows_per_half"), "matrix.rows_per_half");
    let cols_per_half = as_u32(get_int(&t, "matrix", "cols_per_half"), "matrix.cols_per_half");
    let halves = as_u32(get_int(&t, "matrix", "halves"), "matrix.halves");

    // ---- validate, with the reason in each message --------------------------
    if halves != 2 {
        panic!(
            "board.toml: [matrix] halves = {halves} is not supported: the Gazell link is \
             fixed at two pipes (pipe 0 = left, pipe 1 = right). Adding more halves means \
             changing the pump()/build_row() logic in src/gazell.rs as well."
        );
    }
    if cols_per_half == 0 || cols_per_half > 8 {
        panic!(
            "board.toml: [matrix] cols_per_half = {cols_per_half} must be 1..=8: each half \
             sends one byte per row, i.e. one bit per column."
        );
    }
    if rows_per_half == 0 || rows_per_half > 32 {
        panic!(
            "board.toml: [matrix] rows_per_half = {rows_per_half} must be 1..=32: one payload \
             byte per row, and Gazell's payload limit is 32 bytes \
             (NRF_GZLL_CONST_MAX_PAYLOAD_LENGTH)."
        );
    }

    let channel_table: Vec<u8> = get(&t, "gazell", "channel_table")
        .as_array()
        .expect("board.toml: [gazell] channel_table must be an array of integers")
        .iter()
        .map(|v| {
            let n = v.as_integer().expect("channel must be an integer");
            u8::try_from(n).unwrap_or_else(|_| panic!("channel {n} does not fit in u8"))
        })
        .collect();
    if channel_table.is_empty() || channel_table.len() > 16 {
        panic!(
            "board.toml: [gazell] channel_table has {} entries; Gazell allows 1..=16 \
             (NRF_GZLL_CONST_MAX_CHANNEL_TABLE_SIZE).",
            channel_table.len()
        );
    }
    for c in &channel_table {
        if *c > 100 {
            panic!("board.toml: channel {c} is outside the 2.4GHz band (0..=100)");
        }
    }

    let datarate = match get_str(&t, "gazell", "datarate").to_ascii_lowercase().as_str() {
        "250kbit" => 0,
        "1mbit" => 1,
        "2mbit" => 2,
        other => panic!("board.toml: [gazell] datarate \"{other}\" must be 250kbit, 1mbit or 2mbit"),
    };

    // Pipes 1..7 share base address 1, which is the right half's base address,
    // so any of them can serve it; pipe 0 is reserved for the left half, whose
    // base address is the only one base_address_0 can hold.
    // Diagnostic: serve each half through the other pipe. See board.toml.
    // Diagnostic: report link quality as self-typed keypresses. See board.toml.

    let vial_keyboard_id = {
        let s = get_str(&t, "device", "vial_keyboard_id");
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        if s.len() != 16 {
            panic!("board.toml: [device] vial_keyboard_id must be 16 hex digits (8 bytes), got {s:?}");
        }
        (0..8)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("vial_keyboard_id must be hex"))
            .collect()
    };

    Board {
        chip: get_str(&t, "chip", "name"),
        flash_origin: get_bytes(&t, "memory", "flash_origin"),
        flash_length: get_bytes(&t, "memory", "flash_length"),
        ram_origin: get_bytes(&t, "memory", "ram_origin"),
        ram_length: get_bytes(&t, "memory", "ram_length"),

        vid: as_u32(get_int(&t, "device", "vid"), "device.vid") as u16,
        pid: as_u32(get_int(&t, "device", "pid"), "device.pid") as u16,
        manufacturer: get_str(&t, "device", "manufacturer"),
        product_name: get_str(&t, "device", "product_name"),
        vial_name: get_str(&t, "device", "vial_name"),
        vial_keyboard_id,

        rows_per_half,
        cols_per_half,
        halves,

        channel_table,
        base_address_0: as_u32(get_int(&t, "gazell", "base_address_0"), "gazell.base_address_0"),
        base_address_1: as_u32(get_int(&t, "gazell", "base_address_1"), "gazell.base_address_1"),
        datarate,
        timeslot_period_us: as_u32(
            get_int(&t, "gazell", "timeslot_period_us"),
            "gazell.timeslot_period_us",
        ),
        link_timeout_ms: as_u32(get_int(&t, "gazell", "link_timeout_ms"), "gazell.link_timeout_ms"),
        poll_ms: as_u32(get_int(&t, "gazell", "poll_ms"), "gazell.poll_ms"),

        storage_start_addr: as_u32(
            get_int(&t, "storage", "start_addr"),
            "storage.start_addr",
        ),
        storage_num_sectors: as_u32(
            get_int(&t, "storage", "num_sectors"),
            "storage.num_sectors",
        ),
        flash_reserved_top: as_u32(
            get_int(&t, "flash", "reserved_top"),
            "flash.reserved_top",
        ),

        // Optional; "keep" unless board.toml says otherwise (see reg_mode).
        reg0_ldo: reg_mode(&t, "reg0").0,
        reg0_dcdc: reg_mode(&t, "reg0").1,
        reg1_ldo: reg_mode(&t, "reg1").0,
        reg1_dcdc: reg_mode(&t, "reg1").1,
    }
}

// ---------------------------------------------------------------------------
// memory.x cross-check
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------

fn write_board_consts(out: &Path, b: &Board) {
    let mut s = String::new();
    s.push_str("// @generated by build.rs from board.toml - do not edit.\n\n");

    s.push_str("// ---- device identity ----\n");
    s.push_str(&format!("pub const VID: u16 = 0x{:04X};\n", b.vid));
    s.push_str(&format!("pub const PID: u16 = 0x{:04X};\n", b.pid));
    s.push_str(&format!("pub const MANUFACTURER: &str = {:?};\n", b.manufacturer));
    s.push_str(&format!("pub const PRODUCT_NAME: &str = {:?};\n", b.product_name));

    s.push_str("\n// ---- matrix geometry ----\n");
    s.push_str(&format!("pub const ROWS_PER_HALF: usize = {};\n", b.rows_per_half));
    s.push_str(&format!("pub const COLS_PER_HALF: usize = {};\n", b.cols_per_half));
    s.push_str(&format!("pub const HALVES: usize = {};\n", b.halves));

    s.push_str("\n// ---- gazell link ----\n");
    s.push_str("pub const CHANNEL_TABLE: [u8; ");
    s.push_str(&format!("{}] = [", b.channel_table.len()));
    for (i, c) in b.channel_table.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&format!("{c}"));
    }
    s.push_str("];\n");
    s.push_str(&format!("pub const BASE_ADDRESS_0: u32 = 0x{:08X};\n", b.base_address_0));
    s.push_str(&format!("pub const BASE_ADDRESS_1: u32 = 0x{:08X};\n", b.base_address_1));
    s.push_str(&format!("pub const DATARATE: u32 = {};\n", b.datarate));
    s.push_str(&format!("pub const TIMESLOT_PERIOD_US: u32 = {};\n", b.timeslot_period_us));
    s.push_str(&format!("pub const LINK_TIMEOUT_MS: u64 = {};\n", b.link_timeout_ms));
    s.push_str(&format!("pub const POLL_MS: u64 = {};\n", b.poll_ms));

    s.push_str("\n// ---- flash storage ----\n");
    // Types match rmk's StorageConfig fields (usize / u8) so call sites need no casts.
    s.push_str(&format!(
        "pub const STORAGE_START_ADDR: usize = 0x{:X};\n",
        b.storage_start_addr
    ));
    s.push_str(&format!(
        "pub const STORAGE_NUM_SECTORS: u8 = {};\n",
        b.storage_num_sectors
    ));

    s.push_str("\n// ---- power / regulator ----\n");
    // See reg_mode: the bootloader may leave these in a mode the board cannot
    // actually sustain, and RMK's chip defaults are DC/DC on both.
    for (name, ldo, dcdc) in [
        ("REG0", b.reg0_ldo, b.reg0_dcdc),
        ("REG1", b.reg1_ldo, b.reg1_dcdc),
    ] {
        // REG0 (the high-voltage regulator) only exists on nRF52840; nRF52833's
        // PAC has no DCDCEN0 at all, so emitting the constants there would only
        // produce unused-constant warnings in src/main.rs's cfg'd-out branch.
        if name == "REG0" && b.chip != "nrf52840" {
            continue;
        }
        s.push_str(&format!("pub const {name}_FORCE_LDO: bool = {ldo};\n"));
        s.push_str(&format!("pub const {name}_FORCE_DCDC: bool = {dcdc};\n"));
    }

    fs::write(out.join("board_generated.rs"), s).unwrap();
}

/// Emit the Vial definition, deriving the matrix block and the layout grid from
/// the geometry so they cannot disagree with the firmware.
fn write_vial_json(out: &Path, b: &Board) {
    let cols_total = b.cols_per_half * b.halves;

    let mut rows = Vec::new();
    for r in 0..b.rows_per_half {
        let mut keys = Vec::new();
        for c in 0..cols_total {
            // Visual gap between the halves, the way the real board looks.
            if c == b.cols_per_half {
                keys.push("\n            {\n              \"x\": 0.5\n            }".to_string());
            }
            keys.push(format!("\n            \"{r},{c}\""));
        }
        rows.push(format!("          [{} \n          ]", keys.join(",")));
    }

    let json = format!(
        r#"{{
    "name": {name},
    "vendorId": "0x{vid:04X}",
    "productId": "0x{pid:04X}",
    "lighting": "none",
    "matrix": {{
        "rows": {rows},
        "cols": {cols}
    }},
    "layouts": {{
        "keymap": [
{layout}
        ]
    }}
}}
"#,
        name = serde_json_string(&b.vial_name),
        vid = b.vid,
        pid = b.pid,
        rows = b.rows_per_half,
        cols = cols_total,
        layout = rows.join(",\n"),
    );

    fs::write(out.join("vial.json"), json).unwrap();
}

/// Minimal JSON string quoting (the `json` crate's stringify already handles the
/// generated file later; this is only for the value we inject ourselves).
fn serde_json_string(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn generate_vial_config(out: &Path, b: &Board) {
    let vial_json = fs::read_to_string(out.join("vial.json")).unwrap();
    let vial_cfg = json::stringify(json::parse(&vial_json).unwrap());

    let mut compressed: Vec<u8> = Vec::new();
    XzEncoder::new(vial_cfg.as_bytes(), 6)
        .read_to_end(&mut compressed)
        .unwrap();

    let decls = [
        const_declaration!(pub VIAL_KEYBOARD_DEF = compressed),
        const_declaration!(pub VIAL_KEYBOARD_ID = b.vial_keyboard_id),
    ]
    .join("\n");
    fs::write(out.join("config_generated.rs"), decls).unwrap();
}

// ---------------------------------------------------------------------------

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed=board.toml");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");

    let board = load_board();

    // ---- chip <-> layout validation ----------------------------------------
    // (flash size, RAM size) per nRF52 part that has USB. A layout that cannot
    // fit is the classic "flashes fine, never boots" mistake.
    let (chip_flash, chip_ram_base, chip_ram_size) = match board.chip.as_str() {
        "nrf52840" => (1024 * 1024u32, 0x2000_0000u32, 256 * 1024u32),
        "nrf52833" => (512 * 1024, 0x2000_0000, 128 * 1024),
        "nrf52820" => (256 * 1024, 0x2000_0000, 32 * 1024),
        other => panic!(
            "board.toml: [chip] name = {other:?} is not supported. This firmware needs a USB \
             device controller, so only nrf52840, nrf52833 and nrf52820 are possible \
             (nrf52832/811/810 have no USB)."
        ),
    };

    let enabled: Vec<&str> = ["nrf52840", "nrf52833", "nrf52820"]
        .into_iter()
        .filter(|c| env::var(format!("CARGO_FEATURE_{}", c.to_uppercase())).is_ok())
        .collect();
    if enabled.len() != 1 {
        panic!(
            "exactly one chip feature must be enabled, found {enabled:?}. Use \
             `cargo build --features nrf52833 --no-default-features`."
        );
    }
    if enabled[0] != board.chip {
        panic!(
            "chip mismatch: cargo feature `{}` is enabled but board.toml says [chip] name = {:?}. \
             They must agree, or the linker script and the PAC describe different chips.",
            enabled[0], board.chip
        );
    }

    let flash_end = board.flash_origin + board.flash_length;
    if flash_end > chip_flash {
        panic!(
            "board.toml: [memory] flash_origin + flash_length = 0x{:X}..0x{:X} exceeds {} flash \
             (0x{:X} bytes).",
            board.flash_origin, flash_end, board.chip, chip_flash
        );
    }
    let ram_end = board.ram_origin + board.ram_length;
    if board.ram_origin < chip_ram_base || ram_end > chip_ram_base + chip_ram_size {
        panic!(
            "board.toml: [memory] RAM 0x{:X}..0x{:X} is outside {} RAM (0x{:X}..0x{:X}).",
            board.ram_origin,
            ram_end,
            board.chip,
            chip_ram_base,
            chip_ram_base + chip_ram_size
        );
    }

    // ---- storage placement -------------------------------------------------
    // Storage must not overlap live code: this is the one mistake here that
    // bricks the keyboard in a confusing way (keys stop working after the first
    // settings write, with nothing obviously wrong at build time).
    let storage_end = board.storage_start_addr + board.storage_num_sectors * 4096;
    if board.storage_start_addr < flash_end {
        panic!(
            "board.toml: [storage] start_addr = 0x{:X} is inside the application region \
             ([memory] FLASH is 0x{:X}..0x{:X}). Flash storage would overwrite live code.",
            board.storage_start_addr, board.flash_origin, flash_end
        );
    }
    if storage_end > board.flash_reserved_top {
        panic!(
            "board.toml: [storage] region 0x{:X}..0x{:X} runs into [flash] reserved_top = 0x{:X} \
             (the factory USB bootloader). That is the only way to re-flash this dongle without \
             a debug probe, so the build refuses.",
            board.storage_start_addr, storage_end, board.flash_reserved_top
        );
    }

    write_board_consts(&out, &board);
    write_vial_json(&out, &board);
    generate_vial_config(&out, &board);

    // ---- linker wiring (from the working keypoint-rmk-dongle build) ---------
    // memory.x is GENERATED here rather than hand-maintained, so the layout is a
    // board.toml entry like everything else and cannot disagree with the checks
    // above.
    fs::write(
        out.join("memory.x"),
        format!(
            "/* @generated by build.rs from board.toml - do not edit. */\n\
             MEMORY\n{{\n\
             \x20 FLASH : ORIGIN = 0x{:08X}, LENGTH = {}K\n\
             \x20 RAM   : ORIGIN = 0x{:08X}, LENGTH = {}K\n\
             }}\n",
            board.flash_origin,
            board.flash_length / 1024,
            board.ram_origin,
            board.ram_length / 1024
        ),
    )
    .unwrap();
    println!("cargo:rustc-link-search={}", out.display());

    // `--nmagic`: required because our FLASH origin (0x1000) is not 64K-aligned.
    println!("cargo:rustc-link-arg=--nmagic");
    println!("cargo:rustc-link-arg=-Tlink.x");
    println!("cargo:rustc-link-arg=-Tdefmt.x");

    // ---- Nordic Gazell library (closed source, SDK 17.1.1, Cortex-M4 hard-float)
    //
    // Symbol contract read out with arm-none-eabi-nm rather than assumed: it
    // needs from us only memcpy/memset plus the four nrf_gzll_* callbacks (see
    // src/gazell.rs), defines everything else it references, and defines the
    // three ISRs it uses (TIMER2_IRQHandler, RADIO_IRQHandler,
    // SWI0_EGU0_IRQHandler) which src/gazell.rs reaches through three vector
    // table thunks because nrf-pac names those interrupts differently.
    // Where the Gazell archive comes from, and which of its per-chip builds to
    // use. Overridable with GZLL_DIR / GZLL_LIB so a different SDK's build can
    // be tried without touching code - the library version is a real variable
    // here: the original keyboard (nRF51822) was built against SDK 11.0.0's
    // gzll_gcc.a, which is nRF51-only and cannot run on nRF52840, while SDK
    // 12.3 was the first nRF52 port of that same code. This port now links SDK
    // 17.1.1's nRF52840-specific build, whose symbol contract was checked to be
    // identical (same four callbacks required, same three ISRs defined).
    let gzll_lib =
        std::env::var("GZLL_LIB").unwrap_or_else(|_| "gzll_nrf52840_gcc.a".to_string());
    let gzll_dir = {
        // The archive is vendored in this repository, so a clone - on a GitHub
        // runner, or on anybody else's machine - links the same bytes without
        // ever having seen an nRF5 SDK checkout. An absolute GZLL_DIR is taken
        // as-is; a relative one resolves against this crate's manifest
        // directory, which IS the repository root here (there is no sub-crate).
        let raw = std::env::var("GZLL_DIR").unwrap_or_else(|_| "vendor/gzll".to_string());
        let p = Path::new(&raw);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()))
                .join(p)
        };
        if !abs.join(&gzll_lib).is_file() {
            panic!(
                "Gazell archive {gzll_lib} not found in '{}'.\n\
                 This build needs exactly one file from the Nordic SDK, vendored\n\
                 under vendor/gzll together with its licence. If you moved it,\n\
                 point GZLL_DIR at a directory holding that archive (an nRF5 SDK\n\
                 components/proprietary_rf/gzll/gcc works).\n\
                 See vendor/gzll/README.md for provenance and licence terms.",
                abs.display()
            );
        }
        abs.display().to_string()
    };
    println!("cargo:rustc-link-search=native={gzll_dir}");
    // `+verbatim`: the file is `gzll_nrf52840_gcc.a`, not `libgzll_nrf52840_gcc.a`.
    println!("cargo:rustc-link-lib=static:+verbatim={gzll_lib}");
    println!("cargo:rerun-if-changed={gzll_dir}/{gzll_lib}");
    println!("cargo:rerun-if-env-changed=GZLL_DIR");
    println!("cargo:rerun-if-env-changed=GZLL_LIB");

    println!(
        "cargo:warning=board: {} rows x {} cols ({} halves), storage at 0x{:X}+{}K",
        board.rows_per_half,
        board.cols_per_half * board.halves,
        board.halves,
        board.storage_start_addr,
        board.num_sectors_kb()
    );
}

impl Board {
    fn num_sectors_kb(&self) -> u32 {
        self.storage_num_sectors * 4
    }
}
