//! Gazell dongle: 2.4GHz matrix in, USB HID keyboard (with Vial) out.
//!
//! One nRF52840 (PCA10059) doing both jobs:
//!   * left/right halves' 4x6 matrices arrive over Nordic Gazell (2.4GHz,
//!     strictly one-way: transmitters -> this dongle)
//!   * the host sees one USB HID keyboard, with Vial for live keymap editing
//!
//! Architecture, and why:
//!
//!   * **USB is `rmk::usb::UsbTransport`**, not a hand-written stack. It is the
//!     stack already verified to enumerate on this exact dongle
//!     (VID_1313/PID_1208 with keyboard + mouse + Vial interfaces).
//!   * **No BLE at all.** The link is one-way Gazell, so MPSL and
//!     SoftDeviceController are dropped - which also hands RADIO to Gazell and
//!     frees the timeslot machinery. Both consequences are handled below.
//!   * **Input arrives as `KeyboardEvent`s.** RMK's own split driver does exactly
//!     this (`rmk/src/split/driver.rs`: validate row/col, then publish
//!     `KeyboardEvent::key(row + row_offset, col + col_offset, pressed)`), and
//!     the `Keyboard` processor turns those into HID reports. So Gazell plugs in
//!     at the same seam a split link would, via an `InputDevice`.
//!
//! Entered by a jump from the factory bootloader
//! ---------------------------------------------
//! `nrf_bootloader_app_start` sets VTOR and branches - it does NOT reset the
//! chip - so this application inherits peripheral and clock state. Two clock
//! facts follow, and both are handled in `prepare_clocks`:
//!
//! 1. **HFXO must be running before USBD is touched.** The nRF52840 USB
//!    controller needs the 32 MHz crystal (HFINT is good to a few percent, USB
//!    wants +/-0.25%), and enabling USBD while still on HFINT wedges it so that
//!    `Bus::enable` waits forever for `EVENTCAUSE.READY`. With MPSL present this
//!    was implicit (MPSL starts HFXO for the radio); without it we must ask.
//! 2. **The waits must be bounded.** Per the nRF52840 PS, `TASKS_HFCLKSTART` /
//!    `TASKS_LFCLKSTART` have *no effect - including no event* - when the
//!    oscillator already runs. The bootloader uses USB (so HFXO may be up) and
//!    app_timer (so LFCLK is up), and `embassy_nrf::init` waits for both events
//!    unconditionally. Unbounded waits there deadlock; see the comments on
//!    `prepare_clocks`.

#![no_std]
#![no_main]

mod board;
mod gazell;
mod keymap;
mod vial;

use defmt::info;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_nrf::interrupt::InterruptExt;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::{self, Driver};
use embassy_nrf::{bind_interrupts, peripherals};
use panic_probe as _;
use rmk::config::{BehaviorConfig, DeviceConfig, PositionalConfig, RmkConfig, StorageConfig, VialConfig};
use rmk::host::HostService;
use rmk::keyboard::Keyboard;
use rmk::processor::builtin::wpm::WpmProcessor;
use rmk::storage::async_flash_wrapper;
use rmk::usb::UsbTransport;
use rmk::{KeymapData, initialize_keymap_and_storage, run_all};
use vial::{VIAL_KEYBOARD_DEF, VIAL_KEYBOARD_ID};

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
    // Gazell will claim RADIO / TIMER2 / SWI0_EGU0 once the receiver lands; see
    // gazell.rs. Nothing else in this build uses them (no MPSL/SDC).
});

