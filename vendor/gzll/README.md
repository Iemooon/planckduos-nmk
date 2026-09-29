# Vendored Nordic Gazell archive

This firmware links one precompiled static library: Nordic Semiconductor's Gazell
2.4 GHz host stack. Nothing else from the nRF5 SDK is needed - every symbol the
Rust side calls is declared by hand in `src/gazell.rs`, so no headers are required
to build.

| file | chip | bytes | md5 |
|---|---|---|---|
| `gzll_nrf52840_gcc.a` | nRF52840 | 7082428 | f38ef14a6c56a2516a8d4bf773c9d25e |
| `license.txt`         | Nordic's notice, must travel with the binary | 1956 | d2ca93a244fde7c208164989b0e28408 |

Provenance: nRF5 SDK **17.1.1**, `components/proprietary_rf/gzll/gcc/`, unmodified.
Taken from that directory rather than rebuilt, because the archive is closed-source.

## Why this one file serves all four boards

The nRF52833 variants link the same archive. Both chips are Cortex-M4F with the
same peripheral memory map, and the build has always done this: `build.rs` takes
`gzll_nrf52840_gcc.a` regardless of the cargo chip feature, so the nRF52833
images that are flashed and in use are built against exactly this file.

The SDK also ships a chip-generic `gzll_nrf52_gcc.a`; it is not vendored here
because nothing in this project refers to it. `GZLL_LIB` selects a different
archive if you ever want to compare them.

## Why 17.1.1 and not something older

The version of this library was the last standing variable in a link-quality
investigation. SDK 12.3's nRF52 build services the **second** Gazell pipe unevenly -
measured 150-299 packets/s on pipe 0 against 50-149 on pipe 1 - which appears as
stutter on whichever half is not on pipe 0. It is not a configuration problem: the
channel table, timeslot counts, transmit power, sync lifetime, packet pool, fetch
timing, debouncing and the event pipeline were all cleared first, and none of it
mattered because the library itself was the constant. SDK 17.1.1's nRF52840-specific
build does not have the fault.

Before swapping libraries, check the symbol contract with `arm-none-eabi-nm`: the
archive needs exactly `memcpy`, `memset`, `nrf_gzll_host_rx_data_ready`,
`nrf_gzll_device_tx_success`, `nrf_gzll_device_tx_failed` and `nrf_gzll_disabled`
from us, and defines `TIMER2_IRQHandler` / `RADIO_IRQHandler` /
`SWI0_EGU0_IRQHandler` itself. Both SDK builds happen to agree - which is why the
fault was invisible to any linker-level check.

## Licence terms that apply to this directory

`license.txt` (BSD-3-style, Nordic Semiconductor) permits redistribution in binary
form provided the copyright notice, the conditions list and the disclaimer are
reproduced with the distribution - hence this file and `license.txt` sit next to the
archive. Two further conditions bind users of this repository:

  * the software may only be used with a Nordic Semiconductor integrated circuit;
  * the binaries may not be reverse engineered, decompiled, modified or disassembled.

## Overriding this copy

`build.rs` looks at `GZLL_DIR` and `GZLL_LIB` first, so a different SDK build can be
tried without editing code or this directory. The default resolves to this folder
relative to the repository root, and a missing archive fails the build with a
message rather than falling back to a path on somebody else's machine.
