#!/usr/bin/env python3
"""Extract genuine palm-muted DI recordings from Guitar-TECHS (CC BY 4.0).

Developer tool: Python 3, NumPy and ffmpeg. Fetches only the MIDI and DI
members with HTTP ranges, verifies their ZIP CRCs, and records SHA-256s.
"""

import argparse
import concurrent.futures
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import urllib.request
import wave
import zlib

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
URL = "https://zenodo.org/api/records/14963133/files/P2_techniques.zip/content"
SIZE = 395_839_610
ARCHIVE_MD5 = "f4189251ce50be25f06a173b2c2bba00"
RATE = 48_000


def sha(data):
    return hashlib.sha256(data).hexdigest()


def get_range(start, end):
    request = urllib.request.Request(URL, headers={"Range": f"bytes={start}-{end}"})
    with urllib.request.urlopen(request, timeout=120) as response:
        expected = f"bytes {start}-{end}/{SIZE}"
        if response.status != 206 or response.headers.get("Content-Range") != expected:
            raise ValueError("Server did not return the requested ZIP range")
        data = response.read()
    if len(data) != end - start + 1:
        raise ValueError("Incomplete ZIP range")
    return data


def entries(cache):
    file = cache / "zip-tail.bin"
    if not file.exists():
        file.write_bytes(get_range(SIZE - 65_536, SIZE - 1))
    tail = file.read_bytes()
    result = []
    position = 0
    while (position := tail.find(b"PK\x01\x02", position)) >= 0:
        header = struct.unpack_from("<4s6H3I5H2I", tail, position)
        name_len, extra_len, comment_len = header[10:13]
        name = tail[position + 46:position + 46 + name_len].decode()
        result.append({"name": name, "method": header[4], "crc32": header[7],
                       "compressed": header[8], "size": header[9], "offset": header[-1]})
        position += 46 + name_len + extra_len + comment_len
    if not result:
        raise ValueError("Missing ZIP central directory")
    eocd = tail.rfind(b"PK\x05\x06")
    count, _, directory = struct.unpack_from("<HII", tail, eocd + 10)
    if count != len(result):
        raise ValueError("Incomplete ZIP central directory")
    for i, entry in enumerate(result):
        entry["end"] = result[i+1]["offset"] if i+1 < len(result) else directory
    return result


