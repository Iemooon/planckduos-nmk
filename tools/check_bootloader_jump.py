# -*- coding: utf-8 -*-
"""Prove the bootloader-jump code is in one image and not the other.

`jump_to_bootloader()`'s nRF path writes GPREGRET (POWER base 0x40000000 +
0x51C) and then resets. If the feature is wired up correctly, that register
address must appear in the image built with it and be absent from the one built
without - a check on the artifact rather than on intent.
"""

import re
import struct
import sys

GPREGRET = 0x4000051C
needles = {
    "GPREGRET addr 0x4000051C (little endian)": struct.pack("<I", GPREGRET),
    "GPREGRET addr via halfwords (1C 05 / 00 40)": bytes([0x1C, 0x05, 0x00, 0x40]),
}

for path in sys.argv[1:]:
    data = open(path, "rb").read()
    print(f"--- {path}  ({len(data)} bytes)")
    for what, needle in needles.items():
        n = data.count(needle)
        print(f"    {what}: {n} occurrence(s)")
    # Any 0x57 "magic" is meaningless on its own (it is a common byte), so it is
    # only reported as context, never as proof.
    print(f"    byte 0x57 count (context only): {data.count(bytes([0x57]))}")