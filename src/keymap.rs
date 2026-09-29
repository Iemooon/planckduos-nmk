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
//! WHAT THIS TABLE IS - and what it is NOT
//! ---------------------------------------------------------------------------
//! This is a usable base layout: QWERTY on layer 0, digits and punctuation on
//! layer 1, function keys and volume on layer 2. It used to be a bring-up probe
//! whose whole top row was F13..F24 (one distinct non-typing keycode per logical
//! column, so that a wrong column showed up as the wrong F-key instead of a
//! plausible-looking letter). That job is done - the Gazell bit-to-column mapping
//! and the mirrored half were verified over the air - so the probe row is gone.
//! Keep the trick in mind for the next board: it is the cheapest way to see the
//! matrix through the link.
//!
//! It is NOT a transcription of the original firmware, and it cannot be: the
//! QMK/Vial definition this board ships with
//! (`vial-qmk/keyboards/planckduos411/keymaps/vial/keymap.c`) has all sixteen
//! layers filled with `KC_NO`. That is normal for a Vial keyboard - the layout
//! lives in flash, written by the editor, so the compiled-in default is
//! deliberately empty. So this table is ours: conventional Planck geometry (two
//! 6-column halves, two spaces in the middle, layer keys under the pinkies),
//! chosen so a blank EEPROM boots into something you can type on.
//!
//! ---------------------------------------------------------------------------
//! ONE THING THAT WILL BITE YOU
//! ---------------------------------------------------------------------------
//! **RMK only writes the compiled-in default when storage is empty** (rmk
//! `src/lib.rs` hands `data.encoder_map`/keymap to Storage, and
//! `storage/mod.rs` fills defaults only if the region is blank). Once the board
//! has been used with Vial, it reads back the layout FROM FLASH and this table is
//! ignored entirely - editing it and re-flashing changes nothing you can observe.
//! To make this (or any future) default take effect on a board that has been
//! edited before, either clear Vial's storage or set `StorageConfig.clear_layout`
//! once. Not a bug in the keymap; a consequence of the layout being data, not code.
//! ---------------------------------------------------------------------------

use rmk::types::action::KeyAction;
use rmk::{a, k, mo};

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
        // ==================== Layer 0: QWERTY ====================
        // Two 1u spaces sit in the middle (col 5 and col 6), which is where the
        // halves meet: a wide space bar without needing a 2u key, and every cell
        // stays one physical switch - the matrix has no way to express a key that
        // spans columns anyway.
        [
            [k!(Tab),   k!(Q), k!(W), k!(E), k!(R), k!(T),
             k!(Y),     k!(U), k!(I), k!(O), k!(P), k!(Backspace)],
            [k!(Escape), k!(A), k!(S), k!(D), k!(F), k!(G),
             k!(H),      k!(J), k!(K), k!(L), k!(Semicolon), k!(Quote)],
            [k!(LShift), k!(Z), k!(X), k!(C), k!(V), k!(B),
             k!(N),      k!(M), k!(Comma), k!(Dot), k!(Slash), k!(Return)],
            [k!(LCtrl), k!(LGui), k!(LAlt), mo!(1), mo!(2), k!(Space),
             k!(Space), mo!(1), k!(Left), k!(Down), k!(Up), k!(Right)],
        ],
        // ==================== Layer 1: digits and punctuation ====================
        // `Transparent` where layer 0 is already the right answer (letters,
        // modifiers, spaces): an inherited cell is smaller and more honest than a
        // duplicate, and it keeps the halves aligned with the base layer.
        [
            [k!(Grave), k!(Kc1), k!(Kc2), k!(Kc3), k!(Kc4), k!(Kc5),
             k!(Kc6),   k!(Kc7), k!(Kc8), k!(Kc9), k!(Kc0), k!(Minus)],
            [k!(Delete), k!(Equal), k!(LeftBracket), k!(RightBracket), k!(Backslash), a!(Transparent),
             a!(Transparent), k!(Home), k!(PageUp), k!(PageDown), k!(End), a!(Transparent)],
            [a!(Transparent), a!(Transparent), a!(Transparent), a!(Transparent), a!(Transparent), a!(Transparent),
             a!(Transparent), k!(Left), k!(Down), k!(Up), k!(Right), k!(Return)],
            [a!(Transparent), a!(Transparent), a!(Transparent), mo!(1), mo!(2), a!(Transparent),
             a!(Transparent), mo!(1), a!(Transparent), a!(Transparent), a!(Transparent), a!(Transparent)],
        ],
        // ==================== Layer 2: function keys and volume ====================
        // The F-row is the reason this layer exists; volume is here because the
        // board has no dedicated media keys and the top row is otherwise unused.
        // The volume codes are Consumer-page usages, not HID keyboard ones, hence
        // the Kb prefix (rmk `types/src/keycode/hid.rs`).
        [
            [k!(F1), k!(F2), k!(F3), k!(F4), k!(F5), k!(F6),
             k!(F7), k!(F8), k!(F9), k!(F10), k!(F11), k!(F12)],
            [a!(Transparent), k!(KbMute), k!(KbVolumeDown), k!(KbVolumeUp), a!(Transparent), a!(Transparent),
             a!(Transparent), a!(Transparent), a!(Transparent), a!(Transparent), a!(Transparent), a!(Transparent)],
            [a!(Transparent), a!(Transparent), a!(Transparent), a!(Transparent), a!(Transparent), a!(Transparent),
             a!(Transparent), a!(Transparent), a!(Transparent), a!(Transparent), a!(Transparent), k!(Escape)],
            [a!(Transparent), a!(Transparent), a!(Transparent), mo!(1), mo!(2), a!(Transparent),
             a!(Transparent), mo!(2), a!(Transparent), a!(Transparent), a!(Transparent), a!(Transparent)],
        ],
        // =========== Layers 3..15: empty, for Vial to fill in ===========
        // Transparent, not No: an untouched layer that inherits everything is what
        // an empty page in the editor should mean. (keypoint-nmk deliberately writes
        // `No` in its reserved layers because that firmware binds those cells to
        // pointer features - a different reason, so do not "unify" the two.)
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
