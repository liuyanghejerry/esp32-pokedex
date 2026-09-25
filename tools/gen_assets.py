#!/usr/bin/env python3
"""Generate pokedex firmware assets from open-pokefirered data (read-only).

Sources (never modified):
  ~/develop/open-pokefirered/firered/assets/pokedex.json         species base data
  ~/develop/open-pokefirered/firered/assets/pokedex_entries.json dex text / height / weight
  ~/develop/open-pokefirered/firered/assets/battle_fronts.bin    386 x 64x64 RGBA8888 front sprites
  ~/develop/open-pokefirered/firered/assets/battle_fronts.json   name -> byte offset
  unown_fronts.bin (form 'A' fallback for #201), pokemon_icons.bin (fallback for #360)

Outputs (into this repo only):
  firmware/assets/sprites.bin   386 x (16-colour BGR565 palette + 4bpp indices)
  firmware/src/dex_data.rs      Rust static tables for the national dex (1..=386)

Sprites are 4 bits per pixel: index 0 is transparent, 1..=15 index the
sprite's own palette. GBA front sprites come from 4bpp artwork, so this is
lossless — the generator asserts that no sprite needs more than 15 opaque
colours, and that the packed blob decodes back to the source pixels exactly.
(At 8 KB of RGB565 + 512 B of mask per sprite the blob was 3.4 MB, which did
not leave room for the cries and BGM in the chip's 8 MB of flash MMU.)

Known data gaps and fallbacks:
  - pokedex_entries.json misses CHIKORITA (#152) and TYRANITAR (#248):
    placeholder category/description/height/weight are emitted.
  - battle_fronts.bin misses UNOWN (#201) and CASTFORM (#360):
    UNOWN uses form 'A' from unown_fronts.bin; CASTFORM uses its 32x32
    menu icon (first frame) nearest-upsampled to 64x64.
"""

import json
import os
import sys

SRC = os.path.expanduser("~/develop/open-pokefirered/firered/assets")
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_BIN = os.path.join(ROOT, "firmware", "assets", "sprites.bin")
OUT_RS = os.path.join(ROOT, "firmware", "src", "dex_data.rs")
ZH_CACHE = os.path.join(ROOT, "tools", "zh_cache.json")
OUT_CHARSET = os.path.join(ROOT, "tools", "charset.json")

DEX_COUNT = 386

# every UI string the firmware draws (keeps the font subset complete);
# a-z covers the English flavor-text fallback for species without
# official Chinese dex text
UI_STRINGS = (
    "宝可梦图鉴身高体重攻击防御特攻特防速度未知上下切换确详情"
    "一般格斗飞行毒地面岩石虫幽灵钢火水草电超能力冰龙恶"
    "HPmkg/.:%·…（）()0123456789 "
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
    ",;!?-'\""
)

TRANSLIT = {
    "é": "e",
    "É": "E",
    "♀": " F",
    "♂": " M",
    "\u2019": "'",
}


def clean_text(s: str) -> str:
    for k, v in TRANSLIT.items():
        s = s.replace(k, v)
    s = s.replace("_", " ")
    return " ".join(s.replace("\n", " ").split())


def rgb565(r: int, g: int, b: int) -> int:
    return ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3)


def rgb565_mask_ref(rgba: bytes) -> tuple[bytearray, bytearray]:
    """Reference (unpacked) encoding, kept only to verify `to_indexed`."""
    px = bytearray(8192)
    mask = bytearray(512)
    for i in range(4096):
        r, g, b, a = rgba[4 * i : 4 * i + 4]
        if a:
            c = rgb565(r, g, b)
            px[2 * i] = c >> 8
            px[2 * i + 1] = c & 0xFF
            mask[i >> 3] |= 0x80 >> (i & 7)
    return px, mask


