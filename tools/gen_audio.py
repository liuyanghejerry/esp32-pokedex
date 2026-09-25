#!/usr/bin/env python3
"""Generate the firmware audio blobs from open-pokefirered sound assets (read-only).

Sources (never modified):
  ~/develop/open-pokefirered/firered/assets/cries/index.json   389 cries, 8-bit 10512 Hz mono
  ~/develop/open-pokefirered/firered/assets/cries/*.wav        one WAV per species
  ~/develop/open-pokefirered/firered/assets/bgm/*.wav          rendered BGM (8/16-bit, 8k-13.4k)
  ~/develop/open-pokefirered/firered/assets/sfx/*.wav          menu/battle sound effects
  ~/develop/open-pokefirered/firered/assets/pokedex.json       species names (dex order)

Outputs (into this repo only):
  firmware/assets/cries.bin      386 cries, 8-bit unsigned PCM
  firmware/assets/bgm.bin        the BGM playlist, 8-bit unsigned PCM
  firmware/assets/sfx.bin        the SFX set + startup jingle
  firmware/src/audio_data.rs     clip tables (offset/length/gain) + include_bytes

Everything is resampled to one output rate so the firmware needs a single I2S
clock and a trivial mixer:

  * cries stay at their native 10512 Hz and are simply played 4.9% fast
    (10512 -> 11025). That is 84 cents, far below the point where a GBA cry
    sounds wrong, and it keeps the 386-cry payload byte-identical to the ROM.
  * BGM/SFX are resampled to 11025 Hz here, at build time.

`gain` is a loudness-normalisation factor (255 = unity) applied by the mixer:
every clip is lifted/attenuated so its peak lands near TARGET_PEAK. Playback
levels per source (cry / bgm / sfx) are the firmware's business, not this
script's.
"""

import array
import json
import math
import os
import sys
import wave

SRC = os.path.expanduser("~/develop/open-pokefirered/firered/assets")
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = os.path.join(ROOT, "firmware", "assets")
OUT_RS = os.path.join(ROOT, "firmware", "src", "audio_data.rs")

# The exporter's own BFP codec (6-bit mantissa + 4-bit block exponent). Tracks
# whose index entry says `codec: bfp` are stored **encoded** in their .wav: the
# data chunk is the bitstream, not PCM, so reading it as PCM yields white
# noise. Its decoder is integer-only and agrees bit for bit with the Rust one
# in firered-app, so we reuse the source of truth instead of reimplementing it.
CONVERTER = os.path.join(SRC, "..", "tools", "firered-asset-converter")
sys.path.insert(0, os.path.abspath(CONVERTER))
import bfp  # noqa: E402  (needs the sys.path entry above)

# Wire format of one BFP block: 1 exponent byte + 12 mantissa bytes = 16 samples.
BFP_BLOCK_BYTES = 13

DEX_COUNT = 386
OUT_RATE = 11025
# Peak every clip is normalised to before it is stored as 8-bit unsigned. The
# normalisation is baked into the blob (8-bit storage needs it anyway); the
# tables' per-clip `gain` is a *loudness trim* on top, unity in practice.
TARGET_PEAK = 0.99
MAX_BOOST = 2.0
# Total flash budget for the three blobs (bytes). The app partition is
# 0x7F0000; sprites take 0.8 MB of it, leaving room for this plus ~2.5 MB.
BUDGET = 5_000_000
# Anti-alias FIR (windowed sinc) applied when downsampling, and click guards.
LP_TAPS = 17
FADE_IN_MS = 1.5
FADE_OUT_MS = 6.0
# A track whose zero-crossing rate is above this is not audio at all (white
# noise sits at 0.5) — the check exists because the BFP-encoded tracks in the
# source tree look like PCM in their WAV headers.
MAX_ZCR = 0.32

# The boot-time BGM playlist: classic town/route themes from FireRed.
# (label, 中文标题, source WAV stem)
PLAYLIST = [
    ("PALLET", "真新镇", "mus_pallet"),
    ("ROUTE1", "1号道路", "mus_route1"),
    ("POKE_CENTER", "宝可梦中心", "mus_poke_center"),
    ("CELADON", "玉虹市", "mus_celadon"),
]

