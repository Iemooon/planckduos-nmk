"""Report a flash span from an Intel HEX file, and (optionally) compare two
Gazell archives so the library swap is documented with numbers, not memory.
"""
import os
import re
import sys


def hex_span(path):
    base = 0
    lo = hi = None
    n = 0
    for line in open(path):
        line = line.strip()
        if not line.startswith(":"):
            continue
        cnt = int(line[1:3], 16)
        addr16 = int(line[3:7], 16)
        rec = int(line[7:9], 16)
        if rec == 2:
            # extended SEGMENT address: the field is a paragraph count (x16).
            # llvm-objcopy emits THIS record type, not type 04 - ignoring it
            # folds the high half of the image back down to 0x0000..0xFFFF and
            # makes the reported span nonsense.
            base = int(line[9:13], 16) << 4
        elif rec == 4:
            base = int(line[9:13], 16) << 16
        elif rec == 0:
            a = base + addr16
            lo = a if lo is None else min(lo, a)
            hi = max(hi or 0, a + cnt)
            n += cnt
    return lo, hi, n


# Images before `--` are the ones to report a span for; anything after it is an
# archive to fingerprint. Without this split, a library path would be reported as
# a "(missing)" hex file.
images = sys.argv[1:sys.argv.index("--")] if "--" in sys.argv else sys.argv[1:]
for p in images:
    if os.path.exists(p):
        lo, hi, n = hex_span(p)
        # No percentage here on purpose: the application slot is per-board
        # (636K / 448K / 260K), and a single hardcoded denominator reports a
        # wrong-sounding number for three of the four variants. tools\
        # verify_variant.py owns the partition table and does the arithmetic
        # against the right region.
        print("%-26s flash 0x%05X..0x%05X  = %6d bytes"
              % (os.path.basename(p), lo, hi, n))
    else:
        print("%-26s (missing)" % p)

print()
# The archive this build links is vendored in the repository, so the default
# fingerprint is about the file that actually ends up in the firmware. Any other
# archive - an older SDK's build, a candidate replacement - can be named after a
# `--` to compare it against the vendored one, which is how the library-version
# conclusion in vendor/gzll/README.md was originally reached.
_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
VENDORED = os.path.join(_ROOT, "vendor", "gzll", "gzll_nrf52840_gcc.a")
extra = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
pair = [("vendored (what the build links)", VENDORED)]
pair += [(os.path.basename(p), p) for p in extra]
print("== archive fingerprints ==")
for label, p in pair:
    if not os.path.exists(p):
        print("  %-24s (missing) %s" % (label, p))
        continue
    blob = open(p, "rb").read()
    gcc = re.search(rb"GCC: [^\x00]{0,70}", blob)
    members = re.findall(rb"nrf_[a-z_]+\.(?:c\.)?o/", blob)
    print("  %-24s %8d bytes  members=%d  %s"
          % (label, len(blob), len(set(members)),
             (gcc.group(0).decode("ascii", "replace") if gcc else "no GCC tag")))
