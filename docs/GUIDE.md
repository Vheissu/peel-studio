# Guide

How the main parts of Peel Studio work. Mixing, EQ, mastering,
instruments, MIDI and recording are covered in the
[audio tools guide](AUDIO_TOOLS.md).

## Projects

File > New project starts blank, with no instruments or notes. File > New
from template offers Rap, Trap, Hip-hop, Synthwave, EDM and Rock / Metal
song starters, alongside House and Beat and melody. Rock / Metal opens
Channels on a one-bar Double kick pattern with the step grid at 1/16T, so
the six-kick bursts on beats one and three line up with the steps. Trap
uses the 808 instrument and slides it up an octave at the end of each hook
bar; Synthwave and Hip-hop use the gated and dusty drum presets.

The demo songs are a separate choice: the start screen lists them with
their style, tempo and key, and File > Demo songs opens the same list.
File > Close project returns to the start screen. New, Open and Close ask
before replacing unsaved work; cancelling Save As keeps the current project.

### Templates

File > Save as template… stores a copy of the open project under a name you
choose: instruments and their settings, plugin state, mixer routing,
patterns, recorded expression and the arrangement. Audio comes along too,
including files the project only referenced, so the template still works if
the original project or those files move. Saving a template leaves the open
project as it was, with the same file, title and unsaved changes. Names must
be unique; an existing template is never replaced.

Your templates are listed under File > New from template > My templates and
behind the My templates button on the start screen. Picking one starts a new
Untitled project. The first save asks where to put it and copies its audio
there, so the template itself is never changed. Templates live in a
`Templates` folder next to the app's settings (on macOS,
`~/Library/Application Support/Peel Studio/Templates`). There's no rename or
delete in the app yet; remove a template's `.peelproj` file and `.peeldata`
folder by hand.

### Recorded instruments

Acoustic Kit plays real recordings of kick, snare, closed/open hats, ride,
crash and toms, with four velocity layers and three alternate takes per
piece. Choose its Full Kit preset for a GM drum keyboard, or a piece preset
for a Channel Rack row. New Rock / Metal songs use the recorded kit.
They also use Recorded Guitar for sustained power chords, Palm-Muted Guitar
for chugs and Recorded Bass for a picked bass line. These instruments use
bundled multisamples, with alternate takes, velocity response and no
external sample files. The guitars include clean and amplified sounds.
Palm-Muted Guitar uses dedicated palm-muted recordings from Guitar-TECHS
under CC BY 4.0. Exports do not generate credit files; the licence still
requires attribution when publishing audio that uses these recordings. See
[sample credits](../assets/strings/palm-muted/README.md).
Kick, Snare, Hat and Tom also offer modeled acoustic and 808/909 circuit
sounds. Existing projects and older presets retain their sound.

### Kits

In Channels, the Add menu lists kits above the instruments. Trap kit adds
a kick, snare, clap, closed and open hats and an 808 set to slide; Acoustic
kit adds six pieces of the recorded kit. Each piece gets its own row and
mixer track, and the whole kit is one undo step.

### Grids

Step keys are colored by beat, four to a color, so the bar reads at a
glance. **Steps** sets the drum grid; the bar count sets the pattern's
length. Choose 1/32 or 1/64 for fast repeated kicks. The Piano Roll has
separate **Grid** and **Note length** controls, with quarter notes, eighths,
sixteenths, finer divisions and triplets. Note length also offers dotted
values. Changing the grid leaves existing notes untouched. Cmd/Ctrl+wheel
zooms the Piano Roll; Shift+wheel scrolls a long step grid sideways.

## 808

The 808 instrument is a tuned sub bass: a sine that drops onto its note for
the punch, holds, then decays, through a drive stage and a tone filter. It
plays one note at a time. With Glide on Legato, a note that starts while
another is still held slides to the new pitch instead of restarting, so a
slide is two overlapping notes in the Piano Roll: lengthen the first until
it runs under the second. Always also slides between separate notes; Off
restarts every note. Glide Time is the same for any interval. The Piano
Roll draws a line from one note to the next wherever a slide will happen.

Note Off chooses Release (the key gates the note; a short Release chokes
it) or Ring (the decay always runs out, like a one-shot sample). Decay
Track shortens higher notes. Drive offers Warm, Tube, Clip and Fold;
Shape and Octave add harmonics before it so the bass reads on small
speakers. Fourteen presets run from Clean Sub to Fuzz. The Trap starter
and Night Shift use Slide.

## Presets

Hat, Snare, Clap, Kick, Bass, Poly, Cross, Mallet and Tine have presets
for trap, boom bap and synthwave: Trap Tight and Trap Open hats (Open is
cut by the note's length), Dusty drums, Rim Click and Finger Snap, a Gated
snare and clap, Trap Bell, Flute Lead, Synth Brass, PWM Pad, Night Lead,
Cowbell and more. Reverb, Delay, Compressor and Mastering have presets to
match, such as Snare Plate, Neon Hall, Worn Echo and 808 Duck.

## Tape and Gate

**Tape** adds saturation, a head bump, a high cut, wow, flutter and hiss
that only sounds under a signal. **Gate** mutes or ducks a track below a
threshold, with hold, hysteresis and a sidechain: put it after a reverb on
a return and key it from the dry snare for a gated reverb.

## Clip editing

Right-click an arrangement clip for Duplicate, Copy or Cut. Use the Select
tool or Shift-click to work with a group. Cmd/Ctrl+D repeats the selection
with its timing and lane spacing intact.

Right-click an empty lane to paste at that grid position, or point at the
destination and press Cmd/Ctrl+V. Cmd/Ctrl+A selects every arrangement clip.
Copies share their source pattern; use Make unique to edit one independently.

## Export

Open Export from the toolbar or press Cmd/Ctrl+E. Choose a mix or one file
per mixer track, then the format and quality:

| Format | Quality |
| --- | --- |
| WAV | 16-bit or 24-bit PCM, or 32-bit float |
| MP3 | 128, 192, 256 or 320 kbps constant bitrate; 44.1 or 48 kHz |
| FLAC | Lossless 16-bit or 24-bit PCM; up to 96 kHz |
| AIFF | Uncompressed 16-bit or 24-bit PCM |

WAV and AIFF offer sample rates from 44.1 to 192 kHz; FLAC supports up to
96 kHz. Stereo or mono, effect tails and optional normalization are
available for every format.
Normalization adjusts each rendered file independently to -1 dBFS peak;
it isn't a loudness target. Leave it off for stems whose relative levels
you want to preserve. MP3 encoding can shift the decoded peak slightly.
TPDF dither can be enabled for integer PCM output.

The headless renderer chooses the format from the output extension and
accepts `--rate`, `--bits`, `--bitrate`, `--tail`, `--mono`, `--normalize`
and `--no-dither`. Existing WAV render commands still work.
