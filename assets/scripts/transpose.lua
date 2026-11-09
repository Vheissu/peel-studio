-- Transpose selected notes, or every note on the current channel if none are selected.
local selected = {}
for _, id in ipairs(peel.context.notes) do selected[id] = true end
local count = 0
for _, pattern in ipairs(peel.project.patterns) do
    if pattern.id == peel.context.pattern then
        for _, note in ipairs(pattern.notes) do
            local chosen = (#peel.context.notes > 0 and selected[note.id])
                or (#peel.context.notes == 0 and note.channel == peel.context.channel)
            if chosen then
                note.key = math.max(0, note.key - 12)
                count = count + 1
            end
        end
    end
end
peel.log("Transposed " .. count .. " notes down an octave.")
