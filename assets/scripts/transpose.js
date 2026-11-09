// Transpose selected notes, or every note on the current channel if none are selected.
const selected = new Set(peel.context.notes);
let count = 0;
for (const pattern of peel.project.patterns) {
    if (pattern.id !== peel.context.pattern) continue;
    for (const note of pattern.notes) {
        if (selected.size ? !selected.has(note.id) : note.channel !== peel.context.channel) continue;
        note.key = Math.min(127, note.key + 12);
        count++;
    }
}
peel.log(`Transposed ${count} notes up an octave.`);
