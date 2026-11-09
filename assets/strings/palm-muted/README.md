# Palm-muted guitar

Palm-Muted Guitar plays 133 recorded palm-muted notes from
[Guitar-TECHS v1](https://zenodo.org/records/14963133), Player 2's EVH
Wolfgang with its neck pickup. The recordings cover E2 through D6 (MIDI
40–86) chromatically. Where the player recorded a pitch on several string
positions, Peel alternates those performances. The lowest six pitches
have one recording each. One incorrect MIDI pitch label is corrected and
one unpitched strike is excluded; the manifest records both decisions.

The source is [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
[NOTICE.txt](NOTICE.txt) credits the creators and describes the changes;
[LICENSE.txt](LICENSE.txt) contains the license. Include the credit when
publishing audio that uses these recordings. Exports do not generate
`.credits.txt` files. You can copy the attribution from NOTICE.txt into your
release credits. Existing sidecars remain untouched, and imported source
attribution metadata is preserved in projects and personal templates.

The source MIDI labels locate each note; audio onset detection corrects
recording offsets. Preparation selects one DI channel, removes low DC and
reduces constant background noise, then trims noisy tails and applies
boundary fades. One gain preserves differences between performances.
Playback balances attack levels within 6 dB of the bank’s common gain.
The WAVs are mono 48 kHz PCM16 and contain the player's palm-muted attacks
and decay. Peel applies its existing amp and cabinet during playback.
The older Recorded Guitar and its Palm Mute preset keep their original
staccato recordings and settings for project compatibility.

[manifest.json](manifest.json) records source and output hashes, each MIDI
label, actual audio start, pitch, take and processing. Selected ZIP members
are verified against their CRC32 values; the published full-archive MD5 is
recorded as provenance and is not a claim of full-archive verification.

To rebuild, install NumPy and ffmpeg, then run:

```sh
python3 scripts/prepare-palm-samples.py
```

The script caches source members under `/tmp/peel-palm-sources` and fetches
only the needed ZIP ranges from the pinned Zenodo record.
