local p = peel.project
local notes = 0
for _, pattern in ipairs(p.patterns) do notes = notes + #pattern.notes end
peel.log(p.meta.title .. ": " .. #p.channels .. " channels, " .. #p.patterns .. " patterns, " .. notes .. " notes.")