def to_indexed(rgba: bytes) -> tuple[bytes, bytes]:
    """Pack one 64x64 RGBA sprite into (16-colour palette, 4bpp indices).

    Index 0 is transparent and 1..=15 index the palette, which is written as a
    fixed 16 entries so every sprite has the same stride.
    """
    palette: list[int] = []
    lookup: dict[int, int] = {}
    idx = bytearray(2048)
    for i in range(4096):
        r, g, b, a = rgba[4 * i : 4 * i + 4]
        v = 0
        if a:
            c = rgb565(r, g, b)
            v = lookup.get(c, 0)
            if v == 0:
                v = len(palette) + 1
                if v > 15:
                    sys.exit(f"sprite needs more than 15 opaque colours (pixel {i})")
                lookup[c] = v
                palette.append(c)
        idx[i >> 1] |= (v << 4) if (i & 1) == 0 else v
    # Index 0 is reserved for transparency, so the palette starts at slot 1.
    pal = bytearray(2)
    for c in palette:
        pal += bytes((c >> 8, c & 0xFF))
    pal += bytes(2 * (15 - len(palette)))
    return bytes(pal), bytes(idx)


def decode_indexed(blob: bytes, base: int) -> tuple[list[int], bytearray, bytearray]:
    """Inverse of `to_indexed`, used to prove the blob round-trips."""
    pal = [int.from_bytes(blob[base + 2 * i : base + 2 * i + 2], "big") for i in range(16)]
    px = bytearray(8192)
    mask = bytearray(512)
    for i in range(4096):
        byte = blob[base + 32 + (i >> 1)]
        v = (byte >> 4) if (i & 1) == 0 else (byte & 0xF)
        if v:
            c = pal[v]
            px[2 * i] = c >> 8
            px[2 * i + 1] = c & 0xFF
            mask[i >> 3] |= 0x80 >> (i & 7)
    return pal, px, mask


