# Big Rusty Drums

Recorded by Karoryfer Samples. These sounds come from the CC0 release of
[Big Rusty Drums](https://shop.karoryfer.com/pages/free-big-rusty-drums): a
Szpaderski kick and snare, Szpaderski rack and floor toms, and Sabian hats,
ride and crash. The full library has more instruments and articulations;
Peel includes eight pieces, four strike strengths and three alternate
performances at each strength (96 recordings).

Source: [sfzinstruments/karoryfer.big-rusty-drums](https://github.com/sfzinstruments/karoryfer.big-rusty-drums),
commit `f07ce00df34a46b6b08375be56fe116cf15782bc`, retrieved 2026-11-10.
The original CC0-1.0 dedication is in [LICENSE.txt](LICENSE.txt).
These recordings may be used in commercial music and redistributed.

Close and overhead microphones are mixed together, with a bottom microphone
added for the snare wires. The original timing between microphones stays
intact. A 12 Hz high-pass removes DC; unused silence is trimmed, with short
fades at the boundaries. Cymbal tails run up to nine seconds.

Each piece is then mixed the way a drum channel is: EQ on all of them,
and compression and light saturation on the kick, snare and toms. The kick
loses some of its lowest boom and box and gains beater attack, and its tail
is shortened. The snare and toms get more stick and less box. The cymbals
are high-passed and brightened, and the open hat's ring near 560 Hz is cut.
The compressor's threshold is set from the loudest take of a piece, so soft
strikes pass untouched and the layers keep their order. One common gain per
piece preserves the differences between takes. There is no added reverb.
The exact settings are the `MIX` table in the build script and are copied
into the manifest.

The WAVs are stereo, 48 kHz, signed 16-bit PCM (about 71 MiB altogether).
They are embedded in the app and shared by all Acoustic Kit instances.
Headers and velocity levels are read once on a control thread; voices read
the PCM directly. They require no download or external files.

[manifest.json](manifest.json) lists the mix settings and, for every
recording, the source microphone files, original velocity layer and take,
source and output SHA-256, mic weight, trim, gain, frame count, peak and RMS.

To rebuild, install NumPy, SciPy and ffmpeg, then run from the repository root:

```sh
python3 scripts/prepare-drum-samples.py
```

The tool downloads just the selected recordings from the pinned revision,
caches the originals in `/tmp/peel-recorded-drum-sources`, and rebuilds the
WAVs and manifest. It is a developer tool; the app never runs it.