# Sound effects used by the UI and the boot animation. The startup jingle is
# FireRed's menu-exit sting, which lands well under the logo animation.
SFX = [
    ("STARTUP", "bgm/mus_new_game_exit.wav"),
    ("BALL_THROW", "sfx/se_ball_throw.wav"),
    ("BALL_BOUNCE_1", "sfx/se_ball_bounce_1.wav"),
    ("BALL_BOUNCE_2", "sfx/se_ball_bounce_2.wav"),
    ("BALL_BOUNCE_3", "sfx/se_ball_bounce_3.wav"),
    ("BALL_OPEN", "sfx/se_ball_open.wav"),
    ("BALL_CLICK", "sfx/se_ball_click.wav"),
    ("DEX_PAGE", "sfx/se_dex_page.wav"),
    ("DEX_SCROLL", "sfx/se_dex_scroll.wav"),
    ("CLICK", "sfx/se_click.wav"),
    ("CARD_FLIP", "sfx/se_card_flip.wav"),
]


def zcr(s):
    """Zero-crossing rate; ~0.5 means white noise."""
    n = len(s)
    if n < 2:
        return 0.0
    return sum(1 for i in range(1, n) if (s[i - 1] < 0) != (s[i] < 0)) / (n - 1)


def decode_bfp(raw, channels):
    """Decode the exporter's block-floating-point stream (see `bfp.py`)."""
    frames = len(raw) // (BFP_BLOCK_BYTES * channels) * bfp.BLOCK
    return [v / 32768.0 for v in bfp.decode(raw, channels, frames)]


def read_wav(path, bfp_encoded=False):
    """Read a WAV as (mono float samples in [-1,1], source rate)."""
    with wave.open(path) as w:
        n, ch, sw, rate = w.getnframes(), w.getnchannels(), w.getsampwidth(), w.getframerate()
        raw = w.readframes(n)
    if bfp_encoded:
        s = decode_bfp(raw, ch)
    elif sw == 1:  # unsigned
        s = [(b - 128) / 128.0 for b in raw]
    elif sw == 2:
        a = array.array("h")
        a.frombytes(raw)
        s = [v / 32768.0 for v in a]
    else:
        sys.exit(f"{path}: unsupported sample width {sw * 8} bit")
    if ch > 1:  # downmix (the board has one speaker anyway)
        s = [sum(s[i : i + ch]) / ch for i in range(0, len(s), ch)]
    return s, rate


def lowpass(s, cutoff_norm):
    """Windowed-sinc low-pass; `cutoff_norm` is the cutoff as a fraction of
    the sample rate (so 0.5 = Nyquist)."""
    m = LP_TAPS - 1
    h = []
    for i in range(LP_TAPS):
        x = i - m / 2
        if x == 0:
            v = 2 * cutoff_norm
        else:
            v = math.sin(2 * math.pi * cutoff_norm * x) / (math.pi * x)
        v *= 0.54 - 0.46 * math.cos(2 * math.pi * i / m)  # Hamming
        h.append(v)
    norm = sum(h)
    h = [v / norm for v in h]
    out = [0.0] * len(s)
    for i in range(len(s)):
        acc = 0.0
        for k in range(LP_TAPS):
            j = i + k - m // 2
            if 0 <= j < len(s):
                acc += s[j] * h[k]
        out[i] = acc
    return out


def resample(s, src_rate, dst_rate):
    """Linear interpolation, with an anti-alias pre-filter on downsample."""
    if src_rate == dst_rate:
        return list(s)
    if dst_rate < src_rate:
        s = lowpass(s, 0.45 * dst_rate / src_rate)
    n_out = int(len(s) * dst_rate / src_rate)
    out = [0.0] * n_out
    step = src_rate / dst_rate
    for i in range(n_out):
        pos = i * step
        i0 = int(pos)
        frac = pos - i0
        a = s[i0]
        b = s[i0 + 1] if i0 + 1 < len(s) else a
        out[i] = a + (b - a) * frac
    return out


