"""Correct `ar` archive reader: list every member with its real name.

The member FILE SET is the fingerprint that identifies which nRF5 SDK release
this Gazell library was cut from - the module layout changed between releases
(softdevice / chb / nrf_timer usage), and the source-path strings baked into
the object files name the SDK tree outright.
"""
import os
import re
import struct
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
PATH = (sys.argv[1] if len(sys.argv) > 1 else
        os.path.normpath(os.path.join(_HERE, "..", "reference", "gzll", "gzll_nrf52_gcc.a")))
data = open(PATH, "rb").read()
assert data[:8] == b"!<arch>\n", "not an ar archive"

pos = 8
raw = []
strtab = b""
while pos + 60 <= len(data):
    name = data[pos:pos + 16].decode("ascii", "replace")
    try:
        size = int(data[pos + 48:pos + 58].decode("ascii").strip())
    except ValueError:
        break
    body_off = pos + 60
    body = data[body_off:body_off + size]
    nm = name.rstrip()
    if nm == "//":
        strtab = body
    elif nm.endswith("/") and nm != "/":
        raw.append((nm[:-1], body))              # short name
    elif nm == "/":
        pass                                      # symbol table, skip
    elif nm.startswith("/") and nm[1:].isdigit():
        off = int(nm[1:])
        end = strtab.find(b"/\n", off)
        real = strtab[off:end if end > 0 else None]
        raw.append((real.decode("ascii", "replace"), body))
    else:
        raw.append((nm, body))
    pos = body_off + size + (size & 1)

print("== %d archive members ==" % len(raw))
for nm, body in raw:
    tag = ""
    if body[:4] == b"\x7fELF":
        machine, = struct.unpack_from("<H", body, 18)
        flags, = struct.unpack_from("<I", body, 40)
        arch = "VFPv3/DSP" if flags & 0x400 else ("hard-float" if flags & 0x400 else "?")
        tag = "ELF e_machine=%d flags=0x%08x" % (machine, flags)
    print("  %-28s %7d  %s" % (nm, len(body), tag))

print("\n== strings naming a source tree / toolchain ==")
pats = [
    rb"/[A-Za-z0-9_\-./]{6,}",          # absolute-ish paths
    rb"[A-Za-z0-9_\-./]{0,20}(?:SDK|sdk|nrf5[12]|S1[1234]|softdevice|gcc-arm|GNU)[A-Za-z0-9_\-./]{0,30}",
]
seen = set()
for pat in pats:
    for m in re.finditer(pat, data):
        s = m.group(0).decode("ascii", "replace")
        if s in seen or len(s) < 6:
            continue
        seen.add(s)
        print("  ", s)

print("\n== GCC producer / ABI attributes ==")
for m in re.finditer(rb"GCC:.{0,60}", data):
    print("  ", m.group(0).decode("ascii", "replace"))
for m in re.finditer(rb"Tag_ABI_VFP_args.{0,24}", data):
    print("  ", m.group(0).decode("ascii", "replace"))
