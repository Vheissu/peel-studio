# Recorded guitar and bass

These recordings come from Karoryfer Samples' CC0 libraries. The green
guitar was recorded by Brian Wood, as credited in the original readme.

- [Black And Green Guitars](https://shop.karoryfer.com/pages/free-black-and-green-guitars): the green hollowbody guitar, with sustained plucks, separately recorded staccato notes and release sounds. [Source](https://github.com/sfzinstruments/karoryfer.black-and-green-guitars), revision `b3b3249d37dc977a1a297bd2dc053e6d9b6b805c`.
- [Black And Blue Basses](https://shop.karoryfer.com/pages/free-black-and-blue-basses): the blue five-string solidbody bass played with a pick. [Source](https://github.com/sfzinstruments/karoryfer.black-and-blue-basses), revision `6e7d674cdb41be7a54dbccb15472401ad01099b9`.

Retrieved 2026-11-10. Original CC0 dedications are in
[LICENSE-guitar.txt](LICENSE-guitar.txt) and [LICENSE-bass.txt](LICENSE-bass.txt).
The recordings may be used in commercial music and redistributed.

The guitar has 16 sampled notes from E2 (MIDI 40) to C#6 (85), spaced three
semitones apart. Sustains have three plucking strengths and two alternate
takes; staccato and release sounds have two takes each. The bass has 14
sampled notes from B0 (23) to D4 (62), two strike strengths and three takes.
There are 244 recordings, about 82 MiB of mono, 48 kHz, signed 16-bit PCM.
Peel pitches nearby samples to the played note.

The original bass SFZ maps use written pitch, an octave above the sounding
recordings. Peel's map uses sounding MIDI pitch: the source `babyblue_b1`
recording has a fundamental around 31 Hz and is B0 (23), not B1 (35).

Preparation removes DC with an 8 Hz high-pass, trims unused leading
silence, and applies short boundary fades. Long guitar sustains keep their
full recorded length (up to 12 seconds); sources ending while still audible
get a gradual fade of up to 1.5 seconds before the boundary. A small tuning correction is
shared by all takes and articulations at each root, based on the median
sustained pitch. The movement within a performance stays intact. A single
gain per instrument preserves recorded dynamics and differences between
takes. Preparation adds no amp, distortion or compression; Peel applies
its amp, cabinet and drive during playback.

The muted guitar articulation uses the source's staccato performances.
Peel's Palm Mute preset further damps and darkens them for short riffs;
the source is not a dedicated modern metal palm-mute library.

The WAVs are embedded and shared by instrument instances. Projects and
personal templates need no external sample paths or downloads.
[manifest.json](manifest.json) lists each source, hashes, root key, take,
velocity layer, tuning correction, gain and output measurements.

To rebuild, install NumPy and ffmpeg and run from the repository root:

```sh
python3 scripts/prepare-string-samples.py
```

The developer tool downloads only the selected files from the pinned
revisions and caches them in `/tmp/peel-recorded-string-sources`.