def fade(s, rate):
    """Short fades so a clip never starts or ends on a step (audible click)."""
    n_in = max(1, int(rate * FADE_IN_MS / 1000))
    n_out = max(1, int(rate * FADE_OUT_MS / 1000))
    n_in = min(n_in, len(s))
    n_out = min(n_out, len(s))
    for i in range(n_in):
        s[i] *= i / n_in
    for i in range(n_out):
        s[len(s) - 1 - i] *= i / n_out
    return s


def gain_for(s):
    """Peak-normalisation factor. Baked into the stored samples (8-bit storage
    needs the fit anyway); the tables keep a separate loudness trim."""
    peak = max((abs(v) for v in s), default=0.0)
    if peak < 0.01:
        return 1.0
    return min(TARGET_PEAK / peak, MAX_BOOST)


def to_u8(s, gain):
    return bytes(min(255, max(0, int(v * gain * 128.0 + 128.0 + 0.5))) for v in s)


def pack_clip(s, rate, blob, do_fade, what):
    """Resample + fade + normalise one clip, append it to `blob` and return
    `(offset, length, loudness trim)`.

    The peak normalisation is baked into the samples — 8-bit storage needs the
    fit anyway — and the trim comes back at unity, so the mixer's per-source
    levels stay the only place playback loudness is decided.
    """
    s = resample(s, rate, OUT_RATE)
    if do_fade:
        s = fade(s, OUT_RATE)
    zc = zcr(s)
    if zc > MAX_ZCR:
        sys.exit(f"{what}: zero-crossing rate {zc:.2f} looks like noise, refusing to pack")
    data = to_u8(s, gain_for(s))
    off = len(blob)
    blob += data
    return off, len(data), 255


def load_wav(path, bfp_encoded=False):
    if not os.path.exists(path):
        sys.exit(f"missing source: {path}")
    return read_wav(path, bfp_encoded)


def build_cries():
    index = json.load(open(f"{SRC}/cries/index.json"))
    by_name = {c["species"]: c for c in index["cries"]}
    species = json.load(open(f"{SRC}/pokedex.json"))["species"]

    blob = bytearray()
    rows = []
    missing = []
    for no in range(1, DEX_COUNT + 1):
        name = species[no]["name"]
        rec = by_name.get(name)
        if rec is None:
            missing.append(no)
            rows.append((0, 0, 255))
            continue
        s, rate = load_wav(os.path.join(SRC, rec["source"]))
        # Cries keep their native rate: playing 10512 Hz data at 11025 Hz is a
        # 4.9% pitch lift, and it saves resampling all 386 of them.
        s = fade(s, rate)
        gain = gain_for(s)
        data = to_u8(s, gain)
        off = len(blob)
        blob += data
        rows.append((off, len(data), 255))
    if missing:
        print(f"  ! no cry for dex numbers {missing}")
    open(f"{OUT_DIR}/cries.bin", "wb").write(blob)
    print(f"  cries.bin   {len(blob):>9,} B  ({len(blob) / DEX_COUNT:.0f} B/species)")
    return rows, len(blob)


def build_bgm():
    index = {s["label"]: s for s in json.load(open(f"{SRC}/bgm/index.json"))["songs"]}
    blob = bytearray()
    rows = []
    for label, title, stem in PLAYLIST:
        meta = index.get(stem, {})
        encoded = meta.get("codec") == "bfp"
        s, rate = load_wav(f"{SRC}/bgm/{stem}.wav", bfp_encoded=encoded)
        off, ln, gain = pack_clip(s, rate, blob, False, stem)
        rows.append((title, off, ln, gain))
        kind = "bfp->pcm" if encoded else "pcm"
        print(f"  bgm {label:<12} {title:<6} {len(s):>7} samples @{rate} {kind} -> {ln:>8,} B")
    open(f"{OUT_DIR}/bgm.bin", "wb").write(blob)
    print(f"  bgm.bin     {len(blob):>9,} B")
    return rows, len(blob)


