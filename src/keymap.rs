//! Keymap for the Gazell dongle.
//!
//! Logical matrix is 4 rows x 12 columns:
//!   columns 0..5   = left half  (Gazell pipe 0, channel table {15, 47, 71})
//!   columns 6..11  = right half (Gazell pipe 1, channel table {31, 57, 81})
//!
//! The halves are physically mirrored, so the right half's bit 0 is logical
//! column 6; see `gazell.rs`.
//!
//! ---------------------------------------------------------------------------
//! THIS LAYOUT IS A PLACEHOLDER, chosen to make bring-up verifiable:
//!
//!   * row 0 is F13..F24, i.e. one *distinct, non-typing* keycode per logical
//!     column. That turns "which physical key produced which report" into a
//!     directly observable answer - vital when checking the Gazell bit-to-column
//!     mapping and the mirrored half, since a wrong column shows up as the wrong
//!     F-key rather than as a plausible-looking letter.
//!   * rows 1..3 are a plain QWERTY-ish grid so the board is usable for typing.
//!
//! The real planckduos411 layout should replace this (transcribed from the
//! original QMK keymap) before this is anything but a bring-up build.
//! ---------------------------------------------------------------------------

use rmk::types::action::KeyAction;
use rmk::{a, k};

/// Dimensions come from `board.toml` (via `crate::board`), NOT from this file.
/// The array type below is built from them, so a keymap whose literal does not
/// match the matrix dimensions is a compile error rather than a runtime
/// surprise - which is the main reason to derive them instead of writing `4` and
/// `12` here.
pub(crate) const COL: usize = crate::board::COL;
pub(crate) const ROW: usize = crate::board::ROW;
/// The layer count is a compile-time property of the keymap array below - it is
/// also the number RMK reports to Vial, which is what decides how many layers
/// the editor offers.
pub(crate) const NUM_LAYER: usize = 16;

/// One all-transparent layer, used for the layers that have not been filled in
/// yet. A transparent key falls through to the layer below, so an untouched
/// layer adds capacity without changing any behaviour.
const EMPTY_LAYER: [[KeyAction; COL]; ROW] = [[KeyAction::Transparent; COL]; ROW];

#[rustfmt::skip]
pub const fn get_default_keymap() -> [[[KeyAction; COL]; ROW]; NUM_LAYER] {
    [
        // ==================== Layer 0 ====================
        [
            // Row 0: one distinct F-key per logical column (bring-up probe row).
            //         left half 0..5            | right half 6..11
            [k!(F13), k!(F14), k!(F15), k!(F16), k!(F17), k!(F18),
             k!(F19), k!(F20), k!(F21), k!(F22), k!(F23), k!(F24)],
            [k!(Tab), k!(Q), k!(W), k!(E), k!(R), k!(T),
             k!(Y),   k!(U), k!(I), k!(O), k!(P), k!(Backspace)],
            [k!(Escape), k!(A), k!(S), k!(D), k!(F), k!(G),
             k!(H),      k!(J), k!(K), k!(L), k!(Semicolon), k!(Quote)],
            [k!(LShift), k!(Z), k!(X), k!(C), k!(V), k!(B),
             k!(N),      k!(M), k!(Comma), k!(Dot), k!(Slash), k!(Enter)],
        ],
        // ==================== Layer 1 (placeholder) ====================
        [
            [k!(F1), k!(F2), k!(F3), k!(F4), k!(F5), k!(F6),
             k!(F7), k!(F8), k!(F9), k!(F10), k!(F11), k!(F12)],
            [k!(Kp7), k!(Kp8), k!(Kp9), k!(KpMinus), k!(KpPlus), a!(No),
             a!(No),  a!(No),  a!(No),  a!(No),     a!(No),    a!(No)],
            [k!(Kp4), k!(Kp5), k!(Kp6), k!(KpAsterisk), k!(KpSlash), a!(No),
             k!(Left), k!(Down), k!(Up), k!(Right), a!(No), a!(No)],
            [k!(Kp1), k!(Kp2), k!(Kp3), k!(Kp0), k!(KpDot), a!(No),
             a!(No),  a!(No),  a!(No),  a!(No),  a!(No),    a!(No)],
        ],
        // =========== Layers 2..15: empty, for Vial to fill in ===========
        EMPTY_LAYER,
        EMPTY_LAYER,
        EMPTY_LAYER,
        EMPTY_LAYER,
        EMPTY_LAYER,
        EMPTY_LAYER,
        EMPTY_LAYER,
        EMPTY_LAYER,
        EMPTY_LAYER,
        EMPTY_LAYER,
        EMPTY_LAYER,
        EMPTY_LAYER,
        EMPTY_LAYER,
        EMPTY_LAYER,
    ]
}
