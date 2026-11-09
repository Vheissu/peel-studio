#!/usr/bin/env python3
"""Prepare the bundled CC0 guitar and bass recordings. Requires NumPy/ffmpeg."""

import argparse
import concurrent.futures
import hashlib
import json
from pathlib import Path
import re
import subprocess
import urllib.request
import wave

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
RATE = 48_000
SOURCES = {
    "guitar": ("karoryfer.black-and-green-guitars", "b3b3249d37dc977a1a297bd2dc053e6d9b6b805c"),
    "bass": ("karoryfer.black-and-blue-basses", "6e7d674cdb41be7a54dbccb15472401ad01099b9"),
}
GUITAR_ROOTS = list(range(40, 86, 3))
BASS_ROOTS = list(range(23, 63, 3))


def sha(data):
    return hashlib.sha256(data).hexdigest()


def fetch(kind, path, cache):
    repo, revision = SOURCES[kind]
    file = cache / repo / path
    if not file.exists():
        file.parent.mkdir(parents=True, exist_ok=True)
        url = f"https://raw.githubusercontent.com/sfzinstruments/{repo}/{revision}/{path}"
        with urllib.request.urlopen(url, timeout=60) as response:
            data = response.read()
        file.write_bytes(data)
    return file


def regions(text):
    for region in text.split("<region>")[1:]:
        sample = re.search(r"sample=(\S+)", region)
        root = re.search(r"pitch_keycenter=(\d+)", region)
        if sample and root:
            yield int(root[1]), sample[1].replace("\\", "/").removeprefix("../")


def selected(cache):
    sustain = fetch("guitar", "Programs/modules/maps_green/ord.sfz", cache).read_text()
    mute = fetch("guitar", "Programs/modules/maps_green/stac.sfz", cache).read_text()
    sources = dict(regions(sustain))
    muted = dict(regions(mute))
    out = []
    for root in GUITAR_ROOTS:
        stem = re.sub(r"_(p|mf|f)_rr\d+\.wav$", "", sources[root])
        for layer, strength in enumerate(("p", "mf", "f"), 1):
            for rr in (1, 2):
                path = f"{stem}_{strength}_rr{rr}.wav"
                out.append(("guitar", "sustain", root, layer, rr, path, 12.0))
        mute_stem = re.sub(r"_rr\d+\.wav$", "", muted[root])
        note_name = Path(stem).name.removeprefix("twang_")
        for rr in (1, 2):
            out.append(("guitar", "mute", root, 1, rr, f"{mute_stem}_rr{rr}.wav", 1.0))
            out.append(("guitar", "release", root, 1, rr, f"Samples/green/rel/release_{note_name}_rr{rr}.wav", .5))
    for layer, strength in enumerate(("f", "ff"), 1):
        mapping = fetch("bass", f"Programs/maps/babyblue_reg_{strength}_map.sfz", cache).read_text()
        source_map = dict(regions(mapping))
        for root in BASS_ROOTS:
            # The original mapping writes bass an octave above sounding pitch.
            # A source b1 sample measures ~31 Hz (B0, key23), not B1 (key35).
            stem = re.sub(r"_rr\d+\.wav$", "", source_map[root + 12])
            for rr in (1, 2, 3):
                out.append(("bass", "pick", root, layer, rr, f"{stem}_rr{rr}.wav", 5.0))
    return out


def decode(file):
    data = subprocess.check_output([
        "ffmpeg", "-v", "error", "-i", str(file), "-ar", str(RATE),
        "-ac", "1", "-f", "f32le", "-",
    ])
    return np.frombuffer(data, "<f4").astype(np.float64)


def prepare(audio, length):
    raw = subprocess.check_output([
        "ffmpeg", "-v", "error", "-f", "f64le", "-ar", str(RATE),
        "-ac", "1", "-i", "-", "-af", "highpass=f=8:p=1", "-f", "f64le", "-",
    ], input=audio.astype("<f8").tobytes())
    audio = np.frombuffer(raw, "<f8").copy()
    if not len(audio) or not np.isfinite(audio).all():
        raise ValueError("Invalid source recording")
    peak = float(np.max(np.abs(audio)))
    if peak < .00001:
        raise ValueError("Silent source recording")
    onset = np.flatnonzero(np.abs(audio) > max(.00001, peak * .005))
    start = max(0, int(onset[0]) - 48)
    audible = np.flatnonzero(np.abs(audio) > max(.00001, peak * .0003))
    end = min(len(audio), int(audible[-1]) + 2401, start + int(length * RATE))
    audio = audio[start:end].copy()
    attack = min(12, len(audio))
    audio[:attack] *= np.linspace(0., 1., attack)
    fade = min(960, len(audio))
    audio[-fade:] *= np.linspace(1., 0., fade)
    return audio, start


def pitch_hz(audio, root):
    # Autocorrelation around the expected fundamental, for provenance and
    # diagnostics. The median across takes supplies one correction per root,
    # retaining pitch movement and differences within each performance.
    audio = audio[4800:28800:4]
    if len(audio) < 2400:
        return None
    rate = RATE / 4
    audio = audio - np.mean(audio)
    n = 1 << (len(audio) * 2 - 1).bit_length()
    spectrum = np.fft.rfft(audio, n)
    corr = np.fft.irfft(spectrum * np.conj(spectrum), n)[:len(audio)]
    period = rate / (440 * 2 ** ((root - 69) / 12))
    lo, hi = max(2, int(period * .94)), min(len(corr)-2, int(period * 1.06)+1)
    if hi <= lo:
        return None
    lag = lo + int(np.argmax(corr[lo:hi + 1]))
    a, b, c = corr[lag-1:lag+2]
    shift = .5 * (a-c) / (a-2*b+c) if abs(a-2*b+c) > 1e-12 else 0
    return float(rate / (lag + np.clip(shift, -.5, .5)))


