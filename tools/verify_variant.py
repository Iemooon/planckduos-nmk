#!/usr/bin/env python3
"""Verify the three board variants of the Planck Duos receiver, by PRODUCT.

WHY THIS EXISTS
---------------
The three variants differ ONLY in a layout that is generated at build time
(build.rs -> memory.x) and in which cargo chip feature is enabled. Neither is
visible in the source you happen to be looking at, and getting either wrong
produces an image that flashes successfully and then never runs - or, worse,
storage that writes over live code. So the claim "this file is for board X" is
checked against the file itself, not against the intent of the build.

That gap is not hypothetical here: the variants are selected by an environment
variable (BOARD_TOML), and a `set` that did not take effect yields a file with
the right NAME and the wrong layout. Reading the table below from board.toml
would therefore defeat the purpose - the table is deliberately a separate
statement of what each board's partition map is, so a mistake in board.toml
shows up as a failure instead of being quietly agreed with.

WHAT IS CHECKED, per variant:
  * the .hex starts exactly at the application origin that board's bootloader
    jumps to (0x1000 for the two UF2/DFU boards, 0x27000 for blue macro)
  * the application does not run past the end of its declared region
  * the application never reaches into the storage region (0xA0000 etc.)
  * the storage region never reaches into [flash] reserved_top
  * the .uf2 carries the right family ID - a UF2 bootloader rejects a file with
    the wrong family, and nRF52840 and nRF52833 have different ones
  * the .uf2 starts at the same address as the .hex (same image, two formats)
  * images that MUST differ do differ: identical output means an override did
    not take effect. All three layouts here are distinct, so all three images
    must be distinct - which also catches the two nRF52840 boards being built
    from the same [storage] when only one of them was meant to change.

NOT checked, because it cannot be read off the product: whether the Gazell
parameters match the transmitters. That is board.toml's business.

Usage:  python tools\\verify_variant.py        (from the project root, after a build)
"""

import hashlib
import os
import struct
import sys

# name,             origin,     app_len,      reserved_top, storage,   storage_kb, uf2 family
VARIANTS = [
    ("52840-dongle",     0x00001000, 636 * 1024, 0xE0000, 0xA0000,  24, None),
    ("nicenano-52840",   0x00001000, 636 * 1024, 0xF4000, 0xA0000, 128, 0xADA52840),
    ("blue-macro-52833", 0x00027000, 260 * 1024, 0x74000, 0x68000,  48, 0x621E937A),
]

PREFIX = "gazell-dongle-"

UF2_MAGIC0 = 0x0A324655
UF2_MAGIC1 = 0x9E5D5157
UF2_MAGIC_END = 0x0AB16F30
UF2_BLOCK = 512


def hex_span(path):
    """Return (lo, hi, nbytes) of the data records in an Intel HEX file.

    Both extended-address record types are honoured: this project's
    `cargo objcopy` emits type 02 (extended SEGMENT, a paragraph count, so x16)
    and not type 04 (extended LINEAR, x65536). A parser that knows only 04
    reads the whole image back down to 0x0000..0xFFFF and reports nonsense.
    """
    base = 0
    lo = hi = None
    n = 0
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line.startswith(":"):
                continue
            cnt = int(line[1:3], 16)
            addr16 = int(line[3:7], 16)
            rec = int(line[7:9], 16)
            if rec == 2:
                base = int(line[9:13], 16) << 4
            elif rec == 4:
                base = int(line[9:13], 16) << 16
            elif rec == 0:
                a = base + addr16
                lo = a if lo is None else min(lo, a)
                hi = max(hi or 0, a + cnt)
                n += cnt
    return lo, hi, n


def uf2_scan(path):
    """Return (lo, hi, family_id, nblocks) for a UF2 file, or raise."""
    data = open(path, "rb").read()
    if len(data) % UF2_BLOCK:
        raise ValueError("size %d is not a multiple of %d" % (len(data), UF2_BLOCK))
    n = len(data) // UF2_BLOCK
    lo = hi = None
    family = None
    for i in range(n):
        b = data[i * UF2_BLOCK:(i + 1) * UF2_BLOCK]
        m0, m1, _flags, addr, psize, bno, nblocks, fam = struct.unpack("<8I", b[:32])
        if m0 != UF2_MAGIC0 or m1 != UF2_MAGIC1:
            raise ValueError("block %d: bad magic %08x %08x" % (i, m0, m1))
        if struct.unpack("<I", b[508:512])[0] != UF2_MAGIC_END:
            raise ValueError("block %d: bad end magic" % i)
        if bno != i or nblocks != n:
            raise ValueError("block %d: index %d of %d (file holds %d)" % (i, bno, nblocks, n))
        lo = addr if lo is None else min(lo, addr)
        hi = max(hi or 0, addr + psize)
        if family is None:
            family = fam
    return lo, hi, family, n