def member(entry, cache):
    target = cache / Path(entry["name"]).name
    if target.exists():
        data = target.read_bytes()
        if len(data) == entry["size"] and zlib.crc32(data) == entry["crc32"]:
            return target
    if entry["method"] != 8:
        raise ValueError("Expected a deflated ZIP member")
    start, stop = entry["offset"], entry["end"]
    step = max(1, (stop - start + 7) // 8)
    chunks = [(a, min(a+step, stop)-1) for a in range(start, stop, step)]

    def fetch_chunk(pair):
        a, b = pair
        path = cache / f"range-{a}-{b}.bin"
        if not path.exists() or path.stat().st_size != b-a+1:
            path.write_bytes(get_range(a, b))
        return path.read_bytes()

    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as executor:
        compressed = b"".join(executor.map(fetch_chunk, chunks))
    if compressed[:4] != b"PK\x03\x04":
        raise ValueError("Missing local ZIP header")
    n, e = struct.unpack_from("<HH", compressed, 26)
    offset = 30 + n + e
    data = zlib.decompress(compressed[offset:offset + entry["compressed"]], -15)
    if len(data) != entry["size"] or zlib.crc32(data) != entry["crc32"]:
        raise ValueError("ZIP member integrity check failed")
    target.write_bytes(data)
    print(f"Verified {entry['name']}: {len(data):,} bytes", flush=True)
    return target


def variable(data, position):
    value = 0
    for _ in range(4):
        byte = data[position]
        position += 1
        value = (value << 7) | (byte & 127)
        if byte < 128:
            return value, position
    raise ValueError("Invalid MIDI variable length")


def midi_notes(data):
    if data[:4] != b"MThd":
        raise ValueError("Missing MIDI header")
    header_len = int.from_bytes(data[4:8], "big")
    fmt, tracks, division = struct.unpack_from(">HHH", data, 8)
    if fmt not in (0, 1) or division & 0x8000:
        raise ValueError("Unsupported MIDI timing")
    position = 8 + header_len
    events = []
    for track in range(tracks):
        if data[position:position+4] != b"MTrk":
            raise ValueError("Missing MIDI track")
        length = int.from_bytes(data[position+4:position+8], "big")
        end = position + 8 + length
        position += 8
        tick, status = 0, None
        while position < end:
            delta, position = variable(data, position)
            tick += delta
            if data[position] & 128:
                status = data[position]
                position += 1
            if status == 255:
                kind = data[position]
                length, position = variable(data, position + 1)
                payload = data[position:position+length]
                position += length
                if kind == 81:
                    events.append((tick, "tempo", int.from_bytes(payload, "big"), track, 0))
            elif status in (240, 247):
                length, position = variable(data, position)
                position += length
            elif status is not None:
                kind, channel = status >> 4, status & 15
                size = 1 if kind in (12, 13) else 2
                payload = data[position:position+size]
                position += size
                if kind in (8, 9):
                    on = kind == 9 and payload[1] > 0
                    events.append((tick, "on" if on else "off", payload[0], channel, payload[1]))
            else:
                raise ValueError("Missing MIDI running status")
    events.sort(key=lambda event: event[0])
    tempo, previous, seconds = 500_000, 0, 0.
    active, notes = {}, []
    for tick, kind, key, channel, velocity in events:
        seconds += (tick - previous) * tempo / (division * 1_000_000)
        previous = tick
        if kind == "tempo":
            tempo = key
        elif kind == "on":
            active[channel, key] = (seconds, velocity)
        elif (channel, key) in active:
            start, vel = active.pop((channel, key))
            notes.append({"key": key, "channel": channel, "start": start,
                          "duration": seconds-start, "velocity": vel})
    return sorted(notes, key=lambda note: note["start"])


def acquire(cache):
    cache.mkdir(parents=True, exist_ok=True)
    index = entries(cache)
    midi_entry = next(e for e in index if e["name"].endswith("midi_PalmMute.mid"))
    audio_entry = next(e for e in index if e["name"].endswith("directinput_PalmMute.wav"))
    midi = member(midi_entry, cache)
    notes = midi_notes(midi.read_bytes())
    (cache / "notes.json").write_text(json.dumps(notes, indent=2) + "\n")
    print(f"MIDI: {len(notes)} notes, keys {sorted({n['key'] for n in notes})}", flush=True)
    print(json.dumps(notes[:5]), flush=True)
    audio = member(audio_entry, cache)
    return audio, notes


# Source labels the audio contradicts. Each entry is verified by the pitch
# check below on every build: the recorded label must fail and the
# correction must pass, or preparation stops.
LABEL_CORRECTIONS = {
    125: {"midi_label_key": 79, "root_key": 78,
          "reason": "Recorded pitch is F#5: harmonics match key 78, not the labelled 79. "
                    "Channel 0 has no other 78 label; the next event (126) is the real 79."},
}
# Source events left out. Each must also fail the pitch check.
EXCLUSIONS = {
    64: {"midi_label_key": 70,
         "reason": "Dead palm mute: mostly pick noise and the ringing low E string. Its weak "
                   "pitched content does not support key 70 (the harmonic check favours 71)."},
}

BLOCK = 96                 # 2 ms onset blocks
ENV = 240                  # 5 ms envelope blocks
MAX_SECONDS = 2.0
PREROLL = 48               # 1 ms before the attack, faded in
FADE_LEVEL_DB = -30.0      # tail fade starts once the envelope has decayed this far
END_LEVEL_DB = -40.0      # the fade ends where the note is this far down or in the noise
MIN_FADE = round(0.05 * RATE)
PITCH_MARGIN = 1.15        # root harmonic score against the best neighbouring semitone


def attack_onset(audio, label):
    """The main pick attack near a MIDI label, as a sample index.

    Guitar-TECHS labels can be up to 100 ms off, and the search window can
    contain isolated clicks before the pick. Find the loudest 2 ms block, then
    walk back while the signal stays above 10 % of it; 6 ms of quiet ends the
    attack, so an earlier click is never mistaken for it.
    """
    begin = max(0, round((label - .25) * RATE))
    finish = min(len(audio), round((label + .3) * RATE))
    search = audio[begin:finish]
    count = len(search) // BLOCK
    rms = np.sqrt(np.mean(search[:count * BLOCK].reshape(-1, BLOCK) ** 2, axis=1))
    peak = int(np.argmax(rms))
    threshold = rms[peak] * .1
    first, quiet = peak, 0
    while first > 0 and quiet < 3:
        quiet = quiet + 1 if rms[first - 1] < threshold else 0
        first -= 1
    first += quiet
    lo = begin + max(0, first - 1) * BLOCK
    hi = begin + (peak + 1) * BLOCK
    region = np.abs(audio[lo:hi])
    return lo + int(np.flatnonzero(region >= .15 * region.max())[0])


def harmonic_score(spectrum, freqs, f0):
    total = 0.
    for k in range(1, 13):
        fk = k * f0
        if fk > 5000:
            break
        band = (freqs >= fk * 2 ** (-25 / 1200)) & (freqs <= fk * 2 ** (25 / 1200))
        total += float(spectrum[band].max())
    return total


def pitch_check(take, key):
    """Compare harmonic evidence for `key` against the two semitones either side.

    Sums spectral peaks at the first harmonics (up to 5 kHz) shortly after
    the attack, so a weak fundamental with clear upper harmonics still counts;
    nothing is required of the fundamental alone. Returns the best offset in
    semitones, the margin of `key` over its best neighbour and the fine
    offset in cents.
    """
    f0 = 440 * 2 ** ((key - 69) / 12)
    start = PREROLL + round(.005 * RATE)
    length = int(min(len(take) - start, max(.08, 16 / f0) * RATE))
    segment = take[start:start + length]
    spectrum = np.abs(np.fft.rfft(segment * np.hanning(len(segment)), 1 << 17))
    freqs = np.fft.rfftfreq(1 << 17, 1 / RATE)
    scores = {d: harmonic_score(spectrum, freqs, f0 * 2 ** (d / 12)) for d in (-2, -1, 0, 1, 2)}
    best = max(scores, key=scores.get)
    neighbour = max(v for d, v in scores.items() if d)
    cents = max(range(-50, 51, 2), key=lambda c: harmonic_score(spectrum, freqs, f0 * 2 ** (c / 1200)))
    return best, scores[0] / max(neighbour, 1e-12), cents


def envelope(x):
    count = len(x) // ENV
    return np.sqrt(np.mean(x[:count * ENV].reshape(-1, ENV) ** 2, axis=1))


def shape_tail(take, floor):
    """Keep the recorded body and decay; fade only once it has died away.

    The fade starts after the last 5 ms block above -30 dB from the envelope
    peak (or above 2.5x the background noise, if that is louder), and ends
    where the note has decayed to -40 dB or into the noise, with at least
    50 ms of raised-cosine fade. Returns the take and its fade start level.
    """
    env = envelope(take)
    peak_block = int(np.argmax(env))
    top = float(env[peak_block])
    fade_level = max(top * 10 ** (FADE_LEVEL_DB / 20), floor * 2.5)
    end_level = max(top * 10 ** (END_LEVEL_DB / 20), floor * 1.5)
    above = np.flatnonzero(env[peak_block:] > fade_level)
    fade_start = (peak_block + int(above[-1]) + 1) * ENV
    above = np.flatnonzero(env[peak_block:] > end_level)
    end = (peak_block + int(above[-1]) + 1) * ENV
    end = min(len(take), max(end, fade_start + MIN_FADE))
    fade_start = min(fade_start, end - MIN_FADE)
    take = take[:end].copy()
    n = end - fade_start
    take[fade_start:] *= .5 * (1 + np.cos(np.pi * np.arange(n) / n))
    take[:PREROLL] *= np.linspace(0., 1., PREROLL)
    level = env[min(fade_start // ENV, len(env) - 1)]
    return take, 20 * np.log10(max(float(level), 1e-12) / top)


def build(cache, output):
    source, notes = acquire(cache)
    # The DI file is dual mono. Use the first channel, remove subsonic DC
    # and reduce its measured constant background hiss by 12 dB. No new
    # harmonics, substitute attacks or synthesized mute envelopes are added.
    raw = subprocess.check_output([
        "ffmpeg", "-v", "error", "-i", str(source), "-af",
        "pan=mono|c0=c0,highpass=f=20:p=1,afftdn=nr=12:nf=-60",
        "-ar", str(RATE), "-f", "f32le", "-",
    ])
    audio = np.frombuffer(raw, "<f4").astype(np.float64)
    prepared, corrections, excluded = [], [], []
    for index, note in enumerate(notes):
        label = note["key"]
        onset = attack_onset(audio, note["start"])
        start = max(0, onset - PREROLL)
        raw_take = audio[start:min(len(audio), start + round(MAX_SECONDS * RATE))].copy()
        label_check = pitch_check(raw_take, label)
        if index in EXCLUSIONS:
            rule = EXCLUSIONS[index]
            if rule["midi_label_key"] != label or label_check[0] == 0 and label_check[1] >= PITCH_MARGIN:
                raise ValueError(f"Exclusion of MIDI event {index} no longer matches the source")
            excluded.append({
                "midi_event": index, "midi_channel": note["channel"],
                "midi_start_seconds": note["start"], "midi_label_key": label,
                "reason": rule["reason"],
                "pitch_check": {"best_offset_semitones": label_check[0],
                                "label_margin": round(label_check[1], 3)},
            })
            continue
        root = label
        if index in LABEL_CORRECTIONS:
            rule = LABEL_CORRECTIONS[index]
            if rule["midi_label_key"] != label or label_check[0] != rule["root_key"] - label:
                raise ValueError(f"Label correction of MIDI event {index} no longer matches the source")
            root = rule["root_key"]
            corrections.append({
                "midi_event": index, "midi_channel": note["channel"],
                "midi_start_seconds": note["start"], "midi_label_key": label,
                "root_key": root, "reason": rule["reason"],
                "pitch_check_at_label": {"best_offset_semitones": label_check[0],
                                         "label_margin": round(label_check[1], 3)},
            })
        best, margin, cents = pitch_check(raw_take, root)
        if best != 0 or margin < PITCH_MARGIN or abs(cents) > 45:
            raise ValueError(f"MIDI event {index} does not sound like key {root}: "
                             f"best {best:+d}, margin {margin:.2f}, {cents:+d} cents")
        noise_start = min(len(audio), round((note["start"] + 2.7) * RATE))
        noise_end = min(len(audio), round((note["start"] + 3.5) * RATE))
        noise = audio[noise_start:noise_end]
        floor = float(np.sqrt(np.mean(noise * noise))) if len(noise) else 0.
        take, fade_db = shape_tail(raw_take, floor)
        if np.max(np.abs(take)) < .001:
            raise ValueError(f"No usable palm mute for MIDI event {index}")
        prepared.append((note, root, take, start, floor, index, margin, cents, fade_db))
    gain = .65 / max(float(np.max(np.abs(p[2]))) for p in prepared)
    output.mkdir(parents=True, exist_ok=True)
    manifest = {
        "source": "https://zenodo.org/records/14963133", "dataset": "Guitar-TECHS", "version": "v1",
        "license": "CC-BY-4.0", "license_url": "https://creativecommons.org/licenses/by/4.0/",
        "archive": "P2_techniques.zip", "archive_md5_published": ARCHIVE_MD5,
        "integrity": "Selected ZIP members verified against central-directory CRC32; SHA-256 below. Whole archive MD5 is published provenance, not a local full-archive verification.",
        "audio_member": "P2_techniques/audio/directinput/directinput_PalmMute.wav",
        "audio_sha256": sha(source.read_bytes()), "midi_sha256": sha((cache / "midi_PalmMute.mid").read_bytes()),
        "performer": "Player 2", "guitar": "EVH Wolfgang", "pickup": "Neck",
        "format": "mono 48000Hz PCM16 WAV",
        "processing": "First DI channel, 20Hz high-pass, 12dB background noise reduction; each take starts 1 ms before the main pick attack found near its MIDI label (walking back from the loudest block, so earlier clicks are skipped); the recorded body and decay are kept and a raised-cosine fade of at least 50 ms starts only once the envelope is 30 dB down or near the background noise; one common gain; no synthesized harmonics or substitute staccato attacks",
        "quality_checks": "Every included take passes a harmonic pitch check at its root_key: harmonic peaks up to 5 kHz score at least 1.15x the best of the two semitones either side, within 45 cents. Label corrections and exclusions are re-verified against the source on every build.",
        "source_midi_events": len(notes),
        "included": len(prepared),
        "label_corrections": corrections,
        "excluded": excluded,
        "samples": [],
    }
    counts = {}
    for note, root, take, start, floor, index, margin, cents, fade_db in prepared:
        counts[root] = counts.get(root, 0) + 1
        number = counts[root]
        take = take * gain
        pcm = np.rint(take * 32767).astype("<i2")
        file = output / f"palm-{root}-{number}.wav"
        with wave.open(str(file), "wb") as wav:
            wav.setnchannels(1); wav.setsampwidth(2); wav.setframerate(RATE)
            wav.writeframes(pcm.tobytes())
        manifest["samples"].append({
            "file": file.name, "root_key": root, "take": number,
            "midi_event": index, "midi_channel": note["channel"], "midi_label_key": note["key"],
            "label_corrected": root != note["key"], "midi_start_seconds": note["start"],
            "audio_start_seconds": start / RATE, "midi_velocity": note["velocity"],
            "frames": len(take), "gain": gain, "background_rms": floor,
            "peak": float(np.max(np.abs(take))), "rms": float(np.sqrt(np.mean(take * take))),
            "pitch_margin": round(margin, 3), "pitch_cents": cents,
            "fade_start_db": round(float(fade_db), 1),
            "sha256": sha(file.read_bytes()),
        })
    keep = {s["file"] for s in manifest["samples"]}
    for stale in sorted(output.glob("palm-*.wav")):
        if stale.name not in keep:
            stale.unlink()
            print(f"Removed stale {stale.name}", flush=True)
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    lines = ["// Generated by scripts/prepare-palm-samples.py. Original Guitar-TECHS DI takes.",
             "static PALM_FILES: &[(u8, &[&[u8]])] = &["]
    for root, count in sorted(counts.items()):
        lines.append(f"    ({root}, &[")
        for number in range(1, count + 1):
            lines.append(f'        include_bytes!("../../../assets/strings/palm-muted/palm-{root}-{number}.wav"),')
        lines.append("    ]),")
    lines.append("];\n")
    (ROOT / "crates/peel-instruments/src/palm_assets.rs").write_text("\n".join(lines))
    print(f"Prepared {len(prepared)} genuine palm-muted performances at {len(counts)} pitches "
          f"({len(corrections)} label corrected, {len(excluded)} excluded)", flush=True)
    print(f"Take counts: {sorted(counts.items())}", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, default=Path("/tmp/peel-palm-sources"))
    parser.add_argument("--output", type=Path, default=ROOT / "assets/strings/palm-muted")
    args = parser.parse_args()
    build(args.cache, args.output)
