const p = peel.project;
const notes = p.patterns.reduce((total, pattern) => total + pattern.notes.length, 0);
peel.log(`${p.meta.title}: ${p.channels.length} channels, ${p.patterns.length} patterns, ${notes} notes.`);
