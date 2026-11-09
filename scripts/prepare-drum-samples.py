#!/usr/bin/env python3
"""Build the bundled acoustic kit from a pinned CC0 Big Rusty Drums source.

Developer tool only. Requires Python 3, NumPy and ffmpeg on PATH. Downloads
only the selected recordings, caches the originals, and writes PCM16 WAVs
plus a source/processing manifest. The app embeds the results and works offline.
"""

import argparse
import concurrent.futures
import hashlib
import json
from pathlib import Path
import subprocess
import urllib.request
import wave

import numpy as np


REVISION = "f07ce00df34a46b6b08375be56fe116cf15782bc"
REPOSITORY = "https://github.com/sfzinstruments/karoryfer.big-rusty-drums"
BASE = f"https://raw.githubusercontent.com/sfzinstruments/karoryfer.big-rusty-drums/{REVISION}/"
ROOT = Path(__file__).resolve().parent.parent
RATE = 48_000
# (source folder, file prefix, source strike layers, mic weights, peak ceiling,
#  maximum natural tail in seconds). A single gain is applied to ALL takes of
# each piece, retaining the dynamics and variation in the actual performances.
PIECES = {
    "kick": ("kick_24/kick", "k", (3, 7, 10, 14), {"kick": 1., "oh": .15}, .72, 2.),
    "snare": ("snare_14/center", "sn_center", (2, 5, 8, 10), {"top": .65, "btm": .4, "oh": .18}, .72, 3.),
    "closed-hat": ("hihat_14/cl", "ht_cl", (1, 3, 4, 6), {"cl": .7, "oh": .3}, .38, 1.),
    "open-hat": ("hihat_14/open", "ht_open", (1, 3, 4, 6), {"cl": .7, "oh": .3}, .42, 5.),
    "ride": ("ride_22/rd", "rd", (2, 5, 8, 10), {"cl": .65, "oh": .35}, .5, 9.),
    "crash": ("crash_17/cr", "cr", (1, 2, 4, 5), {"cl": .65, "oh": .35}, .6, 9.),
    "rack-tom": ("tom_14/center", "t14", (1, 3, 4, 6), {"cl": .85, "oh": .2}, .65, 3.),
    "floor-tom": ("tom_18/center", "t18", (1, 3, 6, 8), {"cl": .85, "oh": .2}, .65, 4.5),
}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def fetch(path, cache):
    target = cache / path
    if not target.exists():
        target.parent.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(BASE + path, timeout=60) as response:
            content = response.read()
        target.write_bytes(content)
    return target


def decode(path):
    # Keep the original stereo image (mono microphones are duplicated).
    raw = subprocess.check_output([
        "ffmpeg", "-v", "error", "-i", str(path), "-f", "f32le",
        "-acodec", "pcm_f32le", "-ar", str(RATE), "-ac", "2", "-",
    ])
    audio = np.frombuffer(raw, dtype="<f4").reshape(-1, 2).astype(np.float64)
    if not len(audio) or not np.isfinite(audio).all():
        raise ValueError(f"Invalid recording: {path}")
    return audio


def prepare(audio, maximum):
    # Remove DC with a gentle 12 Hz one-pole high-pass. Its state starts at
    # zero; microphone alignment is preserved by filtering after mixing.
    # ffmpeg's standard highpass avoids a Python per-sample loop here.
    raw = subprocess.check_output([
        "ffmpeg", "-v", "error", "-f", "f64le", "-ar", str(RATE),
        "-ac", "2", "-i", "-", "-af", "highpass=f=12:p=1",
        "-f", "f64le", "-",
    ], input=audio.astype("<f8").tobytes())
    audio = np.frombuffer(raw, dtype="<f8").reshape(-1, 2).copy()
    peak = np.max(np.abs(audio))
    threshold = max(1e-5, peak * .001)
    hit = np.flatnonzero(np.max(np.abs(audio), axis=1) > threshold)
    if not len(hit):
        raise ValueError("Silent source recording")
    start = max(0, int(hit[0]) - 48)
    # Keep a little room after the last audible sample, plus a fade. Long
    # cymbals retain up to nine seconds rather than a synthesized envelope.
    audible = np.flatnonzero(np.max(np.abs(audio), axis=1) > max(1e-5, peak * .00025))
    end = min(len(audio), int(audible[-1]) + 2401, start + int(maximum * RATE))
    audio = audio[start:end].copy()
    attack = min(12, len(audio))
    audio[:attack] *= np.linspace(0., 1., attack)[:, None]
    fade = min(1440, len(audio))
    audio[-fade:] *= np.linspace(1., 0., fade)[:, None]
    return audio, start