def sha(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()[:16]


failures = []
checks = 0


def check(ok, label, detail):
    global checks
    checks += 1
    if not ok:
        failures.append("%s: %s" % (label, detail))
    print("  [%s] %s%s" % ("PASS" if ok else "FAIL", label,
                           "" if ok else "  -> " + detail))


for name, origin, app_len, reserved_top, storage, storage_kb, family in VARIANTS:
    hexf = PREFIX + name + ".hex"
    uf2f = PREFIX + name + ".uf2"
    print("\n=== %s ===" % name)

    if not os.path.exists(hexf):
        failures.append("%s: missing %s" % (name, hexf))
        print("  [FAIL] product present  -> %s not found (run tools\\build-variants.cmd)" % hexf)
        continue

    lo, hi, n = hex_span(hexf)
    end_of_app = origin + app_len
    kb = 100.0 * n / app_len

    check(lo == origin, "hex starts at the application origin",
          "got 0x%05X, expected 0x%05X" % (lo, origin))
    check(hi <= end_of_app, "application fits its declared region",
          "ends 0x%05X, region ends 0x%05X (%d bytes past)"
          % (hi, end_of_app, hi - end_of_app))
    check(hi <= storage, "application never reaches storage",
          "app ends 0x%05X, storage starts 0x%05X" % (hi, storage))
    check(storage + storage_kb * 1024 <= reserved_top, "storage never reaches the bootloader",
          "storage ends 0x%05X, reserved_top 0x%05X"
          % (storage + storage_kb * 1024, reserved_top))
    print("       %d bytes = %.0f%% of the %dK application slot, sha256:%s"
          % (n, kb, app_len // 1024, sha(hexf)))

    if family is None:
        if os.path.exists(uf2f):
            failures.append("%s: a .uf2 exists for a board with no UF2 bootloader" % name)
            print("  [FAIL] no uf2 expected  -> %s present; this board is flashed over "
                  "SWD/DFU, a .uf2 here would be silently refused" % uf2f)
        else:
            print("  [skip] .uf2 - this board has no UF2 bootloader")
        continue

    if not os.path.exists(uf2f):
        failures.append("%s: missing %s" % (name, uf2f))
        print("  [FAIL] product present  -> %s not found" % uf2f)
        continue

    ulo, uhi, ufam, nblocks = uf2_scan(uf2f)
    check(ufam == family, "uf2 family ID",
          "got 0x%08X, expected 0x%08X - the bootloader refuses the wrong family"
          % (ufam, family))
    check(ulo == origin, "uf2 starts at the same address as the hex",
          "uf2 0x%05X vs hex 0x%05X" % (ulo, origin))
    # A UF2 carries its data in 256-byte payload blocks, so the tail is padded
    # up to the next block boundary: the end address is the hex's end rounded
    # UP to 256, never less and never further. Anything past one block of
    # padding means the two formats are not the same image.
    padded_hi = (hi + 255) // 256 * 256
    check(uhi == padded_hi, "uf2 covers the same span as the hex (mod block padding)",
          "uf2 ends 0x%05X, hex ends 0x%05X (padded to 0x%05X)"
          % (uhi, hi, padded_hi))
    print("       %d blocks, span 0x%05X..0x%05X" % (nblocks, ulo, uhi))

# ---- cross-variant: all four layouts differ, so all four images must ----
print("\n=== images must differ from each other ===")
digests = {}
for name, _o, _l, _r, _s, _k, _f in VARIANTS:
    hexf = PREFIX + name + ".hex"
    if os.path.exists(hexf):
        digests.setdefault(sha(hexf), []).append(name)
dupes = {d: names for d, names in digests.items() if len(names) > 1}
check(not dupes, "every variant is a distinct image",
      "identical bytes: " + "; ".join("%s == %s" % (a, b)
                                      for d, names in dupes.items()
                                      for a in names for b in names if a < b)
      if dupes else "")
if dupes:
    print("       identical images mean a variant's layout change did not reach the")
    print("       firmware - usually BOARD_TOML pointing at the wrong file, or a build")
    print("       that reused a stale target/ directory.")

print("\n%d checks, %d failures" % (checks, len(failures)))
sys.exit(1 if failures else 0)
