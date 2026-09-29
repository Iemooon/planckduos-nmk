//! Gazell receiver: Nordic's proprietary 2.4GHz link, host role.
//!
//! Each half sends one 4-byte payload per report (4 rows, 6 columns per byte,
//! bit 0 = the leftmost column of that half). This module owns the radio and
//! republishes the result as `KeyboardEvent`s - the same seam RMK's own split
//! driver publishes at - so the keymap, Vial and USB layers downstream are
//! unchanged.
//!
//! The whole design is deliberately the original receiver's
//! (`redox-w-receiver-basic`, nRF51822 + SDK 11.0.0) transcribed onto one
//! nRF52840:
//!
//!   * the host is configured with exactly the same five calls, and NOTHING
//!     else - every parameter left unset is a library default that the working
//!     receiver also ran on. Debugging this port one setting at a time added
//!     twenty-odd variables that all had to be removed again;
//!   * the RX FIFOs are drained the instant a packet arrives (in the callback,
//!     interrupt context), which is what the original achieved by polling its
//!     FIFO every ~10 us in a bare superloop. The FIFO holds only three packets
//!     and a transmitter sends one every 5 ms, so a drain driven by task
//!     scheduling can let it fill - and a full FIFO means no ACK, which makes
//!     the transmitter retransmit instead of queueing new state;
//!   * the received bytes are merged into a matrix (`row = left | right << 6`)
//!     and handed on as key events, with debouncing done by RMK's own debouncer
//!     exactly as RMK's built-in matrix device does it. (In the original, the
//!     receiver handed QMK a raw matrix and QMK did the debouncing.)
//!
//! The one difference forced by putting everything on one chip is
//! `xosc_ctl = MANUAL`: Gazell's default AUTO control powers the 32MHz crystal
//! down between timeslots, which would take USB with it. main.rs starts HFXO
//! instead.
//!
//! Linking the Nordic library
//! --------------------------
//! `gzll_nrf52840_gcc.a` from nRF5 SDK 17.1.1 is linked in via build.rs. The
//! library VERSION matters: SDK 12.3's nRF52 build services the second pipe
//! unevenly (measured: pipe 0 ~185 packets/s, pipe 1 ~92), which showed up as
//! stutter on whichever half was not on pipe 0. SDK 17.1.1's nRF52840 build does
//! not. The symbol contract is the same in both, and was read out with
//! `arm-none-eabi-nm` rather than assumed:
//!
//!   * external references it needs from us - exactly six:
//!       memcpy, memset                              (Rust/compiler_builtins)
//!       nrf_gzll_host_rx_data_ready                 (below)
//!       nrf_gzll_device_tx_success / _tx_failed     (below, no-ops in host mode)
//!       nrf_gzll_disabled                           (below, no-op)
//!   * it defines the three ISRs it needs: TIMER2_IRQHandler, RADIO_IRQHandler,
//!     SWI0_EGU0_IRQHandler. Those are C names; nrf-pac names the interrupts
//!     differently, so the three thunks below bridge them.

use core::sync::atomic::{AtomicU32, Ordering};

use defmt::info;
use rmk::debounce::fast_debouncer::FastDebouncer;
use rmk::debounce::{DebounceState, DebouncerTrait};
use rmk::event::KeyboardEvent;
use rmk::macros::input_device;
use rmk::matrix::KeyState;

use crate::board::{
    BASE_ADDRESS_0, BASE_ADDRESS_1, CHANNEL_TABLE, COLS_PER_HALF, DATARATE, HALF_MASK, HALVES,
    LINK_TIMEOUT_MS, PAYLOAD_LENGTH, POLL_MS, ROWS_PER_HALF, TIMESLOT_PERIOD_US,
};

/// Logical matrix dimensions, used to size the debouncer and the key states.
use crate::board::{COL, ROW};

/// Gazell protocol enumerations - the library's own constants, not board
/// settings:
///   nrf_gzll_mode_t     : DEVICE = 0, HOST = 1, SUSPEND = 2
///   nrf_gzll_xosc_ctl_t : AUTO = 0, MANUAL = 1
const GZLL_MODE_HOST: u32 = 1;
const GZLL_XOSC_CTL_MANUAL: u32 = 1;

/// The left half is always on pipe 0: it is the only pipe that can use
/// `base_address_0`, which is the left half's base address (pipes 1..7 all share
/// `base_address_1`, the right half's). Their address prefix bytes are the
/// library defaults, which is what the transmitters use, so they are left alone.
const PIPE_LEFT: usize = 0;
const PIPE_RIGHT: usize = 1;