def build(cache, output):
    paths = sorted({
        f"Samples/{folder}/{mic}/{prefix}_vl{layer}_rr{rr}.flac"
        for folder, prefix, layers, mics, _, _ in PIECES.values()
        for layer in layers for rr in range(1, 4) for mic in mics
    })
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as executor:
        list(executor.map(lambda path: fetch(path, cache), paths))
    output.mkdir(parents=True, exist_ok=True)
    (output / "LICENSE.txt").write_bytes(fetch("LICENSE", cache).read_bytes())
    manifest = {
        "source": REPOSITORY, "revision": REVISION, "license": "CC0-1.0",
        "sample_rate": RATE, "format": "stereo PCM16 WAV",
        "processing": "Mic mix preserving original relative timing, 12 Hz high-pass, onset trim with 1 ms pre-roll, 0.25 ms attack fade, 30 ms tail fade; one common gain per piece; no per-take normalization",
        "ffmpeg": subprocess.check_output(["ffmpeg", "-version"], text=True).splitlines()[0],
        "samples": [],
    }
    for piece, (folder, prefix, layers, mics, ceiling, maximum) in PIECES.items():
        recordings = []
        for layer, source_layer in enumerate(layers, 1):
            for rr in range(1, 4):
                inputs = []
                for mic, weight in mics.items():
                    path = f"Samples/{folder}/{mic}/{prefix}_vl{source_layer}_rr{rr}.flac"
                    source = cache / path
                    inputs.append((decode(source), weight, {"path": path, "weight": weight, "sha256": sha(source.read_bytes())}))
                frames = max(len(audio) for audio, _, _ in inputs)
                mix = np.zeros((frames, 2), dtype=np.float64)
                for audio, weight, _ in inputs:
                    mix[:len(audio)] += audio * weight
                audio, trimmed = prepare(mix, maximum)
                recordings.append((layer, rr, audio, trimmed, [entry for _, _, entry in inputs]))
        peak = max(float(np.max(np.abs(audio))) for _, _, audio, _, _ in recordings)
        gain = ceiling / peak
        for layer, rr, audio, trimmed, inputs in recordings:
            audio *= gain
            pcm = np.rint(audio * 32767).astype("<i2")
            file = output / f"{piece}-{layer}-{rr}.wav"
            with wave.open(str(file), "wb") as wav:
                wav.setnchannels(2)
                wav.setsampwidth(2)
                wav.setframerate(RATE)
                wav.writeframes(pcm.tobytes())
            manifest["samples"].append({
                "file": file.name, "piece": piece, "layer": layer, "round_robin": rr,
                "source_layer": layers[layer - 1], "sources": inputs,
                "trimmed_source_frames": trimmed, "piece_gain": gain,
                "frames": len(audio), "peak": float(np.max(np.abs(audio))),
                "rms": float(np.sqrt(np.mean(audio * audio))),
                "sha256": sha(file.read_bytes()),
            })
        print(f"{piece}: 12 recordings, gain {gain:.4f}, longest {max(len(a) for _, _, a, _, _ in recordings) / RATE:.2f}s", flush=True)
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Wrote {len(manifest['samples'])} recordings to {output}", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, default=Path("/tmp/peel-recorded-drum-sources"))
    parser.add_argument("--output", type=Path, default=ROOT / "assets/drums/big-rusty")
    args = parser.parse_args()
    build(args.cache, args.output)
