"""Inspect the Intel HEX that actually gets flashed.

`check_image.py` validates the ELF. But the bootloader never sees the ELF - it
sees the .hex (and, for the DFU package, the .bin). So check the bytes that are
really going onto the chip:

  * lowest / highest address in the hex
  * the first two words at the lowest address (initial SP and reset vector)
  * whether the .bin produced by objcopy matches the hex byte-for-byte

Usage: python check_hex.py image.hex [image.bin]
"""

import struct
import sys


def parse_hex(path):
    mem = {}
    upper = 0
    for lineno, line in enumerate(open(path), 1):
        line = line.strip()
        if not line:
            continue
        if not line.startswith(":"):
            raise SystemExit(f"{path}:{lineno}: not an Intel HEX record")
        raw = bytes.fromhex(line[1:])
        count, addr, rectype = raw[0], (raw[1] << 8) | raw[2], raw[3]
        payload = raw[4:4 + count]
        if rectype == 0x00:
            for i, b in enumerate(payload):
                mem[upper + addr + i] = b
        elif rectype == 0x02:
            upper = ((payload[0] << 8) | payload[1]) << 4
        elif rectype == 0x04:
            upper = ((payload[0] << 8) | payload[1]) << 16
        elif rectype == 0x01:
            break
        elif rectype == 0x05:
            pass  # start linear address
        else:
            print(f"  note: record type 0x{rectype:02X} at line {lineno}")
    return mem


def main():
    hexpath = sys.argv[1]
    mem = parse_hex(hexpath)
    lo, hi = min(mem), max(mem)
    print(f"hex             : {hexpath}")
    print(f"address range   : 0x{lo:08X} .. 0x{hi:08X}  ({len(mem)} bytes)")
    print(f"contiguous      : {len(mem) == hi - lo + 1}")

    sp, reset = struct.unpack("<II", bytes(mem[lo + i] for i in range(8)))
    print(f"initial SP      : 0x{sp:08X}")
    print(f"reset vector    : 0x{reset:08X}")

    ok = True
    if not (0x20000000 <= sp <= 0x20040000):
        print("!! initial SP is not in RAM")
        ok = False
    if not (0 <= reset < 0x100000) or reset & 1 == 0:
        print("!! reset vector is not a valid Thumb address in FLASH")
        ok = False
    if lo != 0x1000:
        print(f"!! hex does NOT start at 0x1000 (starts at 0x{lo:X}) - "
              "the dongle bootloader expects the application at 0x1000")
        ok = False

    if len(sys.argv) > 2:
        binpath = sys.argv[2]
        data = open(binpath, "rb").read()
        hexbytes = bytes(mem.get(lo + i, 0xFF) for i in range(len(data)))
        print(f"bin             : {binpath} ({len(data)} bytes)")
        if hexbytes == data:
            print("hex/bin content : IDENTICAL")
        else:
            print("hex/bin content : *** DIFFERENT ***")
            for i, (a, b) in enumerate(zip(hexbytes, data)):
                if a != b:
                    print(f"   first difference at offset {i} (address 0x{lo+i:06X}): hex {a:02X} vs bin {b:02X}")
                    break
            ok = False

    print("RESULT          : " + ("consistent" if ok else "PROBLEM"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