def tune(audio, cents):
    source_rate = round(RATE * 2 ** (cents / 1200))
    raw = subprocess.check_output([
        "ffmpeg", "-v", "error", "-f", "f64le", "-ar", str(RATE),
        "-ac", "1", "-i", "-", "-af", f"asetrate={source_rate},aresample={RATE}",
        "-f", "f64le", "-",
    ], input=audio.astype("<f8").tobytes())
    audio = np.frombuffer(raw, "<f8").copy()
    # Resampling can extend the boundary by a few frames. Keep the short
    # fades and exact-zero ends required by the embedded playback bank.
    audio[:12] *= np.linspace(0., 1., 12)
    fade = min(960, len(audio))
    audio[-fade:] *= np.linspace(1., 0., fade)
    return audio


def build(cache, output):
    entries = selected(cache)
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        list(pool.map(lambda e: fetch(e[0], e[5], cache), entries))
    output.mkdir(parents=True, exist_ok=True)
    for kind in SOURCES:
        license_path = "license" if kind == "bass" else "LICENSE"
        (output / f"LICENSE-{kind}.txt").write_bytes(fetch(kind, license_path, cache).read_bytes())
    manifest = {
        "sources": {k: {"repository": f"https://github.com/sfzinstruments/{repo}", "revision": rev, "license": "CC0-1.0"} for k, (repo, rev) in SOURCES.items()},
        "format": "mono 48000Hz PCM16 WAV", "bass_mapping": "Source SFZ keys minus12, matching sounding pitch; b1 recording is B0 at about31Hz",
        "processing": "8Hz high-pass, leading silence trim with1ms pre-roll, .25ms attack fade and20ms tail fade; audible guitar sustain ends receive an additional exponential fade of up to1.5s to-60dB; one tuning correction per root based on median sustained pitch; one gain per instrument, no per-take normalization",
        "ffmpeg": subprocess.check_output(["ffmpeg", "-version"], text=True).splitlines()[0],
        "samples": [],
    }
    for kind in SOURCES:
        recordings = []
        for entry in entries:
            if entry[0] != kind:
                continue
            _, art, root, layer, rr, path, length = entry
            original = fetch(kind, path, cache)
            audio, trim = prepare(decode(original), length)
            recordings.append((entry, audio, trim, sha(original.read_bytes())))
        corrections = {}
        for root in sorted({entry[2] for entry, _, _, _ in recordings}):
            estimates = [pitch_hz(audio, root) for entry, audio, _, _ in recordings
                         if entry[2] == root and entry[1] in ("sustain", "pick")]
            expected = 440 * 2 ** ((root - 69) / 12)
            cents = [1200 * np.log2(hz / expected) for hz in estimates if hz]
            corrections[root] = -float(np.median(cents))
            if abs(corrections[root]) > 40:
                raise ValueError(f"Check source pitch: {kind} root{root}, {corrections[root]:.1f} cents")
        recordings = [(entry, tune(audio, corrections[entry[2]]), trim, source_sha)
                      for entry, audio, trim, source_sha in recordings]
        # A high-gain amp makes even quiet DI tails audible. Fade the end
        # of long sustains gradually if the source ends while still ringing.
        for entry, audio, _, _ in recordings:
            if entry[1] == "sustain":
                window = audio[max(0, len(audio) - 5760):max(1, len(audio) - 960)]
                tail_rms = float(np.sqrt(np.mean(window * window)))
                if tail_rms > np.max(np.abs(audio)) * .00001:
                    fade = min(72_000, max(12, len(audio) // 4))
                    audio[-fade:] *= np.geomspace(1., .001, fade)
        peak = max(float(np.max(np.abs(audio))) for _, audio, _, _ in recordings)
        gain = .7 / peak
        for entry, audio, trim, source_sha in recordings:
            _, art, root, layer, rr, path, _ = entry
            audio *= gain
            pcm = np.rint(audio * 32767).astype("<i2")
            file = output / f"{kind}-{art}-{root}-{layer}-{rr}.wav"
            with wave.open(str(file), "wb") as wav:
                wav.setnchannels(1); wav.setsampwidth(2); wav.setframerate(RATE)
                wav.writeframes(pcm.tobytes())
            manifest["samples"].append({
                "file": file.name, "instrument": kind, "articulation": art,
                "root_key": root, "layer": layer, "round_robin": rr,
                "source_path": path, "source_sha256": source_sha,
                "trimmed_source_frames": trim, "instrument_gain": gain,
                "root_tuning_cents": corrections[root],
                "frames": len(audio), "peak": float(np.max(np.abs(audio))),
                "rms": float(np.sqrt(np.mean(audio * audio))),
                "estimated_hz": pitch_hz(audio, root) if art != "release" else None,
                "sha256": sha(file.read_bytes()),
            })
        print(f"{kind}: {len(recordings)} recordings, common gain {gain:.4f}", flush=True)
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Prepared {len(entries)} recordings", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, default=Path("/tmp/peel-recorded-string-sources"))
    parser.add_argument("--output", type=Path, default=ROOT / "assets/strings/recorded")
    args = parser.parse_args()
    build(args.cache, args.output)
