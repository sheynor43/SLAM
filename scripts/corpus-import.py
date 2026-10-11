#!/usr/bin/env python3
"""Import replays exported from osu!lazer into the lazer replay corpus.

Usage: scripts/corpus-import.py [--osu-dir DIR] [--corpus DIR] [REPLAY.osr ...]

Without replay arguments, every .osr in <osu-dir>/exports is imported. The map of each
replay is found in the lazer file store (<osu-dir>/files) by the beatmap MD5 recorded in
the replay. Maps are stored as maps/<sha256>.osu, replays are copied to replays/, and a
manifest entry is appended for each replay not already listed. Replays whose map is not
in the file store, non-osu!standard replays and replays that are not lazer's own are
reported and skipped.
"""

import argparse
import hashlib
import os
import shutil
import struct
import sys
from pathlib import Path

# First replay version the corpus accepts as lazer's own: lower versions are stable replays,
# or stable scores converted by lazer (30000001), whose statistics are not lazer's.
FIRST_LAZER_SOURCE_VERSION = 30000002

LEGACY_MODS = [
    (1 << 0, "NF"), (1 << 1, "EZ"), (1 << 2, "TD"), (1 << 3, "HD"), (1 << 4, "HR"),
    (1 << 5, "SD"), (1 << 6, "DT"), (1 << 7, "RX"), (1 << 8, "HT"), (1 << 9, "NC"),
    (1 << 10, "FL"), (1 << 11, "AT"), (1 << 12, "SO"), (1 << 13, "AP"), (1 << 14, "PF"),
]


def read_string(data, pos):
    marker = data[pos]
    pos += 1
    if marker == 0:
        return "", pos
    if marker != 0x0B:
        raise ValueError(f"bad string marker {marker:#x}")
    length = shift = 0
    while True:
        byte = data[pos]
        pos += 1
        length |= (byte & 0x7F) << shift
        shift += 7
        if not byte & 0x80:
            break
    return data[pos:pos + length].decode("utf-8"), pos + length


def read_header(path):
    data = path.read_bytes()
    mode = data[0]
    (version,) = struct.unpack_from("<i", data, 1)
    pos = 5
    map_md5, pos = read_string(data, pos)
    _player, pos = read_string(data, pos)
    _replay_md5, pos = read_string(data, pos)
    pos += 6 * 2 + 4 + 2 + 1
    (mods,) = struct.unpack_from("<i", data, pos)
    names = [name for bit, name in LEGACY_MODS if mods & bit]
    # Nightcore and Perfect imply the bits of Double Time and Sudden Death.
    if "NC" in names and "DT" in names:
        names.remove("DT")
    if "PF" in names and "SD" in names:
        names.remove("SD")
    return mode, version, map_md5, names


def index_maps(files_dir, wanted):
    """MD5 -> path of the .osu files in the lazer file store whose MD5 is in `wanted`."""
    found = {}
    for dirpath, _dirs, names in os.walk(files_dir):
        for name in names:
            path = Path(dirpath, name)
            try:
                with path.open("rb") as f:
                    head = f.read(64)
                    if b"osu file format" not in head:
                        continue
                    data = head + f.read()
            except OSError:
                continue
            md5 = hashlib.md5(data).hexdigest()
            if md5 in wanted:
                found[md5] = path
                if len(found) == len(wanted):
                    return found
    return found


def toml_string(value):
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--osu-dir", type=Path,
                        default=Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share"), "osu"))
    parser.add_argument("--corpus", type=Path,
                        default=Path(os.environ.get("SLAM_CORPUS",
                                                    Path(__file__).resolve().parent.parent.parent / "slam-corpus")))
    parser.add_argument("replays", nargs="*", type=Path)
    args = parser.parse_args()

    replays = args.replays or sorted((args.osu_dir / "exports").glob("*.osr"))
    corpus = args.corpus / "lazer"
    (corpus / "maps").mkdir(parents=True, exist_ok=True)
    (corpus / "replays").mkdir(parents=True, exist_ok=True)
    manifest = corpus / "manifest.toml"
    listed = manifest.read_text(encoding="utf-8") if manifest.exists() else ""

    headers = {}
    for path in replays:
        mode, version, map_md5, mods = read_header(path)
        if mode != 0:
            print(f"skip (mode {mode}): {path.name}", file=sys.stderr)
        elif version < FIRST_LAZER_SOURCE_VERSION:
            print(f"skip (version {version}, not a lazer replay): {path.name}", file=sys.stderr)
        elif toml_string("replays/" + path.name) in listed:
            print(f"already listed: {path.name}", file=sys.stderr)
        else:
            headers[path] = (version, map_md5, mods)

    maps = index_maps(args.osu_dir / "files", {h[1] for h in headers.values()})

    added = 0
    entries = []
    for path, (version, map_md5, mods) in headers.items():
        source = maps.get(map_md5)
        if source is None:
            print(f"skip (map {map_md5} not in the file store): {path.name}", file=sys.stderr)
            continue
        sha256 = hashlib.sha256(source.read_bytes()).hexdigest()
        shutil.copyfile(source, corpus / "maps" / f"{sha256}.osu")
        shutil.copyfile(path, corpus / "replays" / path.name)
        notes = " ".join(mods) if mods else "NM"
        entries.append(
            "\n[[replay]]\n"
            f"file = {toml_string('replays/' + path.name)}\n"
            f"map_sha256 = {toml_string(sha256)}\n"
            'source = "lazer"\n'
            f"notes = {toml_string(f'{notes}; replay version {version}')}\n"
        )
        added += 1

    with manifest.open("a", encoding="utf-8") as f:
        f.writelines(entries)
    print(f"imported {added} of {len(replays)} replays into {corpus}")


if __name__ == "__main__":
    main()
