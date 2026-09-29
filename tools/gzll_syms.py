"""Per-member symbol audit of the vendored Gazell archive.

Reads each ELF member's .symtab directly (no external toolchain needed) and
reports, for every member, which symbols it DEFINES and which it leaves
UNDEFINED. The undefined set is the exact contract the application must
satisfy - if a chip-specific symbol (nrf_delay_us, SystemCoreClock,
NRF_CLOCK-dependent helpers) shows up there, the library's timing behaviour
is only as correct as our implementation of it.
"""
import os
import struct
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
PATH = (sys.argv[1] if len(sys.argv) > 1 else
        os.path.normpath(os.path.join(_HERE, "..", "reference", "gzll", "gzll_nrf52_gcc.a")))
data = open(PATH, "rb").read()

# ---- ar walk (same corrected logic as gzll_provenance) ----------------
pos, raw, strtab = 8, [], b""
while pos + 60 <= len(data):
    name = data[pos:pos + 16].decode("ascii", "replace").rstrip()
    try:
        size = int(data[pos + 48:pos + 58].decode("ascii").strip())
    except ValueError:
        break
    body = data[pos + 60:pos + 60 + size]
    if name == "//":
        strtab = body
    elif name == "/":
        pass
    elif name.startswith("/") and name[1:].isdigit():
        off = int(name[1:])
        end = strtab.find(b"/\n", off)
        raw.append((strtab[off:end].decode("ascii", "replace"), body))
    elif name.endswith("/"):
        raw.append((name[:-1], body))
    pos += 60 + size + (size & 1)

STT_FUNC, STT_NOTYPE, STT_OBJECT = 2, 0, 1
SHN_UNDEF, SHN_ABS = 0, 0xFFF1


def symtab_of(body):
    """Yield (name, st_info, st_shndx) for every entry in an ELF32 symtab."""
    if body[:4] != b"\x7fELF":
        return
    sh_off, = struct.unpack_from("<I", body, 0x20)
    sh_entsize, sh_num, sh_strndx = struct.unpack_from("<HHH", body, 0x2E)
    sections = []
    for i in range(sh_num):
        s = sh_off + i * sh_entsize
        _, stype, _, soff, ssize = struct.unpack_from("<IIIII", body, s + 4)[:5]
        _, saddr, _, sent, _, _ = struct.unpack_from("<QQIIQQ", body, s + 12) if False else (0,)*6
        sections.append((stype, soff, ssize, s + 4))
    # re-read properly: Elf32_Shdr = name(4) type(4) flags(4) addr(4) offset(4)
    # size(4) link(4) info(4) addralign(4) entsize(4)
    sections = []
    for i in range(sh_num):
        s = sh_off + i * sh_entsize
        fields = struct.unpack_from("<10I", body, s)
        sections.append(fields)                       # fields[1]=type,[4]=off,[5]=size,[6]=link,[9]=entsize
    symsec = next((f for f in sections if f[1] == 2), None)   # SHT_SYMTAB
    if not symsec:
        return
    _, _, _, _, off, size, link, _, _, entsize = symsec
    strhdr = sections[link]
    strtab_off, strtab_size = strhdr[4], strhdr[5]
    count = size // entsize
    for i in range(count):
        e = off + i * entsize
        nm_off, _val, _sz, info, _other, shndx = struct.unpack_from("<IIIBBH", body, e)
        if nm_off:
            end = body.index(b"\x00", strtab_off + nm_off)
            nm = body[strtab_off + nm_off:end].decode("ascii", "replace")
        else:
            nm = "?"
        yield nm, info, shndx


print("== per-member defined / undefined symbol audit ==\n")
all_undef = {}
all_def = {}
for nm, body in raw:
    if body[:4] != b"\x7fELF":
        continue
    defined, undef = [], []
    for sname, info, shndx in symtab_of(body):
        bind = info >> 4
        if not sname or sname == "?":
            continue
        if bind not in (1, 2):        # GLOBAL or WEAK
            continue
        if shndx == SHN_UNDEF:
            undef.append(sname)
            all_undef.setdefault(sname, []).append(nm)
        elif shndx != SHN_ABS:
            defined.append(sname)
            all_def.setdefault(sname, []).append(nm)
    print("%-28s defines=%-3d undef=%-3d" % (nm, len(defined), len(undef)))
    for u in undef:
        print("      U %s" % u)

print("\n== the application contract (undefined, defined NOWHERE in the archive) ==")
for u, who in sorted(all_undef.items()):
    if u not in all_def:
        print("  %-40s needed by %s" % (u, ", ".join(sorted(set(who)))))

print("\n== weak/abs symbols worth noting ==")
for u in sorted(all_def):
    if u.startswith(("system_", "__aeabi", "nrf_delay", "SystemCore")):
        print("  ", u)
