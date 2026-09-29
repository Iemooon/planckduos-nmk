# -*- coding: utf-8 -*-
"""Parse a UF2 file and report what a UF2 bootloader would actually do with it.

The one thing that matters for a Nice!Nano / Adafruit-bootloader board is the
per-block target address: a UF2 converted from a 0x1000-origin image must carry
0x1000, or the bootloader writes the application over the wrong region.
"""

import struct
import sys

MAGIC0 = 0x0A324655
MAGIC1 = 0x9E5D5157
MAGIC_END = 0x0AB16F30
BLOCK = 512

path = sys.argv[1] if len(sys.argv) > 1 else None
if path is None:
    sys.exit("usage: python tools\\uf2_inspect.py <file.uf2>")
data = open(path, "rb").read()

if len(data) % BLOCK:
    sys.exit(f"not a UF2: size {len(data)} is not a multiple of {BLOCK}")

n = len(data) // BLOCK
blocks = []
for i in range(n):
    b = data[i * BLOCK:(i + 1) * BLOCK]
    m0, m1, flags, addr, psize, bno, nblocks, fam = struct.unpack("<8I", b[:32])
    if m0 != MAGIC0 or m1 != MAGIC1:
        sys.exit(f"block {i}: bad magic {m0:#x} {m1:#x}")
    if struct.unpack("<I", b[508:512])[0] != MAGIC_END:
        sys.exit(f"block {i}: bad end magic")
    if bno != i:
        sys.exit(f"block {i}: out of order (blockNo={bno})")
    if nblocks != n:
        sys.exit(f"block {i}: numBlocks={nblocks} but file holds {n}")
    if i == 0:
        print(f"flags      : {flags:#x}")
        print(f"familyID   : {fam:#010x}  (nRF52840 = 0xada52840)")
    blocks.append((addr, psize))

lo = min(a for a, _ in blocks)
hi = max(a + s for a, s in blocks)
gaps = [(blocks[i][0] + blocks[i][1], blocks[i + 1][0])
        for i in range(n - 1)
        if blocks[i][0] + blocks[i][1] != blocks[i + 1][0]]

print(f"blocks     : {n}")
print(f"first block: addr {blocks[0][0]:#010x} payload {blocks[0][1]}")
print(f"last block : addr {blocks[-1][0]:#010x} payload {blocks[-1][1]}")
print(f"span       : {lo:#010x} .. {hi:#010x}  ({(hi - lo) / 1024:.1f} KiB)")
print(f"contiguous : {not gaps}" + (f"  gaps={[(hex(a), hex(b)) for a, b in gaps]}" if gaps else ""))

# The regions this image must stay out of, whichever bootloader the board runs.
for name, start in (("Nordic DFU bootloader", 0xE0000), ("Adafruit bootloader", 0xF4000)):
    print(f"vs {name} @ {start:#x}: {'OK, app ends below it' if hi <= start else '*** OVERLAP ***'}")