/// Slot in `raw` for each half: 0 = left, 1 = right.
const SLOT_LEFT: usize = 0;
const SLOT_RIGHT: usize = 1;

/// The `raw` slot that holds `pipe`'s data.
const fn slot_of(pipe: usize) -> usize {
    if pipe == PIPE_LEFT { SLOT_LEFT } else { SLOT_RIGHT }
}

// ---------------------------------------------------------------------------
// FFI to the Nordic library
// ---------------------------------------------------------------------------

// `nrf_gzll_device_tx_info_t` is 8 bytes, which AAPCS passes via a pointer to a
// caller-made copy - hence `*const u8` rather than a by-value struct. Getting
// this wrong would corrupt the stack, so it is spelled out rather than
// "modelled". (Host mode never calls those two callbacks anyway.)
#[allow(improper_ctypes)]
unsafe extern "C" {
    fn nrf_gzll_init(mode: u32) -> bool;
    fn nrf_gzll_enable() -> bool;
    fn nrf_gzll_set_channel_table(channel_table: *mut u8, size: u32) -> bool;
    fn nrf_gzll_set_datarate(data_rate: u32) -> bool;
    fn nrf_gzll_set_timeslot_period(period_us: u32) -> bool;
    fn nrf_gzll_set_base_address_0(base_address: u32) -> bool;
    fn nrf_gzll_set_base_address_1(base_address: u32) -> bool;
    fn nrf_gzll_set_xosc_ctl(xosc_ctl: u32) -> bool;

    fn nrf_gzll_get_rx_fifo_packet_count(pipe: u32) -> i32;
    fn nrf_gzll_fetch_packet_from_rx_fifo(pipe: u32, payload: *mut u8, length: *mut u32) -> bool;

    // Provided by the archive; called from the thunks below.
    fn TIMER2_IRQHandler();
    fn RADIO_IRQHandler();
    fn SWI0_EGU0_IRQHandler();
}

/// `nrf_gzll_set_channel_table` takes a non-const pointer, so the table has to
/// live in a mutable static. It is only read by the library.
static mut CHANNEL_TABLE_STORAGE: [u8; CHANNEL_TABLE.len()] = CHANNEL_TABLE;

/// Each half's raw row bytes, one per row, packed into a u32 (row 0 in the low
/// byte). Written from the RX callback (interrupt context), read by the task.
/// Valid while ROWS_PER_HALF <= 4.
static RAW: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// Packets received per half since boot, so the task can tell whether a half has
/// said anything since the last time it looked.
static RX_COUNT: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// Total packets seen; used once, for the startup log line.
static RX_PACKETS: AtomicU32 = AtomicU32::new(0);

/// Take everything waiting in `pipe`'s RX FIFO into `RAW`; returns how many
/// packets were taken.
///
/// Called from the RX callback - the earliest possible moment - and again from
/// the task as a safety net. This is the heart of the port: the original
/// receiver polls its FIFO every ~10 us, whereas a drain scheduled by an async
/// task can be delayed past the point where the three-packet FIFO fills.
///
/// # Safety
/// `pipe` must be a valid pipe index.
unsafe fn drain_pipe(pipe: u32) -> u32 {
    let slot = slot_of(pipe as usize);
    let mut taken = 0u32;
    unsafe {
        while nrf_gzll_get_rx_fifo_packet_count(pipe) > 0 {
            let mut payload = [0u8; PAYLOAD_LENGTH as usize];
            let mut length = PAYLOAD_LENGTH;
            if !nrf_gzll_fetch_packet_from_rx_fifo(pipe, payload.as_mut_ptr(), &mut length) {
                break;
            }
            taken += 1;
            if length != PAYLOAD_LENGTH {
                continue;
            }
            let mut packed = 0u32;
            for row in 0..ROWS_PER_HALF {
                packed |= (payload[row] as u32) << (8 * row);
            }
            RAW[slot].store(packed, Ordering::Relaxed);
        }
    }
    if taken > 0 {
        RX_COUNT[slot].fetch_add(taken, Ordering::Relaxed);
    }
    taken
}

