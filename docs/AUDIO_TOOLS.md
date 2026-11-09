# Audio tools

## Mixer

Press **F9** to open the mixer. Each mixer track has stereo metering,
a level fader, pan, mute and solo. The master stays on the right while
tracks scroll. Insert effects process the track before its output routing
and post-fader sends. Send targets and output buses use the same mixer
routing stored in the project.

Click an insert to open its editor. The chain can contain more than four
effects; bypass, reorder or remove inserts from their menus. Mastering
is available as a native effect and from the master strip.

## EQ

The EQ has a low shelf, two parametric bells, a high shelf and low/high
cuts. Drag a band node to set frequency and gain; adjust Q to change its
width. The response curve shows the actual filter response. Factory
presets include Flat, Clean Low End, Vocal Presence, Air and Telephone.

The analyzer reads the selected EQ's input and output, rather than the
final master signal. Its spectra use dBFS; the filter response uses dB.
The waveform strip shows recent output peaks. Analysis runs locally,
with the FFT on the UI thread and a bounded audio tap in the callback.
Stereo power preserves out-of-phase material that a mono sum would hide.

## Resonances

Listen to a representative passage before reviewing resonance candidates.
The detector looks for narrow peaks that remain above their nearby
frequencies. It suppresses silence and broadband noise and proposes
conservative cuts, up to 6 dB.

These are candidates to audition. A sustained musical fundamental can
qualify, and a prominent frequency may belong in the mix. Applying a cut
uses one of the two bell bands; review the target band before replacing
an existing setting. Reset and listen again after changing the source.

## Mastering

The Mastering effect combines input trim, broad tone EQ, linked bus
compression, stereo width and a lookahead limiter. Simple and Advanced
views edit the same parameters; switching views does not change the sound.
Clean, Balanced, Warm, Punch and Loud presets provide starting points.

Low and Air are shelves at 120 Hz and 8 kHz; Body is a broad bell at
500 Hz. Compression uses a soft knee and an 80 Hz detector filter.
The limiter defaults to a -1 dB ceiling with true-peak detection enabled.
Its fixed lookahead is about 4 ms. Plugin delay compensation aligns
parallel instrument, audio, bus and send paths, including sidechain keys
at their receiving insert. Offline mixes and stems discard the compensated
preroll so their requested start and length stay intact.

The ceiling applies at the effect's output. Keep the master fader at 0 dB
and place Mastering last in the chain to preserve it in the exported mix.
Later effects or positive master-fader gain can exceed it. Metronome,
preview and direct input monitoring are added after the master inserts.

Peak and RMS readouts are dBFS, not LUFS. The limiter's true-peak detector
uses the existing oversampled native limiter; the visible sample-peak
readout is not a separate standards-compliant true-peak measurement.

## Instruments

Select an instrument channel to play it from the computer keyboard or a
MIDI controller. The computer keyboard maps Z–M and Q–P to notes;
minus and equals change octave. Open **Instrument** from a channel menu or the piano-keys button in the
inspector. The editor provides parameter controls, presets and a mouse
keyboard for auditioning sounds.

Native instruments use Peel's controls. CLAP instruments expose their
parameter controls and saved plugin state; embedded third-party plugin
GUIs are not supported by this host yet.

## MIDI

Choose an input port and optional MIDI channel filter in Audio and MIDI.
Hardware keyboards and software MIDI sources can play the live target.
Sustain and note release follow the instrument that received the note,
so changing selection does not leave notes held on the old instrument.

Use Learn to bind a controller to a device parameter, channel or mixer
fader/pan control. Turn the hardware knob once, then move it to control
the target. Logarithmic frequency and time parameters retain their
normal response curve. CC64 keeps its sustain role; CC120 and CC123
remain panic and all-notes-off controls. Pitch bend, channel aftertouch and
polyphonic aftertouch reach the selected instrument, including held and
sustained notes. The default bend range is two semitones; choose 0–48 in
the instrument editor, alongside its pitch and pressure controls.

Native pressure adds up to about 4 dB of gain and, on filtered instruments,
up to three quarters of an octave of brightness. Per-key pressure affects
only that key; channel pressure affects all voices. CLAP instruments receive
the expression supported by their note dialect. MIDI-only plugins need
RPN 0 pitch sensitivity support, or a plugin bend range that matches Peel.
Expression changes performance state rather than saved instrument parameters.
Pattern recording captures pitch, pressure and timbre for each note.

A channel-specific controller binding overrides an omni binding on that
channel while preserving the omni binding for other channels.

Bindings are machine preferences and reference project object IDs.
Relearn or remove a binding if its target no longer exists. Controller
moves are grouped into undo gestures rather than adding one undo step
per MIDI message.

## MPE

Open an instrument and choose Lower or Upper in its MIDI controls. Lower
uses channel 1 as the zone manager; Upper uses channel 16. Members chooses
the adjacent note channels, up to 15. One zone feeds the selected instrument.
Ordinary channel filtering applies again when MPE is Off.

