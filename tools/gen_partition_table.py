#!/usr/bin/env python3
"""Generate the ESP32 partition table binary ( flashed at 0x8000 ).

Layout keeps nvs/phy identical to the FoloToy factory table and grows the
factory app to the rest of the 8 MB flash (same as ai-passport upstream
default partitions.csv, minus the old product's asset partitions):

  nvs       data/nvs   0x9000    0x6000
  phy_init  data/phy   0xf000    0x1000
  factory   app/factory 0x10000  0x7F0000   (0x10000 + 0x7F0000 = 8 MB)

Entry format per ESP-IDF `gen_esp32part`: 32 bytes each —
magic bytes 0xAA 0x50, type, subtype, offset u32le, size u32le, 16-byte
label, flags u32le; an MD5 trailer entry (0xEB 0xEB + md5 of prior
entries) is appended; the table is 0xFF-padded to 4 KiB.
"""

import hashlib
import os
import struct

ROOT = __file__.rsplit("/", 2)[0]
OUT = f"{ROOT}/build/partitions.bin"

APP, DATA = 0, 1
SUBTYPE_FACTORY, SUBTYPE_PHY, SUBTYPE_NVS = 0x00, 0x01, 0x02
MAGIC = b"\xAA\x50"

ENTRIES = [
    (DATA, SUBTYPE_NVS, 0x9000, 0x6000, "nvs"),
    (DATA, SUBTYPE_PHY, 0xF000, 0x1000, "phy_init"),
    (APP, SUBTYPE_FACTORY, 0x10000, 0x7F0000, "factory"),
]


def entry(etype: int, subtype: int, offset: int, size: int, label: str) -> bytes:
    return struct.pack(
        "<2sBBLL16sL",
        MAGIC,
        etype,
        subtype,
        offset,
        size,
        label.encode(),
        0,
    )


def main() -> None:
    body = b"".join(entry(*e) for e in ENTRIES)
    md5_trailer = b"\xEB\xEB" + b"\xFF" * 14 + hashlib.md5(body).digest()
    # 0xC00 like gen_esp32part: 3K for entries, 1K of the 4K sector kept for signature
    table = (body + md5_trailer).ljust(0xC00, b"\xFF")
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    open(OUT, "wb").write(table)
    print(f"wrote {OUT} ({len(table)} bytes, entries={len(ENTRIES)})")
    assert 0x10000 + 0x7F0000 == 0x800000, "factory must end exactly at 8 MB"


if __name__ == "__main__":
    main()
