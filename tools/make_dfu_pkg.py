#!/usr/bin/env python3
"""Build a Nordic Secure-DFU .zip for the nRF52840 Dongle's factory bootloader.

Why this script exists
----------------------
Flashing the dongle from the command line needs a DFU package. Every off-the-shelf
tool on this machine failed:

  * `nrfutil` (Nordic, PyPI) 6.x      -> will not install on Python 3.14
  * `nrfutil` (Nordic, PyPI) 5.2.0    -> installed, but is Python-2-only
                                         (`dict.iteritems`), cannot even import
  * `adafruit-nrfutil` 0.5.3          -> only generates *legacy* (manifest 0.5)
                                         packages; its 0.7/0.8 (secure) paths are
                                         broken (`binascii.hexlify` returns bytes,
                                         which json.dumps cannot serialize)

So we build the package ourselves. The pieces are small and fully specified by the
`.proto` that ships with `dfu_cc_pb2`:

  <name>.bin   the raw application image
  <name>.dat   protobuf *secure* init packet: SHA-256 hash + size + type + versions
  manifest.json  {"manifest": {"application": {"bin_file": ..., "dat_file": ...}}}

Note the manifest deliberately has **no `dfu_version` key**. `nrfutil-device`'s
parser walks every entry of the `manifest` object and expects each one to be a
firmware object with `bin_file`/`dat_file`; an extra numeric `dfu_version` entry
makes it report the confusing
    Internal sdfu error: bin_file not found in Number(0.5)
(proof: strings inside nrfutil-device.exe contain the field-name blob
 `applicationbootloadermanifest...bin_filedat_file` and no `dfu_version`.)

Usage:
  python make_dfu_pkg.py --bin usb-hid-probe.bin --out usb-hid-probe-dfu.zip \
      [--app-version 1] [--hw-version 52] [--sd-req 0x00]
"""

import argparse
import binascii
import hashlib
import json
import os
import sys
import zipfile

sys.path.insert(0, r"C:\Python314\Lib\site-packages")

from nordicsemi.dfu import dfu_cc_pb2 as pb  # noqa: E402


def build_init_packet(bin_path, app_version, hw_version, sd_req, boot_validation):
    data = open(bin_path, "rb").read()

    # !!! THE HASH GOES IN BYTE-REVERSED !!!
    #
    # The nRF5 DFU bootloader compares its computed SHA-256 against this field
    # *as a little-endian byte string*. nrfutil does the same thing, with a
    # comment that gives the game away (nordicsemi/dfu/package.py,
    # calculate_sha256_hash):
    #
    #     # return hash in little endian
    #     sha256 = digest.digest()
    #     return sha256[31::-1]
    #
    # Writing the digest in normal order uploads perfectly (the bootloader even
    # reports a matching CRC-32 for every byte) and then fails the very last
    # ObjectExecute with result=ExtError, extended_error=0x0C ==
    # "The hash of the received firmware image does not match the hash in the
    # init packet." That cost several flash cycles; do not "fix" this line.
    digest = hashlib.sha256(data).digest()[::-1]

    packet = pb.Packet()
    packet.command.op_code = pb.INIT

    init = pb.InitCommand()
    init.fw_version = app_version
    init.hw_version = hw_version
    if sd_req:
        init.sd_req.extend(sd_req)
    init.type = pb.APPLICATION
    init.sd_size = 0
    init.bl_size = 0
    init.app_size = len(data)
    init.is_debug = False
    init.hash.hash_type = pb.SHA256
    init.hash.hash = digest

    # Nordic's own `nrfutil pkg generate` always emits exactly one
    # boot_validation entry (default VALIDATE_GENERATED_CRC, empty bytes) for an
    # unsigned application image. An empty list makes the bootloader reject the
    # init packet with ExtError/VerificationFailed.
    if boot_validation is not None:
        bv = init.boot_validation.add()
        bv.type = boot_validation
        bv.bytes = b""

    packet.command.init.CopyFrom(init)

    return packet.SerializeToString(), len(data), digest


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True, help="raw application .bin")
    ap.add_argument("--out", required=True, help="output .zip")
    ap.add_argument("--app-version", type=lambda v: int(v, 0), default=1)
    # 52 = the value Nordic's own dongle documentation uses (--hw-version 52).
    # 0xFFFFFFFF is nrfutil's "unspecified" default.
    ap.add_argument("--hw-version", type=lambda v: int(v, 0), default=52)
    ap.add_argument("--sd-req", default="0x00",
                    help="comma separated SoftDevice IDs; empty string = no requirement")
    ap.add_argument("--name", default=None, help="base name inside the zip")
    # VALIDATE_GENERATED_CRC=1, VALIDATE_SHA256=2, VALIDATE_ECDSA_P256_SHA256=3;
    # "none" omits the field entirely.
    ap.add_argument("--boot-validation", default="1",
                    help="0/1/2/3 or 'none'")
    args = ap.parse_args()

    name = args.name or os.path.splitext(os.path.basename(args.bin))[0]
    sd_req = [int(x, 0) for x in args.sd_req.split(",")] if args.sd_req else []
    bv = None if args.boot_validation == "none" else int(args.boot_validation, 0)

    dat, size, digest = build_init_packet(args.bin, args.app_version,
                                         args.hw_version, sd_req, bv)

    manifest = {
        "manifest": {
            "application": {
                "bin_file": name + ".bin",
                "dat_file": name + ".dat",
            }
        }
    }

    with zipfile.ZipFile(args.out, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr(name + ".bin", open(args.bin, "rb").read())
        z.writestr(name + ".dat", dat)
        z.writestr("manifest.json", json.dumps(manifest, indent=4))

    print(f"package     : {args.out}")
    print(f"app size    : {size}")
    print(f"sha256      : {binascii.hexlify(digest).decode()}")
    print(f"fw_version  : {args.app_version}")
    print(f"hw_version  : {args.hw_version} (0x{args.hw_version:X})")
    print(f"sd_req      : {sd_req}")
    print(f"init packet : {len(dat)} bytes")


if __name__ == "__main__":
    main()
