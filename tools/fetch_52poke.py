#!/usr/bin/env python3
"""Fill missing Chinese dex flavor text from the 52poke wiki (community
translations for species never released in an official Chinese game).

Reads tools/zh_cache.json, finds ids with flavor == None, scrapes each
species' wiki page HTML (api.php is unreliable; article pages are CDN
cached), parses the 图鉴介绍 table and picks one text by version
preference (FireRed-consistent first).

Writes tools/zh52poke_cache.json {id: text}. gen_assets.py merges it in.
Safe to kill and restart: finished ids persist.
"""

import html as htmllib
import json
import os
import re
import sys
import time
import urllib.parse
import urllib.request

ROOT = __file__.rsplit("/", 2)[0]
ZH = os.path.join(ROOT, "tools", "zh_cache.json")
OUT = os.path.join(ROOT, "tools", "zh52poke_cache.json")
UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 esp32-pokedex"

VERSION_PREFERENCE = [
    "火红", "叶绿", "火紅", "葉綠",
    "绿宝石", "綠寶石", "红宝石", "紅寶石", "蓝宝石", "藍寶石",
    "水晶",
    "金", "銀", "银",
    "究极之日", "究极之月", "太阳", "月亮", "剑", "盾",
]


def strip_tags(s: str) -> str:
    s = re.sub(r"<[^>]+>", "", s)
    s = htmllib.unescape(s)
    return " ".join(s.split())


def parse_section(page_html: str) -> list[tuple[str, str]]:
    start = page_html.find('id="图鉴介绍"')
    if start < 0:
        return []
    end = page_html.find("<h2", start)
    section = page_html[start : end if end > 0 else len(page_html)]

    # version labels are <th> cells; descriptions are leaf cells with
    # class "at-l". The label table for each row sits immediately before
    # its description cell (nesting defeats naive <tr> parsing).
    ths = [(m.start(), strip_tags(m.group(1))) for m in re.finditer(r"<th[^>]*>(.*?)</th>", section, re.S)]
    out = []
    last_pos = 0
    for m in re.finditer(r'<td class="at-l[^"]*"[^>]*>(.*?)</td>', section, re.S):
        text = strip_tags(m.group(1))
        if len(text) < 8 or "{{" in text:  # skip empty/spacer/template cells
            continue
        label = ",".join(t for p, t in ths if last_pos <= p < m.start())
        out.append((label, text))
        last_pos = m.end()
    return out


def pick(entries: list[tuple[str, str]]) -> str | None:
    for pref in VERSION_PREFERENCE:
        for label, text in entries:
            if pref in label:
                return text
    return entries[0][1] if entries else None


def fetch_page(name: str) -> str:
    url = "https://wiki.52poke.com/wiki/" + urllib.parse.quote(name)
    last = None
    for attempt in range(4):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=60) as r:
                return r.read().decode("utf-8", "replace")
        except Exception as e:  # noqa: BLE001
            last = e
            time.sleep(2 * (attempt + 1))
    raise last


def main() -> None:
    zh = json.load(open(ZH))
    targets = [i for i in range(1, 387) if zh[str(i)]["flavor"] is None]
    cache = json.load(open(OUT)) if os.path.exists(OUT) else {}
    targets = [i for i in targets if str(i) not in cache]
    print(f"{len(cache)} cached, fetching {len(targets)}", flush=True)

    errors = []
    for n, pid in enumerate(targets, 1):
        name = zh[str(pid)]["name"]
        try:
            entries = parse_section(fetch_page(name))
            text = pick(entries)
            if not text:
                errors.append((pid, "no 图鉴介绍 rows"))
                continue
            cache[str(pid)] = text
            print(f"  {pid} {name}: {text[:24]}...", flush=True)
        except Exception as e:  # noqa: BLE001
            errors.append((pid, f"{type(e).__name__}: {e}"))
        if n % 5 == 0:
            json.dump(cache, open(OUT, "w"), ensure_ascii=False, indent=0)
    json.dump(cache, open(OUT, "w"), ensure_ascii=False, indent=0)
    print(f"done: {len(cache)} cached, errors {len(errors)}: {errors[:5]}")
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
