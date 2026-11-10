#!/usr/bin/env python3
"""Build the bundled acoustic kit from a pinned CC0 Big Rusty Drums source.

Developer tool only. Requires Python 3, NumPy, SciPy and ffmpeg on PATH.
Downloads only the selected recordings, caches the originals, mixes each piece
(EQ, compression, saturation) and writes PCM16 WAVs plus a source/processing
manifest. The app embeds the results and works offline.
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
from scipy.signal import lfilter


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


# The mix of each piece, applied to the summed microphones. EQ bands are
# (kind, Hz, dB, Q); "hp" is a 12 dB/octave high-pass and may be repeated.
# `comp` is (threshold in dB below the piece's loudest peak, ratio, attack ms,
# release ms): the slow attack lets the stick or beater through and holds the
# ring down, which is where a close-miked drum gets its punch. `drive` blends
# in a tanh stage for density; `tail` is (hold s, time constant s) of a decay
# that shortens a boomy shell. A fixed chain per piece keeps the dynamics
# between strike strengths: soft hits stay under the threshold.
MIX = {
    "kick": {
        "eq": [("hp", 36, 0, .8), ("peak", 62, 4, 1.1), ("peak", 350, -6, 1.),
               ("peak", 3500, 11, .8), ("high", 8000, 5, .7)],
        "comp": (14, 3., 18, 110), "drive": .25, "tail": (.15, .35),
    },
    "snare": {
        "eq": [("hp", 70, 0, .7), ("peak", 200, 2.5, 1.2), ("peak", 450, -3, 1.3),
               ("peak", 5000, 4, .8), ("high", 9000, 6, .7)],
        "comp": (14, 3.5, 8, 100), "drive": .2,
    },
    "closed-hat": {
        "eq": [("hp", 350, 0, .54), ("hp", 350, 0, 1.31), ("high", 8000, 3, .7)],
    },
    "open-hat": {
        "eq": [("hp", 350, 0, .54), ("hp", 350, 0, 1.31), ("peak", 560, -9, 2.5),
               ("high", 8000, 3, .7)],
    },
    "ride": {
        "eq": [("hp", 250, 0, .7), ("peak", 450, -4, 1.), ("high", 8000, 4, .7)],
    },
    "crash": {
        "eq": [("hp", 250, 0, .7), ("peak", 500, -5, 1.2), ("high", 8000, 4, .7)],
    },
    "rack-tom": {
        "eq": [("hp", 55, 0, .7), ("peak", 100, 2, 1.), ("peak", 400, -5, 1.1),
               ("peak", 4000, 7, .8)],
        "comp": (14, 3., 15, 120), "drive": .15,
    },
    "floor-tom": {
        "eq": [("hp", 35, 0, .7), ("peak", 70, 2, 1.), ("peak", 350, -5, 1.1),
               ("peak", 3500, 7, .8)],
        "comp": (14, 3., 15, 120), "drive": .15,
    },
}


def biquad(kind, hz, db, q):
    # RBJ cookbook coefficients.
    w = 2. * np.pi * hz / RATE
    cos, alpha, a = np.cos(w), np.sin(w) / (2. * q), 10. ** (db / 40.)
    if kind == "hp":
        b = [(1. + cos) / 2., -(1. + cos), (1. + cos) / 2.]
        d = [1. + alpha, -2. * cos, 1. - alpha]
    elif kind == "peak":
        b = [1. + alpha * a, -2. * cos, 1. - alpha * a]
        d = [1. + alpha / a, -2. * cos, 1. - alpha / a]
    elif kind == "high":
        r = 2. * np.sqrt(a) * alpha
        b = [a * ((a + 1.) + (a - 1.) * cos + r), -2. * a * ((a - 1.) + (a + 1.) * cos),
             a * ((a + 1.) + (a - 1.) * cos - r)]
        d = [(a + 1.) - (a - 1.) * cos + r, 2. * ((a - 1.) - (a + 1.) * cos),
             (a + 1.) - (a - 1.) * cos - r]
    else:
        raise ValueError(kind)
    return np.array(b) / d[0], np.array(d) / d[0]


def equalize(audio, piece):
    for band in MIX[piece]["eq"]:
        b, a = biquad(*band)
        audio = lfilter(b, a, audio, axis=0)
    hold, tau = MIX[piece].get("tail", (0., 0.))
    if tau:
        t = np.arange(len(audio)) / RATE - hold
        audio = audio * np.exp(-np.maximum(t, 0.) / tau)[:, None]
    return audio


def compress(audio, piece, peak):
    mix = MIX[piece]
    if "comp" in mix:
        below, ratio, attack, release = mix["comp"]
        threshold = peak * 10. ** (-below / 20.)
        up = 1. - np.exp(-1. / (attack * .001 * RATE))
        down = 1. - np.exp(-1. / (release * .001 * RATE))
        # Stereo-linked peak detector with separate attack and release.
        level = np.max(np.abs(audio), axis=1).tolist()
        envelope, e = [], 0.
        for x in level:
            e += (x - e) * (up if x > e else down)
            envelope.append(e)
        over = np.maximum(np.array(envelope) / threshold, 1.)
        audio = audio * (over ** (1. / ratio - 1.))[:, None]
    drive = mix.get("drive", 0.)
    if drive:
        # tanh at the piece's own scale: unity gain for quiet signal.
        scale = peak * .7
        audio = (1. - drive) * audio + drive * scale * np.tanh(audio / scale)
    return audio


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
        "processing": "Mic mix preserving original relative timing, 12 Hz high-pass, onset trim with 1 ms pre-roll, 0.25 ms attack fade, 30 ms tail fade; per-piece EQ, compression and saturation (see mix); one common gain per piece; no per-take normalization",
        "mix": MIX,
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
                recordings.append((layer, rr, equalize(audio, piece), trimmed, [entry for _, _, entry in inputs]))
        # The compressor threshold follows the loudest take of the piece.
        peak = max(float(np.max(np.abs(audio))) for _, _, audio, _, _ in recordings)
        recordings = [(layer, rr, compress(audio, piece, peak), trimmed, inputs)
                      for layer, rr, audio, trimmed, inputs in recordings]
        peak = max(float(np.max(np.abs(audio))) for _, _, audio, _, _ in recordings)
        gain = ceiling / peak
        for layer, rr, audio, trimmed, inputs in recordings:
            audio = audio * gain
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