def main() -> None:
    pd = json.load(open(f"{SRC}/pokedex.json"))
    entries = json.load(open(f"{SRC}/pokedex_entries.json"))
    fronts = json.load(open(f"{SRC}/battle_fronts.bin".replace(".bin", ".json")))
    blob = open(f"{SRC}/battle_fronts.bin", "rb").read()
    unown = open(f"{SRC}/unown_fronts.bin", "rb").read()
    icons = open(f"{SRC}/pokemon_icons.bin", "rb").read()
    type_ids = pd["types"]
    species = pd["species"]
    zh = json.load(open(ZH_CACHE))
    w52 = {}
    if os.path.exists(os.path.join(ROOT, "tools", "zh52poke_cache.json")):
        w52 = json.load(open(os.path.join(ROOT, "tools", "zh52poke_cache.json")))
    missing_zh = [i for i in range(1, DEX_COUNT + 1) if str(i) not in zh]
    if missing_zh:
        sys.exit(f"tools/zh_cache.json missing ids {missing_zh[:10]}... run tools/fetch_zh.py")

    rows = []
    sprites = bytearray()
    for no in range(1, DEX_COUNT + 1):
        sp = species[no]
        name = sp["name"]
        disp = clean_text(name)
        ent = entries.get(name)
        if ent is None:
            ent = {
                "category": "UNKNOWN",
                "desc": "No data recorded for this POKeMON.",
                "height": 0,
                "weight": 0,
            }

        t = [type_ids[x] for x in sp["types"]]
        t2 = t[1] if len(t) > 1 and t[1] != t[0] else 255

        off = fronts.get(name.lower())
        if off is not None:
            rgba = blob[off : off + 16384]
        elif name == "UNOWN":
            rgba = unown[0:16384]
        elif name == "CASTFORM":
            # 32x32 menu icon (first frame), nearest-upsampled to 64x64
            small = icons[no * 8192 : no * 8192 + 4096]
            rgba = bytearray(16384)
            for y in range(32):
                for x in range(32):
                    q = small[(y * 32 + x) * 4 : (y * 32 + x) * 4 + 4]
                    for dy in (0, 1):
                        for dx in (0, 1):
                            i = ((y * 2 + dy) * 64 + x * 2 + dx) * 4
                            rgba[i : i + 4] = q
        else:
            sys.exit(f"missing front sprite for #{no} {name}")

        packed = b"".join(to_indexed(rgba))
        # Packed form must decode back to exactly the same pixels.
        _, dec_px, dec_mask = decode_indexed(packed, 0)
        exp_px, exp_mask = rgb565_mask_ref(rgba)
        if dec_px != exp_px or dec_mask != exp_mask:
            sys.exit(f"#{no} {name}: 4bpp packing is not lossless")
        sprites += packed

        zh_rec = zh[str(no)]
        desc = zh_rec["flavor"]
        if desc is None and str(no) in w52:  # 52poke community translation
            desc = w52[str(no)]
        if desc is None:  # no Chinese text at all -> English FireRed
            desc = clean_text(ent["desc"]) if ent else "No data recorded."
        rows.append(
            dict(
                no=no,
                name=zh_rec["name"],
                name_en=disp,
                category=zh_rec["genus"],
                t1=t[0],
                t2=t2,
                hp=sp["base_hp"],
                atk=sp["base_atk"],
                dfn=sp["base_def"],
                spa=sp["base_spatk"],
                spd=sp["base_spdef"],
                spe=sp["base_speed"],
                height=ent["height"],
                weight=ent["weight"],
                desc=desc,
            )
        )

    os.makedirs(os.path.dirname(OUT_BIN), exist_ok=True)
    os.makedirs(os.path.dirname(OUT_RS), exist_ok=True)
    open(OUT_BIN, "wb").write(sprites)
    print(f"wrote {OUT_BIN} ({len(sprites)} bytes)")

    charset = set(UI_STRINGS)
    for r in rows:
        charset.update(r["name"])
        charset.update(r["category"])
        charset.update(r["desc"])
    json.dump({"chars": "".join(sorted(charset))}, open(OUT_CHARSET, "w"), ensure_ascii=False)
    print(f"wrote {OUT_CHARSET} ({len(charset)} chars)")

    with open(OUT_RS, "w", encoding="utf-8") as f:
        f.write("// @generated by tools/gen_assets.py — do not edit by hand.\n")
        f.write("// Source: open-pokefirered (stats/sprites) + PokeAPI zh-Hans\n")
        f.write("// (official Chinese names/genera/flavor text).\n")
        f.write("pub const DEX_LEN: usize = ")
        f.write(f"{DEX_COUNT};\n")
        f.write("pub const SPRITE_STRIDE: usize = 2080;\n\n")
        f.write("/// One species. `types` uses type ids; 255 = none.\n")
        f.write("#[allow(dead_code)]\n")
        f.write("pub struct DexEntry {\n")
        f.write("    pub no: u16,\n    pub name: &'static str,\n")
        f.write("    pub name_en: &'static str,\n")
        f.write("    pub category: &'static str,\n    pub types: [u8; 2],\n")
        f.write("    /// HP, ATK, DEF, SPA, SPD, SPE\n    pub base: [u8; 6],\n")
        f.write("    pub height_dm: u8,\n    pub weight_hg: u16,\n")
        f.write("    pub desc: &'static str,\n}\n\n")
        f.write("pub static DEX: [DexEntry; DEX_LEN] = [\n")
        for r in rows:
            desc = r["desc"].replace("\\", "\\\\").replace('"', '\\"')
            f.write(
                f"    DexEntry {{ no: {r['no']}, name: \"{r['name']}\", "
                f"name_en: \"{r['name_en']}\", "
                f"category: \"{r['category']}\", types: [{r['t1']}, {r['t2']}], "
                f"base: [{r['hp']}, {r['atk']}, {r['dfn']}, {r['spa']}, {r['spd']}, {r['spe']}], "
                f"height_dm: {r['height']}, weight_hg: {r['weight']}, "
                f"desc: \"{desc}\" }},\n"
            )
        f.write("];\n")
    print(f"wrote {OUT_RS} ({len(rows)} entries)")


if __name__ == "__main__":
    main()

