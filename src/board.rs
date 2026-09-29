//! The board description, as compile-time constants.
//!
//! Everything here comes from `board.toml`, turned into Rust by `build.rs`
//! (`board_generated.rs`). This file adds only the *derived* values and the
//! invariants the rest of the firmware relies on - so a wrong number in
//! `board.toml` fails the build here rather than misbehaving on hardware.
//!
//! Nothing else in the firmware should hardcode geometry: `keymap.rs` sizes its
//! arrays from `ROW`/`COL` (so a keymap that does not match the matrix is a
//! compile error), and `gazell.rs` packs bits using the same `COLS_PER_HALF`.

include!(concat!(env!("OUT_DIR"), "/board_generated.rs"));

// ---------------------------------------------------------------------------
// Derived
// ---------------------------------------------------------------------------

/// Bytes in one half's report: one byte per row.
pub const PAYLOAD_LENGTH: u32 = ROWS_PER_HALF as u32;

/// Logical matrix as RMK sees it. The halves are concatenated along *columns*:
/// left = 0..COLS_PER_HALF, right = COLS_PER_HALF..COL. (The original QMK
/// keyboard definition uses the same 4x12 shape with the split after column 5,
/// so this matches the real hardware.)
pub const ROW: usize = ROWS_PER_HALF;
pub const COL: usize = COLS_PER_HALF * HALVES;

/// Column bits that carry data in one half's row byte. The transmitter writes
/// only these bits, but masking makes that contract explicit: a future
/// transmitter with a wider row cannot leak stray high bits into the column
/// range of the other half.
pub const HALF_MASK: u8 = if COLS_PER_HALF >= 8 {
    0xFF
} else {
    ((1u16 << COLS_PER_HALF) - 1) as u8
};

// ---------------------------------------------------------------------------
// Derived link timing
//
// Gazell's own defaults for the two values below are computed from the
// library's DEFAULT configuration, not from board.toml:
//
//   NRF_GZLL_DEFAULT_SYNC_LIFETIME
//       = 3 * NRF_GZLL_DEFAULT_CHANNEL_TABLE_SIZE(5)
//           * NRF_GZLL_DEFAULT_TIMESLOTS_PER_CHANNEL(2)
//       = 30 timeslots
//   NRF_GZLL_DEFAULT_TIMESLOTS_PER_CHANNEL_WHEN_DEVICE_OUT_OF_SYNC = 15
//
// That reasoning used to continue here, and it was wrong three ways: it called
// this a "4-timeslot table" when the host never calls
// nrf_gzll_set_timeslots_per_channel() and therefore runs on the default of 2;
// it derived "one full channel rotation is 24 timeslots (21.6 ms)" from that 4;
// and it concluded that applying the library's formulas to this configuration
// "gives 72 and 24" - i.e. that sync_lifetime and
// timeslots-per-channel-when-out-of-sync ought to be set. Both were set on that
// basis, and both had to be undone. See below.
//
// The formulas cannot be applied to a board's own configuration at all.
// NRF_GZLL_DEFAULT_SYNC_LIFETIME is a macro over
// NRF_GZLL_DEFAULT_CHANNEL_TABLE_SIZE (5) and
// NRF_GZLL_DEFAULT_TIMESLOTS_PER_CHANNEL (2), resolved when the *library* was
// compiled - so it is 30 timeslots whatever this board's channel table and
// timeslot settings happen to be. Recomputing it "for this configuration"
// produces a number the running library does not use and never did.
//
// The same three errors were made again, independently, in keypoint-nmk on
// 2026-09-17, along with three more of the same family (a channel-selection
// policy, rx pipes_enabled and rx tx_power all "improved" away from the
// defaults, all reverted). That is why the wrong version is described here
// rather than quietly deleted: the reasoning is attractive and it recurs.
// ---------------------------------------------------------------------------

/// Link timing is left entirely at the library defaults, which is what the
/// WORKING receiver runs on.
///
/// redox-w-receiver-basic sets only the channel table, the datarate, the
/// timeslot period and the two base addresses - everything else it leaves at
/// the library default. This port used to "improve" those defaults, deriving
/// them from the library's documented formulas, and every one of those changes
/// had to be undone: the formulas are written in terms of a different table
/// size and timeslot count than the ones actually in use.
///
/// The two settings this affects - sync lifetime (default 30 timeslots) and
/// timeslots-per-channel-when-out-of-sync (default 15) - are therefore never
/// set here at all, which is also why they are not in board.toml:
/// nrf_gzll_set_sync_lifetime() and
/// nrf_gzll_set_timeslots_per_channel_when_device_out_of_sync() are simply
/// never called.

// ---------------------------------------------------------------------------
// Invariants
//
// These duplicate the checks build.rs already makes, deliberately: build.rs
// catches bad input early with a good message, and these make sure the
// *derived* values used here stay sane even if someone edits the generated
// constants or the derivation above.
// ---------------------------------------------------------------------------

const _: () = assert!(
    ROWS_PER_HALF >= 1 && ROWS_PER_HALF <= 32,
    "rows_per_half must be 1..=32 (Gazell payload limit)"
);
const _: () = assert!(
    COLS_PER_HALF >= 1 && COLS_PER_HALF <= 8,
    "cols_per_half must be 1..=8 (one byte per row)"
);
const _: () = assert!(
    HALVES == 2,
    "the Gazell link is two pipes: pipe 0 = left, pipe 1 = right"
);
/// RMK carries row/col as u8 in `KeyboardEvent`.
const _: () = assert!(ROW <= 255 && COL <= 255, "matrix dimensions must fit in u8");
/// The right half is shifted left by COLS_PER_HALF and the result must fit in u16.
const _: () = assert!(COL <= 16, "logical columns must fit in u16 when packing row bits");
