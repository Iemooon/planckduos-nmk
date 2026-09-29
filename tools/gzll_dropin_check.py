"""Verify the drop-in fitness of a replacement Gazell archive.

Two failure modes matter before swapping a vendored .a:
  1. An API our FFI block declares that the new archive does not DEFINE
     -> link error, or worse, a silent rename.
  2. The three ISRs we thunk (TIMER2 / RADIO / SWI0_EGU0). If the new library
     no longer defines them, or defines different ones, the radio never runs.
Also prints the SDK's own Gazell version macros from nrf_gzll.h so the release
is named rather than inferred.
"""
import os
import re
import sys

# The archive to check: argument, then GZLL_ARCHIVE, then the vendored copy this
# repository actually links.
_DEFAULT_ARCHIVE = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "vendor", "gzll", "gzll_nrf52840_gcc.a")
ARCHIVE = (sys.argv[1] if len(sys.argv) > 1
           else os.environ.get("GZLL_ARCHIVE", _DEFAULT_ARCHIVE))
# nrf_gzll.h is NOT vendored - the firmware declares its own FFI, so no header is
# needed to build - which means the SDK header is only available if you point at
# one. Without it the version-macro section says so and the symbol checks still
# run, because those read the archive itself.
HEADER = (sys.argv[2] if len(sys.argv) > 2
          else os.environ.get("GZLL_HEADER", ""))

blob = open(ARCHIVE, "rb").read()
defined = {m.group(0).decode() for m in re.finditer(rb"nrf_[a-z0-9_]{3,}", blob)}

# Every symbol our FFI block asks the archive to provide.
NEEDED_API = [
    "nrf_gzll_init", "nrf_gzll_enable", "nrf_gzll_disable",
    "nrf_gzll_set_channel_table", "nrf_gzll_set_datarate",
    "nrf_gzll_set_timeslot_period", "nrf_gzll_set_timeslots_per_channel",
    "nrf_gzll_set_max_tx_attempts", "nrf_gzll_set_base_address_0",
    "nrf_gzll_set_base_address_1", "nrf_gzll_set_xosc_ctl",
    "nrf_gzll_set_sync_lifetime",
    "nrf_gzll_set_timeslots_per_channel_when_device_out_of_sync",
    "nrf_gzll_set_tx_power", "nrf_gzll_set_address_prefix_byte",
    "nrf_gzll_get_address_prefix_byte", "nrf_gzll_set_rx_pipes_enabled",
    "nrf_gzll_flush_rx_fifo", "nrf_gzll_get_rx_fifo_packet_count",
    "nrf_gzll_fetch_packet_from_rx_fifo",
]
NEEDED_ISR = ["TIMER2_IRQHandler", "RADIO_IRQHandler", "SWI0_EGU0_IRQHandler"]

print("== archive: %s ==" % ARCHIVE)
print("\n-- API our FFI declares --")
missing = 0
for fn in NEEDED_API:
    hit = fn in defined
    missing += 0 if hit else 1
    print("  %-58s %s" % (fn, "OK" if hit else "*** ABSENT ***"))

print("\n-- ISRs we thunk from the vector table --")
for isr in NEEDED_ISR:
    hit = bool(re.search((r"\b%s\b" % isr).encode(), blob))
    missing += 0 if hit else 1
    print("  %-58s %s" % (isr, "OK" if hit else "*** ABSENT ***"))

print("\n-- every ISR the new archive defines --")
isrs = sorted({m.group(0).decode() for m in re.finditer(rb"\b[A-Z][A-Z0-9_]{2,}_IRQHandler\b", blob)})
print("  " + ", ".join(isrs))

print("\n-- Gazell version macros in the SDK header --")
if not HEADER:
    print("   skipped: no nrf_gzll.h given (2nd argument or GZLL_HEADER). The header"
          " is not vendored because the firmware declares its own FFI; pass one from"
          " an nRF5 SDK only if you want the version macros named.")
else:
    try:
        hdr = open(HEADER, "r", encoding="utf-8", errors="replace").read()
        found = list(re.finditer(r"#\s*define\s+\w*(?:VERSION|GZLL_VER)\w*\s+.*", hdr))
        for m in found:
            print("  ", m.group(0).strip())
        if not found:
            print("   no VERSION-style macros matched in", HEADER)
    except OSError as e:
        print("   header unreadable:", e)

print("\nRESULT:", "all symbols present" if missing == 0 else "%d MISSING" % missing)