def build_sfx():
    blob = bytearray()
    rows = []
    for label, rel in SFX:
        s, rate = load_wav(f"{SRC}/{rel}")
        off, ln, gain = pack_clip(s, rate, blob, True, label)
        rows.append((label, off, ln, gain))
        print(f"  sfx {label:<14} {ln:>7,} B")
    open(f"{OUT_DIR}/sfx.bin", "wb").write(blob)
    print(f"  sfx.bin     {len(blob):>9,} B")
    return rows, len(blob)


def write_rs(cries, bgm, sfx):
    with open(OUT_RS, "w", encoding="utf-8") as f:
        f.write("// @generated by tools/gen_audio.py — do not edit by hand.\n")
        f.write("// Source: open-pokefirered cries / bgm / sfx (GBA, 8-bit, resampled to\n")
        f.write(f"// {OUT_RATE} Hz at build time; cries keep their native 10512 Hz data).\n\n")
        f.write(f"/// Output sample rate of every blob below.\n")
        f.write(f"pub const SAMPLE_RATE: u32 = {OUT_RATE};\n\n")
        f.write("/// One playable clip inside a blob. `gain` is loudness normalisation\n")
        f.write("/// (0..=255, 255 = unity); playback levels live in the mixer.\n")
        f.write("#[derive(Clone, Copy)]\n")
        f.write("pub struct Clip {\n")
        f.write("    pub off: u32,\n    pub len: u32,\n    pub gain: u8,\n}\n\n")

        f.write("/// A loopable BGM track.\n")
        f.write("#[derive(Clone, Copy)]\n")
        f.write("pub struct Track {\n")
        f.write("    pub title: &'static str,\n")
        f.write("    pub off: u32,\n    pub len: u32,\n    pub gain: u8,\n}\n\n")

        f.write("pub static CRY_DATA: &[u8] = include_bytes!(\"../assets/cries.bin\");\n")
        f.write("/// Indexed by dex number - 1.\n")
        f.write(f"pub static CRIES: [Clip; {DEX_COUNT}] = [\n")
        for off, ln, gain in cries:
            f.write(f"    Clip {{ off: {off}, len: {ln}, gain: {gain} }},\n")
        f.write("];\n\n")

        f.write("pub static BGM_DATA: &[u8] = include_bytes!(\"../assets/bgm.bin\");\n")
        f.write(f"pub static TRACKS: [Track; {len(bgm)}] = [\n")
        for title, off, ln, gain in bgm:
            f.write(f'    Track {{ title: "{title}", off: {off}, len: {ln}, gain: {gain} }},\n')
        f.write("];\n\n")

        f.write("pub static SFX_DATA: &[u8] = include_bytes!(\"../assets/sfx.bin\");\n")
        f.write("pub static SFX: [Clip; %d] = [\n" % len(sfx))
        for label, off, ln, gain in sfx:
            f.write(f"    Clip {{ off: {off}, len: {ln}, gain: {gain} }}, // {label}\n")
        f.write("];\n\n")

        f.write("/// Named indices into `SFX`.\n")
        f.write("#[allow(dead_code)] // complete table: a given build may not use every cue\n")
        f.write("pub mod sfx {\n")
        for i, (label, _, _, _) in enumerate(sfx):
            f.write(f"    pub const {label}: usize = {i};\n")
        f.write("}\n")
    print(f"  audio_data.rs written")


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    print("gen_audio: packing GBA sound assets")
    cries, n_cry = build_cries()
    bgm, n_bgm = build_bgm()
    sfx, n_sfx = build_sfx()
    write_rs(cries, bgm, sfx)

    total = n_cry + n_bgm + n_sfx
    print(f"  total       {total:>9,} B of {BUDGET:,} B budget")
    if total > BUDGET:
        sys.exit(f"over budget by {total - BUDGET:,} B — drop a BGM track from PLAYLIST")


if __name__ == "__main__":
    main()