/// Bring CLOCK into a state this application and `embassy_nrf::init` can cope
/// with, with every wait bounded. Must run before anything touches USBD.
fn prepare_clocks() {
    use embassy_nrf::pac::clock::vals::{HfclkstatSrc, Lfclksrc};

    let clock = embassy_nrf::pac::CLOCK;

    // ---- 1. HFXO: required for USB -----------------------------------------
    // Only skip if genuinely already running *from the crystal*: HFCLKSTAT.STATE
    // reads 1 even when the source is HFINT, so the source is what matters.
    let st = clock.hfclkstat().read();
    if !(st.state() && st.src() == HfclkstatSrc::Xtal) {
        clock.events_hfclkstarted().write_value(0);
        clock.tasks_hfclkstart().write_value(1);
        // Bounded: if HFXO were already up this event would never arrive, and
        // an unbounded wait here is a hang before USB is ever reached.
        let mut spins: u32 = 0;
        while clock.events_hfclkstarted().read() == 0 {
            spins += 1;
            if spins > 20_000_000 {
                info!("HFXO did not report started; continuing anyway");
                break;
            }
        }
    }

    // ---- 2. LFCLK: stop it, and WAIT for the stop --------------------------
    // app_timer in the bootloader starts LFCLK and does not stop it. embassy's
    // init then does `events_lfclkstarted = 0; tasks_lfclkstart = 1; while !event`
    // and that task has no effect while LFCLK already runs from the selected
    // source - so the wait never completes. Stopping first makes embassy's start
    // a real 0 -> 1 edge. The stop must be waited out: issuing LFCLKSTART while
    // the stop is still settling is ignored.
    clock.tasks_lfclkstop().write_value(1);
    let mut spins: u32 = 0;
    while clock.lfclkstat().read().state() {
        spins += 1;
        if spins > 20_000_000 {
            info!("LFCLK did not report stopped; continuing anyway");
            break;
        }
    }
    clock.lfclksrc().write(|w| w.set_src(Lfclksrc::Rc));
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    info!("gazell-dongle: starting");

    // Interrupt priorities as in the upstream non-BLE nrf52840 example: USB and
    // CLOCK_POWER above the gpiote/time drivers.
    let mut config = embassy_nrf::config::Config::default();
    config.gpiote_interrupt_priority = embassy_nrf::interrupt::Priority::P3;
    config.time_interrupt_priority = embassy_nrf::interrupt::Priority::P3;
    embassy_nrf::interrupt::USBD.set_priority(embassy_nrf::interrupt::Priority::P2);
    embassy_nrf::interrupt::CLOCK_POWER.set_priority(embassy_nrf::interrupt::Priority::P2);

    // Regulator modes are NOT the chip's reset defaults when a UF2 bootloader
    // hands over: the Adafruit bootloader switches REG1 to DC/DC before jumping
    // into the application, and DC/DC needs the external inductor that not every
    // nice!nano revision populates. A firmware that never touches these
    // registers therefore runs in a mode it never chose - on a board without
    // that inductor the supply can collapse the moment the radio or USB draws
    // current, and the board looks "written successfully, then completely dead".
    //
    // The values come from board.toml's `[power]` section, and the defaults
    // stated in the profiles follow RMK's own chip defaults for nRF52840
    // (`dcdc_reg0 = true`, `dcdc_reg1 = true` in
    // rmk/examples/use_config/nrf52840_ble/keyboard.toml), which is the
    // configuration known to work on these boards. "keep" leaves a register
    // alone, which is what every profile here did before the option existed.
    //
    // UICR.REGOUT0 (REG0's output voltage) is deliberately not touched: RMK warns
    // that changing it requires bootloader >= 0.10.0, and these boards ship 3V3.
    if board::REG1_FORCE_LDO {
        embassy_nrf::pac::POWER.dcdcen().write(|w| w.set_dcdcen(false));
    } else if board::REG1_FORCE_DCDC {
        embassy_nrf::pac::POWER.dcdcen().write(|w| w.set_dcdcen(true));
    }
    // REG0 (the high-voltage regulator) only exists on nRF52840 - nRF52833 has no
    // DCDCEN0 register in its PAC, which is why build.rs emits these two
    // constants for nRF52840 only.
    #[cfg(feature = "nrf52840")]
    if board::REG0_FORCE_LDO {
        // The field inside the PAC's Dcdcen0 register is still named DCDCEN.
        embassy_nrf::pac::POWER.dcdcen0().write(|w| w.set_dcdcen(false));
    } else if board::REG0_FORCE_DCDC {
        embassy_nrf::pac::POWER.dcdcen0().write(|w| w.set_dcdcen(true));
    }

    // Clocks before USB: HFXO on (bounded), LFCLK stopped and settled.
    prepare_clocks();

    let p = embassy_nrf::init(config);
    info!("peripherals up, HFXO requested");

    let driver = Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));

    // Internal flash holds the keymap/Vial storage. The address comes from
    // board.toml, and build.rs refuses to build if it overlaps the application
    // region in memory.x (writing storage over live code is the one mistake here
    // that fails in a confusing way) or reaches into the bootloader.
    let flash = async_flash_wrapper(Nvmc::new(p.NVMC));
    let storage_config = StorageConfig {
        start_addr: board::STORAGE_START_ADDR,
        num_sectors: board::STORAGE_NUM_SECTORS,
        ..Default::default()
    };

    let rmk_config = RmkConfig {
        device_config: DeviceConfig {
            vid: board::VID,
            pid: board::PID,
            manufacturer: board::MANUFACTURER,
            product_name: board::PRODUCT_NAME,
            ..DeviceConfig::default()
        },
        // Vial's unlock policy.
        //
        // `insecure: true` makes HostLock::is_unlocked() unconditionally true,
        // so nothing ever has to be unlocked: keymap edits and the Vial matrix
        // tester both work straight away. (`VialConfig::new()` hardcodes
        // `insecure: false`, which is why the struct is built directly here.)
        //
        // `unlock_keys` is nevertheless a real combination rather than empty.
        // The Vial GUI runs an unlock handshake before physical-presence actions
        // - its "enter bootloader" button above all - and this is the list of
        // positions it shows as "the keys to press". Empty means an
        // unsatisfiable prompt, 0xFF-filled key positions in the reply, and a
        // "No unlock keys provided" warning from the firmware. Because
        // `insecure` already reports the keyboard as unlocked, these keys never
        // actually have to be pressed; they only make the protocol well-formed.
        //
        // Positions are matrix (row, col) with 12 columns: 0-5 left half,
        // 6-11 right half. Below is the left half's top row, first two keys -
        // one half on purpose, so unlocking never needs both transmitters awake.
        vial_config: VialConfig {
            vial_keyboard_id: VIAL_KEYBOARD_ID,
            vial_keyboard_def: VIAL_KEYBOARD_DEF,
            unlock_keys: &[(0, 0), (0, 1)],
            insecure: true,
        },
        ..Default::default()
    };

    let mut keymap_data = KeymapData::new(keymap::get_default_keymap());
    let mut behavior_config = BehaviorConfig::default();
    let per_key_config = PositionalConfig::default();

    let (keymap, mut storage) = initialize_keymap_and_storage(
        &mut keymap_data,
        flash,
        &storage_config,
        &mut behavior_config,
        &per_key_config,
    )
    .await;
    info!("keymap + storage initialised");

    let mut keyboard = Keyboard::new(&keymap);
    let host_service = HostService::new(&keymap, &rmk_config);
    let mut usb_transport = UsbTransport::new(driver, rmk_config.device_config).with_host_service(&host_service);
    let mut wpm_processor = WpmProcessor::new();

    // NOTE: no hardware watchdog. RMK's `Nrf52Watchdog` is gated behind the nRF
    // BLE features (`_nrf_ble`), which this build deliberately does not enable,
    // so there is no runner to hand `p.WDT` to. If a watchdog is wanted here it
    // has to be a small local one; leaving it out is also the safer bring-up
    // choice, since a watchdog reset is indistinguishable from a crash.

    // ---------------------------------------------------------------------
    // Input source: the Gazell 2.4GHz receiver.
    //
    // `init` configures and enables the radio; from then on the Nordic library
    // is interrupt driven (TIMER2/RADIO/SWI0_EGU0) and this device only turns
    // received packets into `KeyboardEvent`s. That is the same seam RMK's own
    // split link publishes at, so everything downstream - keymap, Vial, USB - is
    // the combination already verified to work on this dongle.
    //
    // `gazell::test_matrix` was the bring-up placeholder that pulsed F13; it
    // proved the USB/keymap/report path in isolation and has been removed.
    // ---------------------------------------------------------------------
    let mut matrix = gazell::GazellMatrix::new();
    matrix.init();

    info!("entering run loop");
    run_all!(storage, usb_transport, wpm_processor, keyboard, matrix).await;
}
