#!/usr/bin/env python3
"""Stages the iOS app's GameData from a prepared folder, at a fraction of its size (dev tool).

- `horizon-data.bin`: every file but the sounds, zstd-compressed, in one packed file
  (`cargo run -p xtask -- ios-pack`, crates/horizon-core/src/prepared_files.rs). Of vanilla's
  languages only LANGUAGES go in; the app fetches the others when the player picks one.
- the sounds, re-encoded as Ogg Vorbis at quality QUALITY (oggenc's -q; 0 is about 64 kbit/s
  for stereo music, against vanilla's ~80-128) and at most 48 kHz; a file that would not shrink
  stays as it is. Encoded files are kept in CACHE by their source's hash, so a rebuild only
  encodes what changed.
- `horizon-resources.json`, which the packed file is checked against, and the zip files (the
  built-in resource packs, read with seeks), which it leaves out too.

Needs oggdec and oggenc (`brew install vorbis-tools`).

    python3 tools/ios/stage_resources.py --resources .local/horizon-client/26.3 --out OUT/GameData
"""

import argparse
import concurrent.futures
import hashlib
import os
import shutil
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def vorbis_rate(path: Path) -> int:
    """The sample rate in an Ogg Vorbis file's identification header; 0 when not found."""
    head = path.read_bytes()[:256]
    at = head.find(b"\x01vorbis")
    if at < 0 or len(head) < at + 16:
        return 0
    # \x01vorbis, version (u32), channels (u8), rate (u32), little-endian.
    return struct.unpack_from("<I", head, at + 12)[0]


def encode(source: Path, cache: Path, quality: str) -> Path:
    """`source` re-encoded (or itself, when that is no smaller), from the cache when it is there."""
    digest = hashlib.sha1(source.read_bytes()).hexdigest()
    rate = vorbis_rate(source)
    resample = rate > 48000
    key = f"{digest}-q{quality}{'-48k' if resample else ''}"
    done = cache / f"{key}.ogg"
    kept = cache / f"{key}.same"
    if done.exists():
        return done
    if kept.exists():
        return source
    partial = cache / f"{key}.{os.getpid()}.part"
    decode = subprocess.Popen(["oggdec", "-Q", "-o", "-", str(source)], stdout=subprocess.PIPE)
    command = ["oggenc", "-Q", "-q", quality, "-o", str(partial)]
    if resample:
        command += ["--resample", "48000"]
    encoded = subprocess.run([*command, "-"], stdin=decode.stdout, stderr=subprocess.DEVNULL)
    decode.stdout.close()
    if decode.wait() != 0 or encoded.returncode != 0:
        partial.unlink(missing_ok=True)
        raise RuntimeError(f"{source}: re-encoding failed")
    if partial.stat().st_size >= source.stat().st_size:
        partial.unlink()
        kept.touch()
        return source
    partial.rename(done)
    return done


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--resources", type=Path, required=True, help="the prepared folder")
    parser.add_argument("--out", type=Path, required=True, help="the GameData folder to write")
    parser.add_argument("--cache", type=Path, default=ROOT / "target/ios/sound-cache")
    parser.add_argument("--quality", default="0", help="oggenc's -q (default 0)")
    parser.add_argument("--languages", default="en_us", help="vanilla's lang files to ship")
    args = parser.parse_args()
    for tool in ("oggdec", "oggenc"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} not found: brew install vorbis-tools")
    resources = args.resources.resolve()
    if not (resources / "horizon-resources.json").is_file():
        sys.exit(f"{resources} is not a prepared folder (no horizon-resources.json)")
    if args.out.exists():
        shutil.rmtree(args.out)
    args.out.mkdir(parents=True)
    args.cache.mkdir(parents=True, exist_ok=True)

    subprocess.run(
        ["cargo", "run", "-q", "--release", "-p", "xtask", "--", "ios-pack", str(resources),
         str(args.out / "horizon-data.bin"), args.languages],
        cwd=ROOT,
        check=True,
    )
    shutil.copy2(resources / "horizon-resources.json", args.out / "horizon-resources.json")
    for archive in sorted(resources.rglob("*.zip")):
        staged = args.out / archive.relative_to(resources)
        staged.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(archive, staged)

    sounds = sorted(resources.rglob("*.ogg"))
    before = after = 0
    with concurrent.futures.ThreadPoolExecutor(os.cpu_count()) as pool:
        jobs = {pool.submit(encode, s, args.cache, args.quality): s for s in sounds}
        for job in concurrent.futures.as_completed(jobs):
            source = jobs[job]
            staged = args.out / source.relative_to(resources)
            staged.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(job.result(), staged)
            before += source.stat().st_size
            after += staged.stat().st_size
    print(
        f"sounds: {len(sounds)} files, {before / 2**20:.1f} MB -> {after / 2**20:.1f} MB "
        f"(oggenc -q {args.quality})"
    )
    total = sum(f.stat().st_size for f in args.out.rglob("*") if f.is_file())
    print(f"GameData: {total / 2**20:.1f} MB in {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
