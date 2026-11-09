# PyTheory is included in packaged Peel Studio releases.
from pytheory import TonedScale

pattern = next((p for p in peel.project["patterns"] if p["id"] == peel.context["pattern"]), None)
channel = peel.context["channel"]
if pattern is None or channel is None:
    raise ValueError("Select a pattern and an instrument channel first.")

chord = TonedScale(tonic="C4")["major"].triad()
for tone in chord.tones:
    pattern["notes"].append({
        "id": peel.newId(),
        "channel": channel,
        "start": "0",
        "length": "1",
        "key": tone.midi,
        "velocity": 0.8,
    })
peel.log("Added a C major triad using PyTheory.")