Note bend defaults to 48 semitones and Zone bend to 2. Member pitch bend,
channel pressure and CC74 control that member's notes independently, even
when another member plays the same pitch. Manager bend adds to note bend;
manager pressure, slide and sustain apply across the zone. Enabled zones
accept RPN 6 zone configuration and RPN 0 bend sensitivity. Configuration
is a machine preference; saved note curves play without an MPE controller.

Native instruments and CLAP note-expression plugins receive per-voice
pitch, pressure and timbre. Monophonic instruments retain their usual
voice behavior. MIDI-only CLAP instruments need MPE and RPN support; the
host provides 15 note channels with a 96-semitone output bend range.
Larger combined displacements are limited to that range for MIDI output;
native and CLAP note-expression paths support up to 120 semitones.

## Routing

Recording input and primary playback output can use different devices and
sample rates. Live monitoring is resampled with bounded clock correction;
recorded WAV files keep the input's original sample rate and samples.
Choose the input's mono channel or stereo pair and the main output's
mono channel or stereo pair independently.

Additional output routes can send the master or a post-fader mixer bus
to up to four more hardware destinations. A route has its own device,
channel mapping, gain and mute. Multiple routes on one device must use
different output channels. Invalid changes and failed device openings
leave the current routes intact.

Separate interfaces have independent clocks. Peel buffers and resamples
these cue/monitor feeds to correct drift; it does not make them
phase-aligned. Use an OS aggregate device when synchronized multichannel
output is required. Route status reports stream health, buffering and
underruns/overruns. Reapply a route after reconnecting a missing device.

The master route carries the mix before metronome, preview and direct
input monitoring. A bus route carries that bus after inserts and its
fader, pan and mute. Hardware choices belong to this computer and do
not travel in project files. Additional outputs require a real primary audio
output; silent command-line hosts keep hardware output closed.

## Delay compensation

Peel uses the latency reported by each device after preparation. Bypass,
insert changes and reported latency changes rebuild compensation on the
control thread. A CLAP restart request briefly suspends that instance while
it is prepared again, then recalculates its compensation. Stop, seek and
panic clear delayed audio; normal loop wraps
keep continuous history so paths remain aligned even in short loops. The
metronome and recorded take placement account for the graph delay. Direct
input monitoring remains a low-latency feed after the master inserts.

Compensation supports up to 262,144 samples per path and 64 MiB of delay
storage. An unsupported graph reports an error and keeps the previous
compensation. The Audio panel shows the active graph delay. Separate
hardware outputs still have independent clocks as described above.

## Recording

Open **Audio** in the status bar. Choose a microphone/interface input
and the mono channel used by the microphone. Check the input meter, arm an arrangement lane and start a
take in Song mode. Monitoring is optional; headphones avoid feeding
speaker output back into a live microphone.

Stopping a take closes the WAV file, imports it into the project and
places audio clips on the armed lanes with latency compensation. The
audio file is written during recording, rather than kept only in memory.
A manual recording offset can correct an interface's reported latency.
In Pattern mode, Record captures accepted hardware, computer-keyboard,
mouse-piano and scripted notes through the same performance feed. Pitch
bend, channel/polyphonic aftertouch and MPE slide are saved with each note,
including the state when the note starts. Sustain extends the recorded
note until the pedal lifts. A note held across a pattern boundary becomes
separate notes with the carried expression at the next loop's start.

Stopping or disarming closes held notes. Changing the tempo ends the take
and disarms recording. One undo removes the take and its
curves. The Piano Roll's lower lane switches between Velocity, Pitch,
Pressure and Timbre. Select a note to inspect or edit its control points;
Clear removes the chosen curve. Values hold until the next point, with the
instrument's usual smoothing. Curves end at the note's gate; its release
tail keeps the final recorded state.

Projects containing note expression require the updated project reader.
Their manifest uses schema 2 so older Peel versions refuse to open or
overwrite them instead of losing the curves.

## Scripts

All new native effect parameters and presets appear through
`peel.call("devices")`. Use `add_effect` and `set_device` to build and
adjust a mastering chain:

```js
const master = peel.project.mixer.tracks[0].id;
const effect = peel.call("add_effect", {
  track: master, type_id: "peel.fx.mastering"
});
peel.call("set_device", {
  device: effect.device,
  preset: "Balanced",
  params: {ceiling: -1.0}
});
```

`output_routes`, `configure_output_routes` and
`configure_output_channels` expose hardware routing. `midi_controls`,
`configure_midi_controls`, `midi_learn`, `midi_control`, `midi_expression`,
`mpe` and `midi_input` expose
controller setup and internal events. See [MCP](MCP.md) for arguments;
scripts use the same action names without the `peel_` prefix.

Use `note_expression` to read or edit a note's saved curves. Desktop scripts
can arm Pattern recording with `peel.call("ui.record", {armed: true})`.
