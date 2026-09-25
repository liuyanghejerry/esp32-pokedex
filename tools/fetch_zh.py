#!/usr/bin/env python3
"""Build tools/zh_cache.json from PokeAPI's bulk CSV exports (official zh-Hans).

Downloads (via jsdelivr mirror of github.com/PokeAPI/pokeapi, ~15 MB total):
  pokemon_species_names.csv          species id, language id, name, genus
  pokemon_species_flavor_text.csv    species id, version id, language id, text
  versions.csv / version_groups.csv  version -> version group mapping

Output: tools/zh_cache.json {id: {name, genus, flavor}} for ids 1..=386.
zh-Hans is local_language_id 12. Flavor preference mirrors the games'
release order, FireRed first; falls back to any official Chinese text.
"""

import csv
import json
import os
import sys
import urllib.request

ROOT = __file__.rsplit("/", 2)[0]
CACHE = os.path.join(ROOT, "tools", "zh_cache.json")
BASE = "https://cdn.jsdelivr.net/gh/PokeAPI/pokeapi@master/data/v2/csv"
ZH = "12"

FLAVOR_PREFERENCE = [
    "firered-leafgreen",
    "sword-shield",
    "brilliant-diamond-and-shining-pearl",
    "scarlet-violet",
    "legends-arceus",
    "lets-go-pikachu-lets-go-eevee",
    "ultra-sun-ultra-moon",
    "sun-moon",
    "omega-ruby-alpha-sapphire",
    "x-y",
    "black-2-white-2",
    "black-white",
    "heartgold-soulsilver",
    "platinum",
    "diamond-pearl",
    "emerald",
    "ruby-sapphire",
    "crystal",
    "gold-silver",
    "yellow",
    "red-blue",
]


def clean(s: str) -> str:
    return " ".join(s.replace("\n", " ").replace("\x0c", " ").replace("\u00ad", " ").split())


def fetch(name: str) -> str:
    path = f"/tmp/pokeapi_{name}.csv"
    if not os.path.exists(path):
        req = urllib.request.Request(f"{BASE}/{name}.csv", headers={"User-Agent": "esp32-pokedex/0.1"})
        with urllib.request.urlopen(req, timeout=120) as r, open(path, "wb") as f:
            f.write(r.read())
    return path


def rows(name: str):
    with open(fetch(name), newline="", encoding="utf-8") as f:
        yield from csv.DictReader(f)


def main() -> None:
    print("loading CSVs...")
    names = {}
    for r in rows("pokemon_species_names"):
        if r["local_language_id"] == ZH:
            names[int(r["pokemon_species_id"])] = (r["name"], r["genus"])

    vg_of_version = {}
    for r in rows("versions"):
        vg_of_version[int(r["id"])] = int(r["version_group_id"])
    vg_name = {}
    for r in rows("version_groups"):
        vg_name[int(r["id"])] = r["identifier"]
    vg_rank = {vg: i for i, vg in enumerate(FLAVOR_PREFERENCE)}

    best = {}  # species_id -> (rank, text)
    for r in rows("pokemon_species_flavor_text"):
        if r["language_id"] != ZH:
            continue
        sid = int(r["species_id"])
        if not 1 <= sid <= 386:
            continue
        vg = vg_name.get(vg_of_version.get(int(r["version_id"]), -1), "")
        rank = vg_rank.get(vg, len(FLAVOR_PREFERENCE))
        cur = best.get(sid)
        if cur is None or rank < cur[0]:
            best[sid] = (rank, r["flavor_text"])

    missing = [i for i in range(1, 387) if i not in names]
    if missing:
        sys.exit(f"missing zh names for ids {missing[:10]}...")

    cache = {
        str(i): {
            "name": names[i][0],
            "genus": names[i][1],
            # None => species never appeared in a Chinese-localized official
            # game; gen_assets.py falls back to the English FireRed text.
            "flavor": clean(best[i][1]) if i in best else None,
        }
        for i in range(1, 387)
    }
    n_no_flavor = sum(1 for v in cache.values() if v["flavor"] is None)
    json.dump(cache, open(CACHE, "w"), ensure_ascii=False, indent=0)
    print(f"wrote {CACHE} (386 entries, {n_no_flavor} without zh flavor)")
    print("sample:", cache["1"], "|", cache["152"], "|", cache["386"])


if __name__ == "__main__":
    main()