// ---------------------------------------------------------------------------
// The callbacks the Nordic library requires
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn nrf_gzll_host_rx_data_ready(_pipe: u32, _info: u32) {
    // Drain BOTH pipes, not just the one that notified.
    //
    // All the FIFOs share one packet pool (NRF_GZLL_CONST_MAX_TOTAL_PACKETS = 6)
    // and Gazell only acknowledges a packet it has room to store, so a pipe
    // whose notification was dropped by the library's finite callback queue
    // would sit on pool packets and starve the other pipe: that device gets no
    // ACK, keeps retransmitting, and its own TX FIFO fills.
    unsafe {
        drain_pipe(PIPE_LEFT as u32);
        drain_pipe(PIPE_RIGHT as u32);
    }
    RX_PACKETS.fetch_add(1, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn nrf_gzll_device_tx_success(_pipe: u32, _info: *const u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn nrf_gzll_device_tx_failed(_pipe: u32, _info: *const u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn nrf_gzll_disabled() {}

// ---------------------------------------------------------------------------
// Vector-table thunks: PAC interrupt name -> SDK ISR name
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn TIMER2() {
    unsafe { TIMER2_IRQHandler() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn RADIO() {
    unsafe { RADIO_IRQHandler() }
}

/// The PAC names this interrupt `EGU0_SWI0`; the SDK calls it
/// `SWI0_EGU0_IRQHandler`. Same vector, two naming conventions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn EGU0_SWI0() {
    unsafe { SWI0_EGU0_IRQHandler() }
}

// ---------------------------------------------------------------------------
// The input device
// ---------------------------------------------------------------------------

#[input_device(publish = KeyboardEvent)]
pub struct GazellMatrix {
    /// Merge of the two halves, one byte per half per row:
    /// `raw[row * HALVES + slot]`.
    raw: [u8; ROWS_PER_HALF * HALVES],
    /// Per-key debounced state, maintained by RMK's own debouncer - one of the two
    /// it ships, both of which are first-class choices for a real matrix (the
    /// macro picks between them from `[matrix] debouncer`, see
    /// rmk-macro/src/codegen/orchestrator.rs:755), so this behaves like a matrix
    /// source rather than like a hand-written approximation of one.
    ///
    /// `FastDebouncer` commits a change on the FIRST pass that sees it and then
    /// holds that key still for DEBOUNCE_THRESHOLD ms. `DefaultDebouncer`, which
    /// this used to run on, would instead make every change wait out the threshold
    /// before believing it - the wrong trade for this input. The stream is already
    /// debounced where it is measured: `main.c.planck` samples at 1 kHz
    /// (config\nrf_drv_config.h:52 `RTC1_CONFIG_FREQUENCY 1000`) and its
    /// `handle_send` puts a state on the air only after that state has held for
    /// `DEBOUNCE` = 5 consecutive ticks, repeating it every 5 ms after that. A
    /// second window here therefore cannot filter anything the transmitter did
    /// not; it can only delay a change that has already been proven stable, and
    /// with the threshold at RMK's default 20 ms every key paid it - 15 ms of
    /// which was duplicating work done on the other end of the link.
    ///
    /// What is still worth keeping is the lockout half of the fast debouncer: a key
    /// whose state changes twice inside the threshold is held until the window
    /// expires, so contradictory reports cannot become two events. That is the one
    /// failure mode a pre-debounced stream cannot rule out, and it costs nothing on
    /// the leading edge. The threshold itself is set to 5 in keyboard.toml.
    ///
    /// Unlike keypoint-nmk, no indivisible matrix read is needed alongside this
    /// change: a 4-row half packs into ONE u32, so `RAW[slot]` is a single atomic
    /// store and a single atomic load. A 6-row half needs two words and does.
    debouncer: FastDebouncer<ROW, COL>,
    key_states: [[KeyState; ROW]; COL],
    /// Last time a packet was seen per half, for the link timeout.
    last_seen_ms: [u64; 2],
    /// Packets seen per half, to notice activity between polls.
    packets: [u32; 2],
    announced: bool,
}

impl GazellMatrix {
    pub fn new() -> Self {
        Self {
            raw: [0; ROWS_PER_HALF * HALVES],
            debouncer: FastDebouncer::new(),
            key_states: [[KeyState::default(); ROW]; COL],
            last_seen_ms: [0; 2],
            packets: [0; 2],
            announced: false,
        }
    }

    /// Configure and enable the radio. Called once, from `main`, before the
    /// executor starts running this device.
    pub fn init(&mut self) {
        unsafe {
            // MANUAL crystal control: USB needs HFXO permanently, so Gazell must
            // not power it down between timeslots.
            let _ = nrf_gzll_set_xosc_ctl(GZLL_XOSC_CTL_MANUAL);

            let ok = nrf_gzll_init(GZLL_MODE_HOST);

            // ---- the original receiver's configuration, and nothing else ----
            // redox-w-receiver-basic sets exactly these and leaves every other
            // parameter at the library default. Note in particular that it never
            // calls nrf_gzll_set_timeslots_per_channel(), nrf_gzll_set_tx_power()
            // or nrf_gzll_set_rx_pipes_enabled() - so neither does this.
            let ch_ok = nrf_gzll_set_channel_table(
                core::ptr::addr_of_mut!(CHANNEL_TABLE_STORAGE) as *mut u8,
                CHANNEL_TABLE.len() as u32,
            );
            nrf_gzll_set_datarate(DATARATE);
            nrf_gzll_set_timeslot_period(TIMESLOT_PERIOD_US);
            nrf_gzll_set_base_address_0(BASE_ADDRESS_0);
            nrf_gzll_set_base_address_1(BASE_ADDRESS_1);

            let enabled = nrf_gzll_enable();

            info!("gzll: init={} channel_table={} enabled={}", ok, ch_ok, enabled);
        }
    }

    /// Copy what the RX callback has collected into `raw` and refresh the link
    /// timeout bookkeeping.
    fn pump(&mut self) {
        let now = embassy_time::Instant::now().as_millis();

        for pipe in [PIPE_LEFT, PIPE_RIGHT] {
            let slot = slot_of(pipe);
            // The callback normally drains first, in interrupt context; this is
            // the safety net for a notification that did not survive the
            // library's finite callback queue.
            unsafe {
                drain_pipe(pipe as u32);
            }
            let count = RX_COUNT[slot].load(Ordering::Relaxed);
            if count != self.packets[slot] {
                self.packets[slot] = count;
                self.last_seen_ms[slot] = now;
            }
            let packed = RAW[slot].load(Ordering::Relaxed);
            for row in 0..ROWS_PER_HALF {
                self.raw[row * HALVES + slot] = ((packed >> (8 * row)) & 0xFF) as u8;
            }
        }

        if !self.announced {
            self.announced = true;
            info!("gzll: first packets received ({} total)", RX_PACKETS.load(Ordering::Relaxed));
        }
    }

    /// Build the logical row from the two halves.
    ///
    /// Bit-exact with the original receiver and with the QMK code it fed:
    ///   `row = left | (right << 6)`
    /// The halves are physically mirrored, so the right half's bit 0 is logical
    /// column 6 - which is exactly what shifting by COLS_PER_HALF produces.
    fn build_row(&self, row: usize) -> u16 {
        let left = (self.raw[row * HALVES + SLOT_LEFT] & HALF_MASK) as u16;
        let right = (self.raw[row * HALVES + SLOT_RIGHT] & HALF_MASK) as u16;
        left | (right << COLS_PER_HALF)
    }

    async fn read_keyboard_event(&mut self) -> KeyboardEvent {
        loop {
            self.pump();

            // Link timeout: a half that has gone quiet releases its keys, so a
            // lost final packet cannot leave a key stuck. The transmitters power
            // themselves off after 0.5 s without a key, so this only ever fires
            // on a half that is genuinely gone.
            let now = embassy_time::Instant::now().as_millis();
            for slot in 0..HALVES {
                if self.last_seen_ms[slot] != 0
                    && now.saturating_sub(self.last_seen_ms[slot]) > LINK_TIMEOUT_MS
                {
                    self.last_seen_ms[slot] = 0;
                    for row in 0..ROWS_PER_HALF {
                        self.raw[row * HALVES + slot] = 0;
                    }
                }
            }

            // Produce events exactly the way RMK's own matrix device does: keep
            // per-key state and let RMK's debouncer decide when a change is
            // real, one event per pass. That debouncer is the fast one, so the
            // pass that first sees a change is the pass that publishes it - see
            // the field above for why the leading edge is committed here.
            for row in 0..ROW {
                let value = self.build_row(row);
                for col in 0..COL {
                    let pressed = value & (1 << col) != 0;
                    if let DebounceState::Debounced = self.debouncer.detect_change_with_debounce(
                        row,
                        col,
                        pressed,
                        &self.key_states[col][row],
                    ) {
                        self.key_states[col][row].toggle_pressed();
                        return KeyboardEvent::key(
                            row as u8,
                            col as u8,
                            self.key_states[col][row].pressed,
                        );
                    }
                }
            }

            embassy_time::Timer::after_millis(POLL_MS).await;
        }
    }
}
