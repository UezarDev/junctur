-- Junctur's bridge inside DaVinci Resolve (phases 5g, 5z, 5ab). SPDX-License-Identifier: GPL-3.0-or-later
--
-- Junctur installs this into Resolve's Scripts folder. Run it once per Resolve session
-- (Workspace > Scripts > Junctur) and leave it running: when Junctur's "Send to Resolve" writes a
-- request into its mailbox, this makes the edited timeline from Junctur's FCPXML (5z) and places
-- or restyles the captions on it as Text+ clips, or (5ab) updates only the captions on the
-- timeline already there. With the word highlight or an animation on (phase 5l) the clips are
-- auto-subs' animated caption, a Fusion macro round a Text+, driven through the macro's own helpers
-- as auto-subs drives it. Junctur's template picker (phase 5m) asks for the Fusion Titles in the
-- media pool; with a template's own look kept, only the text, timing and speaker colour are set on
-- it. Clips Junctur did not place that sit where new captions go (phase 5n) are asked about before
-- anything changes, then replaced, skipped round, or left with the captions put on a track beside.
--
-- Resolve 21 runs scripts with no io, require or os.execute, so a request is a Lua chunk read with
-- loadfile and the only way back is Fusion's prefs (SetPrefs + SavePrefs writes Fusion.prefs,
-- which Junctur reads); auto-subs (tmoroney/auto-subs, MIT) found this channel. Junctur works out
-- every frame, position, colour and input before asking; this only carries it out.
-- The paths below are filled in by Junctur when it installs the script.

local REQUEST = __JUNCTUR_REQUEST__
local TEMPLATE_BIN = __JUNCTUR_TEMPLATE_BIN__
local PREFS = "Global.JuncturBridge."
-- The root media pool bin Junctur keeps its own Text+ templates in: the plain one and the animated
-- caption (5l).
local BIN = "Junctur"
local TEMPLATE_NAME = "Junctur Caption"
local ANIMATED_NAME = "Junctur Animated Caption"
-- The animated caption's macro tool in a clip's comp: a clip with it is driven as the macro.
local MACRO = "AutoSubs"
-- Told to Junctur in `check` (5z; `info` before), so it can tell a script too old for what it asks
-- (5m; 5n asks before placing over clips it did not place, which a 5m script would not; 5z makes
-- the timeline, and a 5z script from before the caption rounds met it would place without 5l's
-- highlight, 5m's kept look or 5n's question; 5ab adds `open`, the update of captions only).
local SCRIPT = "5ab"

-- Plain global lookups, not rawget(_G, ...): Resolve's script runner hands a script its own
-- environment, which holds these.
local resolve = resolve or Resolve()
local fusion = fusion or fu or resolve:Fusion()

local function hex(s)
    return (s:gsub(".", function(c) return string.format("%02x", c:byte()) end))
end

-- SavePrefs can fail for a moment while Resolve writes the file itself.
local function write_pref(key, value)
    for _ = 1, 5 do
        local ok = pcall(function()
            fusion:SetPrefs(PREFS .. key, value)
            fusion:SavePrefs()
        end)
        if ok then return end
        bmd.wait(0.05)
    end
end

local function reply(id, fields)
    local lines = {}
    for k, v in pairs(fields) do
        lines[#lines + 1] = k .. "\t" .. (tostring(v):gsub("[\t\r\n]", " "))
    end
    write_pref("JuncturReply", id .. ":" .. hex(table.concat(lines, "\n")))
end

local function current()
    local project = resolve:GetProjectManager():GetCurrentProject()
    if not project then return nil, "No project is open in Resolve" end
    local timeline = project:GetCurrentTimeline()
    if not timeline then
        return nil, "No timeline is open in Resolve: open the one the captions go on"
    end
    return project, timeline
end

local function list(t)
    if type(t) == "table" then return t end
    return {}
end

-- A title's name as Junctur compares it (5m): spaces, tabs and line breaks round it dropped, the
-- rule Junctur's own `title_name` (edit/captions.rs) applies to the names it lists and sends, so a
-- title listed in the picker is the one a send finds.
local function title_name(clip)
    local ok, name = pcall(function() return clip:GetClipProperty("Clip Name") end)
    if not ok then return "" end
    return (tostring(name or ""):gsub("^[ \t\n\r\f]+", ""):gsub("[ \t\n\r\f]+$", ""))
end

-- A Fusion Title (a Text+ dragged into the media pool), by the clip type auto-subs lists them by
-- (5m): only these are templates, in the picker's list and when a send looks one up by name.
local function is_title(clip)
    local ok, kind = pcall(function() return clip:GetClipProperty("Type") end)
    return ok and tostring(kind or ""):lower() == "fusion title"
end

-- Calls f(clip, its folder) for every clip under folder until f says stop.
local function each_clip(folder, f)
    for _, sub in ipairs(list(folder:GetSubFolderList())) do
        if each_clip(sub, f) then return true end
    end
    for _, clip in ipairs(list(folder:GetClipList())) do
        if f(clip, folder) then return true end
    end
end

local function sub_named(folder, name)
    for _, sub in ipairs(list(folder:GetSubFolderList())) do
        if sub:GetName() == name then return sub end
    end
end

-- A folder's id, nil when Resolve cannot give one: then no folder is ever taken for the import's.
local function folder_id(f)
    local ok, id = pcall(function() return f:GetUniqueId() end)
    if ok and type(id) == "string" and id ~= "" then return id end
end

-- The folders under root by id, or nil when any has no id.
local function folder_ids(root)
    local ids = {}
    for _, sub in ipairs(list(root:GetSubFolderList())) do
        local id = folder_id(sub)
        if not id then return nil end
        ids[id] = sub
    end
    return ids
end

-- Whether a clip name is the animated caption's: Junctur's name for it, or the bundled bin's own
-- ("AutoSubs Caption <date>") when renaming it did not take.
local function is_animated(n)
    n = tostring(n or "")
    return n == ANIMATED_NAME or n:sub(1, 16) == "AutoSubs Caption"
end

-- The Junctur bin's template of a kind: the animated caption by its name (either of them); the
-- plain one by its name, else the first clip there that is not an animated one (a Text+ of the
-- user's dropped into the bin, as 5g says to do).
local function own(bin, animated)
    local first, named
    for _, clip in ipairs(list(bin:GetClipList())) do
        local n = clip:GetClipProperty("Clip Name")
        if animated then
            if n == ANIMATED_NAME then return clip end
            if is_animated(n) then named = named or clip end
        elseif n == TEMPLATE_NAME then
            return clip
        elseif not is_animated(n) then
            first = first or clip
        end
    end
    return named or first
end

-- The user's Fusion Title when one is named; else the Junctur bin's template of the kind asked
-- for (`animated`: the word highlight or an animation, 5l); else the bundled bin imported, its
-- templates the Junctur bin lacks kept there and the rest of the import deleted.
-- The import's folder is touched only when it is known for sure: exactly one folder new by id
-- after the import, named as the bundled bin is ("AutoSubs", or a name Resolve made from it).
-- Anything less and nothing is moved or deleted, so an auto-subs bin of the user's (which holds
-- a "Basic Template" too) is never what is cleaned up.
local function template(pool, name, animated)
    local root = pool:GetRootFolder()
    if name ~= "" then
        local found
        -- Fusion Titles only, as the picker lists them: a video clip or a still of the same name
        -- earlier in the walk is no template. Both names trimmed by one rule (`title_name`).
        name = name:gsub("^[ \t\n\r\f]+", ""):gsub("[ \t\n\r\f]+$", "")
        each_clip(root, function(clip)
            if is_title(clip) and title_name(clip) == name then
                found = clip
                return true
            end
        end)
        if found then return found end
        return nil, "No Text+ title called \"" .. name .. "\" in Resolve's media pool: drag your "
            .. "Text+ into the media pool and give it that name, or pick Junctur's own"
    end
    local bin = sub_named(root, BIN)
    local found = bin and own(bin, animated)
    if found then return found end
    local unsure = "Resolve did not say which media pool bin the import made, so Junctur left it "
        .. "alone: drag its Text+ (\"Basic Template\") into a bin named \"Junctur\" yourself, or name "
        .. "a Text+ title of yours as the template"
    local before = folder_ids(root)
    if not before then return nil, unsure end
    local was = pool:GetCurrentFolder()
    pool:SetCurrentFolder(root)
    pcall(function() return pool:ImportFolderFromFile(TEMPLATE_BIN) end)
    if was then pcall(function() pool:SetCurrentFolder(was) end) end
    local after = folder_ids(root)
    if not after then return nil, unsure end
    local new, count = nil, 0
    for id, sub in pairs(after) do
        if not before[id] then
            new = sub
            count = count + 1
        end
    end
    if count == 0 then return nil, "Could not import Junctur's caption template into the media pool" end
    if count > 1 or new:GetName():sub(1, 8) ~= "AutoSubs" then return nil, unsure end
    -- The import's plain Text+ and its animated caption (named with a date, "AutoSubs Caption …").
    local basic, anim
    for _, clip in ipairs(list(new:GetClipList())) do
        local n = tostring(clip:GetClipProperty("Clip Name") or "")
        if n == "Basic Template" then
            basic = clip
        elseif n:sub(1, 16) == "AutoSubs Caption" then
            anim = clip
        end
    end
    local want = basic
    if animated then want = anim end
    if not want then
        return nil, "Junctur's caption template bin had no "
            .. (animated and "animated caption" or "Text+") .. " in it"
    end
    -- Each kind the Junctur bin lacks is kept, so a bin from before 5l gains the animated one.
    local keep, names = {}, {}
    if basic and not (bin and own(bin, false)) then
        keep[#keep + 1], names[#names + 1] = basic, TEMPLATE_NAME
    end
    if anim and not (bin and own(bin, true)) then
        keep[#keep + 1], names[#names + 1] = anim, ANIMATED_NAME
    end
    bin = bin or pool:AddSubFolder(root, BIN)
    if was then pcall(function() pool:SetCurrentFolder(was) end) end
    if not bin or not pool:MoveClips(keep, bin) then
        return nil, "Could not move Junctur's caption template into a \"Junctur\" bin"
    end
    for k, clip in ipairs(keep) do
        pcall(function() clip:SetClipProperty("Clip Name", names[k]) end)
    end
    local rest = list(new:GetClipList())
    if #rest > 0 then pool:DeleteClips(rest) end
    pool:DeleteFolders({ new })
    return want
end

-- Resolve's id for the timeline, else its name: `place` checks it is the one `build` made.
local function timeline_id(timeline)
    local ok, id = pcall(function() return timeline:GetUniqueId() end)
    if ok and type(id) == "string" and id ~= "" then return id end
    return "name:" .. tostring(timeline:GetName())
end

-- A clip's Text+ (the tags go on it) and its comp.
local function textplus(item)
    local ok, tool, comp = pcall(function()
        local n = item:GetFusionCompCount()
        if not n or n < 1 then return nil end
        local comp = item:GetFusionCompByIndex(1)
        return comp and (comp:FindTool("Template") or comp:FindToolByID("TextPlus")), comp
    end)
    if not ok or not tool then return nil end
    return tool, comp
end

-- Sets an input and reads it back: nil back means the tool has no such input (a user's own
-- title may lack some), and Fusion does not say so when it is set.
local function set(tool, key, value)
    pcall(function() tool:SetInput(key, value) end)
    local ok, got = pcall(function() return tool:GetInput(key) end)
    return ok and got ~= nil, got
end

-- Lua source compiled as the macro's own controls compile theirs (`loadstring`), or by `load` on a
-- Lua that only has that.
local compile = loadstring or function(src)
    if load then return load(src) end
    return nil, "this Lua has neither loadstring nor load"
end

-- What a send works out once instead of for every caption: each macro helper compiled, and
-- whether the macro's inputs have been read back yet. Reset by each `place`.
local per_send = { helpers = {} }

-- The animated caption's helper `name` (its Lua source is kept in the macro's data), compiled.
local function helper(macro, name)
    local src = macro:GetData(name)
    if type(src) ~= "string" then
        return nil, "the animated caption has no " .. name .. " helper (edited in Fusion?): delete \""
            .. ANIMATED_NAME .. "\" and its captions, and send again; Junctur puts it back"
    end
    local fn = per_send.helpers[src]
    if fn then return fn end
    local chunk, why = compile(src)
    if not chunk then
        return nil, "Resolve's script runner here cannot compile the animated caption's helpers ("
            .. tostring(why) .. "), so the word highlight and animation cannot be applied: turn "
            .. "them off to send plain captions"
    end
    local ok, made = pcall(chunk)
    if not ok or type(made) ~= "function" then
        return nil, "the animated caption's " .. name .. " helper did not load: " .. tostring(made)
    end
    per_send.helpers[src] = made
    return made
end

-- Styles an animated caption (5l) as auto-subs does: the word timing set on the macro before the
-- Text+'s Text (the macro's own Text control turns it into the highlight's keyframes when the
-- text changes), the macro's CharacterLevelStyling given the text too, then every published
-- input through the macro's own SetInputValues helper, which also runs its highlight and
-- animation helpers. The inputs are read back off the macro once a send; one not taken goes into
-- `missing`, as does the highlight when the macro did not keep the word timing.
local function restyle_macro(comp, macro, tool, look, c, missing)
    local apply, why = helper(macro, "SetInputValues")
    if not apply then error(why) end
    local timed = pcall(function() macro:SetData("WordTiming", c.words) end)
    if timed then
        local ok, got = pcall(function() return macro:GetData("WordTiming") end)
        timed = ok and type(got) == "table" and #got == #c.words
    end
    if look.highlight and not timed then missing["word highlight"] = true end
    -- The macro's Text+ takes its text through its own `Text` control: its StyledText is wired to
    -- the macro's Follower, which draws the highlight and the animation. A copy without the
    -- control gets StyledText, so its caption shows, unlit and still.
    local took, text = set(tool, "Text", c.text)
    if not took or tostring(text) ~= c.text then
        local plain, got = set(tool, "StyledText", c.text)
        if not plain or tostring(got) ~= c.text then
            error("the animated caption's Text+ did not take the caption's text")
        end
        if look.highlight then missing["word highlight"] = true end
        if look.animation and look.animation ~= "none" then missing.animation = true end
    end
    pcall(function()
        local cls = comp:FindTool("CharacterLevelStyling1")
        if cls then cls:SetInput("Text", c.text) end
    end)
    local anim = look.animation
    local on = function(b) return b and 1 or 0 end
    local settings = {
        -- Fill: the said word's letters change colour (the macro's style 0).
        HighlightEnabled = on(look.highlight), HighlightStyle = 0,
        HighlightColorRed = c.highlight[1], HighlightColorGreen = c.highlight[2],
        HighlightColorBlue = c.highlight[3],
        -- Pop and slide fade as well, by the macro's own rule.
        FadeEnabled = on(anim == "fade"), PopInEnabled = on(anim == "pop"),
        SlideUpEnabled = on(anim == "slide"),
        AnimationLength = look.animationLength,
        -- The whole caption in and out, not the macro's word-by-word reveal.
        AnimationLevel = 0, AnimationMode = 2,
    }
    -- The text colour: the speaker's, unless the template's look is kept and the speaker has
    -- none (5m).
    if not (look.keep and c.colored == false) then
        settings.FillEnabled = 1
        settings.FillColorRed = c.base[1]
        settings.FillColorGreen = c.base[2]
        settings.FillColorBlue = c.base[3]
    end
    -- The look, unless the template's own is kept (5m).
    if not look.keep then
        settings.Font = look.font
        settings.Style = look.style
        settings.TextSize = look.size
        settings.TextPosition = { c.x, c.y }
        settings.OutlineEnabled = on(look.outline)
        -- Junctur wraps the lines itself.
        settings.WrapEnabled = 0
        if look.outline then
            settings.OutlineThickness = look.thickness
            settings.OutlineColorRed = look.outlineColor[1]
            settings.OutlineColorGreen = look.outlineColor[2]
            settings.OutlineColorBlue = look.outlineColor[3]
        end
    end
    local ok, e = pcall(apply, comp, macro, settings)
    if not ok then error("the animated caption's SetInputValues failed: " .. tostring(e)) end
    -- Every clip of a send is the same macro with the same inputs: read back on the first only.
    if not per_send.checked then
        per_send.checked = true
        for key in pairs(settings) do
            local read, got = pcall(function() return macro:GetInput(key) end)
            if not read or got == nil then missing[key] = true end
        end
    end
    if not look.keep and not set(tool, "Underline", on(look.underline)) then
        missing.Underline = true
    end
end

-- Styles a caption's Text+; errors when the text itself does not take (nothing to show), and
-- adds the names of other inputs the tool did not take to `missing`. A clip of the animated
-- caption is styled through its macro; a plain Text+ asked for the highlight or an animation (a
-- user's own template) is styled as 5g did, and those two are named in `missing`. With the
-- template's look kept (`look.keep`, 5m) only the text and the speaker's colour are set (none
-- when the speaker has no colour), so the template keeps its font, size, place and outline.
local function restyle(comp, tool, look, c, missing)
    local ok, macro = pcall(function() return comp and comp:FindTool(MACRO) end)
    if ok and macro then return restyle_macro(comp, macro, tool, look, c, missing) end
    if look.highlight then missing["word highlight"] = true end
    if look.animation and look.animation ~= "none" then missing.animation = true end
    local took, text = set(tool, "StyledText", c.text)
    if not took or tostring(text) ~= c.text then
        error("the template's Text+ did not take the caption's text")
    end
    if look.keep then
        if c.colored ~= false then
            for k, key in ipairs({ "Red1", "Green1", "Blue1" }) do
                if not set(tool, key, c.fill[k]) then missing[key] = true end
            end
        end
        return
    end
    local inputs = {
        { "Font", look.font }, { "Style", look.style }, { "Size", look.size },
        { "Center", { c.x, c.y } }, { "CharacterOffset", { 0, 0 } },
        { "Enabled1", 1 }, { "Red1", c.fill[1] }, { "Green1", c.fill[2] }, { "Blue1", c.fill[3] },
        { "Underline", look.underline and 1 or 0 }, { "Enabled2", look.outline and 1 or 0 },
    }
    if look.outline then
        inputs[#inputs + 1] = { "Red2", look.outlineColor[1] }
        inputs[#inputs + 1] = { "Green2", look.outlineColor[2] }
        inputs[#inputs + 1] = { "Blue2", look.outlineColor[3] }
        inputs[#inputs + 1] = { "Thickness2", look.thickness }
    end
    for _, kv in ipairs(inputs) do
        if not set(tool, kv[1], kv[2]) then missing[kv[1]] = true end
    end
end

local handlers = {}

-- What `place` needs to know of the timeline the captions go on, and of the template (`build`
-- answers it for the timeline it made, 5z; `info` did for the open one before).
local function describe(timeline, clip)
    return {
        name = timeline:GetName(),
        id = timeline_id(timeline),
        fps = timeline:GetSetting("timelineFrameRate"),
        start = timeline:GetStartFrame(),
        ["end"] = timeline:GetEndFrame(),
        width = timeline:GetSetting("timelineResolutionWidth"),
        height = timeline:GetSetting("timelineResolutionHeight"),
        template = clip and clip:GetClipProperty("Clip Name") or "",
        templateFps = clip and clip:GetClipProperty("FPS") or "",
    }
end

-- The Fusion Titles in the open project's media pool, for Junctur's template picker (5m): by clip
-- name, each once, in the order `template` looks them up (so the one listed is the one a send
-- uses), Junctur's own two left out. auto-subs lists them by the same clip type.
function handlers.templates()
    local project = resolve:GetProjectManager():GetCurrentProject()
    if not project then return { error = "No project is open in Resolve" } end
    local out, seen, n = {}, {}, 0
    each_clip(project:GetMediaPool():GetRootFolder(), function(clip)
        local name = title_name(clip)
        if is_title(clip) and name ~= "" and name ~= TEMPLATE_NAME and name ~= ANIMATED_NAME
            and not seen[name] then
            seen[name] = true
            n = n + 1
            out["t" .. n] = name
        end
    end)
    out.count = n
    return out
end

-- The project's timelines, or nil and why: a count Resolve will not give is an error, never taken
-- as "no timelines", so Junctur never decides on a guess.
local function project_timelines(project)
    local ok, n = pcall(function() return project:GetTimelineCount() end)
    n = ok and tonumber(n)
    if not n then return nil, "Resolve did not say which timelines the project has" end
    local out = {}
    for i = 1, n do
        local got, t = pcall(function() return project:GetTimelineByIndex(i) end)
        if not (got and t) then return nil, "Resolve did not give the project's timeline " .. i end
        out[#out + 1] = t
    end
    return out
end

-- Timeline names compared as Junctur compares them (resolve/timeline.rs): ASCII spaces round them
-- dropped and ASCII letters' case ignored; any other byte must match exactly.
local function norm_name(s)
    s = tostring(s or ""):gsub("^[ \t\n\r\f]+", ""):gsub("[ \t\n\r\f]+$", "")
    return (s:gsub("[A-Z]", function(c) return string.char(c:byte() + 32) end))
end

local function same_name(a, b) return norm_name(a) == norm_name(b) end

-- The project's timelines called `name`, leaving out `except`; nil and why when Resolve will not
-- list them.
local function timelines_named(project, name, except)
    local all, err = project_timelines(project)
    if not all then return nil, err end
    local out = {}
    for _, t in ipairs(all) do
        if t ~= except and same_name(t:GetName(), name) then out[#out + 1] = t end
    end
    return out
end

local function no_project()
    return { error = "No project is open in Resolve: open the one to send to" }
end

-- 5z: the open project's timeline names, so Junctur can ask before a send would meet one, and
-- this script's version (5n's check, moved here from `info`).
function handlers.check(req)
    local project = resolve:GetProjectManager():GetCurrentProject()
    if not project then return no_project() end
    local all, err = project_timelines(project)
    if not all then return { error = err } end
    local out = {}
    for k, t in ipairs(all) do out["timeline" .. k] = t:GetName() end
    out.timelines = #all
    -- The first request of a send: Junctur refuses a script older than it needs before anything.
    out.script = SCRIPT
    return out
end

-- Paths as Resolve and Windows may write them: slashes and ASCII case aside, the same file. Any
-- other byte must match exactly; a path that then does not match is only a miss, and the build
-- has Resolve import the recording instead.
local function same_path(a, b)
    local function norm(s)
        s = tostring(s or ""):gsub("\\", "/")
        return (s:gsub("[A-Z]", function(c) return string.char(c:byte() + 32) end))
    end
    return norm(a) == norm(b)
end

-- Whether a media pool clip is the whole recording as imported: a plain video or audio clip with
-- no in or out of its own. A sub-clip, a marked clip, a timeline or a title could link the edit
-- to a part of the recording or to something else.
local PLAIN = { ["Video + Audio"] = true, ["Video"] = true, ["Audio"] = true }
local function master(c)
    local function prop(k)
        local ok, v = pcall(function() return c:GetClipProperty(k) end)
        return ok and v or nil
    end
    if not PLAIN[prop("Type") or ""] then return false end
    if (prop("In") or "") ~= "" or (prop("Out") or "") ~= "" then return false end
    local ok, marks = pcall(function() return c:GetMarkInOut() end)
    if ok and type(marks) == "table" then
        for _, kind in pairs(marks) do
            if type(kind) == "table" and (kind["in"] ~= nil or kind.out ~= nil) then return false end
        end
    end
    return true
end

-- 5z: makes the edit's timeline from Junctur's FCPXML (req.file) and makes it the current one,
-- leaving every other timeline as it is. The recording already in the media pool is linked to
-- rather than imported again. With `replace` the timeline of that name stays until `finish`, so
-- whatever fails before then (the import, the captions) never costs it: the new one is made under
-- a free name beside it.
function handlers.build(req)
    local project = resolve:GetProjectManager():GetCurrentProject()
    if not project then return no_project() end
    local pool = project:GetMediaPool()
    local olds, err = timelines_named(project, req.name)
    if not olds then return { error = err } end
    if #olds > 0 and not req.replace then
        return { error = "Resolve now has a timeline called \"" .. req.name
            .. "\" (made since Junctur looked): nothing was changed, send again" }
    end
    if #olds > 1 then
        return { error = "Resolve has " .. #olds .. " timelines called \"" .. req.name
            .. "\", so Junctur cannot tell which to replace: nothing was changed. Rename or delete "
            .. "all but one, or choose Create new timeline" }
    end
    local clip
    if req.captions then
        -- The animated caption (5l) when the highlight or an animation is on and no template of
        -- the user's is named; a named one is looked up among the Fusion Titles (5m).
        clip, err = template(pool, req.template or "", req.animated == true)
        if not clip then return { error = err .. " (nothing was changed on the timelines)" } end
    end
    local as = req.name
    if #olds > 0 then
        local k = 1
        repeat
            as = req.name .. " (new" .. (k > 1 and " " .. k or "") .. ")"
            k = k + 1
            local taken
            taken, err = timelines_named(project, as)
            if not taken then return { error = err } end
        until #taken == 0
    end
    local media, folder
    each_clip(pool:GetRootFolder(), function(c, f)
        local ok, p = pcall(function() return c:GetClipProperty("File Path") end)
        if ok and p and p ~= "" and same_path(p, req.source) and master(c) then
            media, folder = c, f
            return true
        end
    end)
    local function import(opts)
        opts.timelineName = as
        local ok, t = pcall(function() return pool:ImportTimelineFromFile(req.file, opts) end)
        if ok and t then return t end
    end
    local made = media and import({ importSourceClips = false, sourceClipsFolders = { folder } })
    made = made or import({ importSourceClips = true })
    if not made then
        return { error = "Resolve could not make the timeline from Junctur's FCPXML (" .. req.file
            .. "): nothing was changed on the timelines" }
    end
    pcall(function() project:SetCurrentTimeline(made) end)
    return describe(made, clip)
end

-- 5z, Replace, once the captions are on the new timeline: deletes the one it replaces and gives
-- the new one its name. Anything Resolve refuses leaves both and says so.
function handlers.finish(req)
    local project = resolve:GetProjectManager():GetCurrentProject()
    if not project then return no_project() end
    local all, err = project_timelines(project)
    if not all then return { error = err } end
    local made
    for _, t in ipairs(all) do
        if timeline_id(t) == req.made then made = t end
    end
    if not made then
        return { error = "The timeline Junctur made is no longer in the project: nothing was replaced" }
    end
    local olds
    olds, err = timelines_named(project, req.name, made)
    if not olds then return { error = err } end
    local keep = "\"" .. tostring(made:GetName()) .. "\" was made beside it"
    if #olds ~= 1 then
        return { replaced = false, name = made:GetName(), note = "Resolve now has " .. #olds
            .. " timelines called \"" .. req.name .. "\", so none was replaced: " .. keep }
    end
    local ok, gone = pcall(function() return project:GetMediaPool():DeleteTimelines({ olds[1] }) end)
    if not (ok and gone) then
        return { replaced = false, name = made:GetName(),
            note = "Resolve did not delete the old \"" .. req.name .. "\": " .. keep }
    end
    local note = ""
    local named = pcall(function() return made:SetName(req.name) end)
    if not named or not same_name(made:GetName(), req.name) then
        note = "The old \"" .. req.name .. "\" was deleted, but Resolve did not rename the new one: it is \""
            .. tostring(made:GetName()) .. "\""
    end
    return { replaced = true, name = made:GetName(), note = note }
end

-- Where the timeline's own cut ends: the last end frame of any clip on its audio tracks and on
-- its video tracks other than Junctur's caption tracks ("Junctur", "Junctur <id> <name>"); nil
-- when there are none.
local function cut_end(t)
    local last
    for _, kind in ipairs({ "video", "audio" }) do
        local ok, n = pcall(function() return t:GetTrackCount(kind) end)
        for i = 1, (ok and tonumber(n) or 0) do
            local name = kind == "video" and tostring(t:GetTrackName(kind, i) or "") or ""
            if name ~= "Junctur" and name:sub(1, 8) ~= "Junctur " then
                for _, item in ipairs(list(t:GetItemListInTrack(kind, i))) do
                    local got, e = pcall(function() return item:GetEnd() end)
                    e = got and tonumber(e)
                    if e and (not last or e > last) then last = e end
                end
            end
        end
    end
    return last
end

-- 5ab, Update captions only: the one timeline called req.name made the current one, so `place`
-- can put the captions on it (an append goes to the current timeline), and where its cut ends, so
-- Junctur can tell a cut that no longer matches its edit. Nothing on any timeline changes.
function handlers.open(req)
    local project = resolve:GetProjectManager():GetCurrentProject()
    if not project then return no_project() end
    local found, err = timelines_named(project, req.name)
    if not found then return { error = err } end
    if #found == 0 then
        return { error = "Resolve no longer has a timeline called \"" .. req.name
            .. "\": nothing was changed. Send again to make it" }
    end
    if #found > 1 then
        return { error = "Resolve has " .. #found .. " timelines called \"" .. req.name
            .. "\", so Junctur cannot tell which to update: nothing was changed. Rename or delete "
            .. "all but one, or choose Create new timeline" }
    end
    local t = found[1]
    -- Switched first and the template looked up after: with none named and no Junctur bin yet
    -- the bundled bin is imported, and that is not done for a send Resolve then refuses.
    local was = project:GetCurrentTimeline()
    pcall(function() project:SetCurrentTimeline(t) end)
    local now = project:GetCurrentTimeline()
    -- Asked to switch, Resolve may have switched somewhere else: say what is open now.
    if not now or timeline_id(now) ~= timeline_id(t) then
        local shown = now and ("now has \"" .. tostring(now:GetName()) .. "\" open")
            or "now has no timeline open"
        return { error = "Resolve did not switch to the timeline \"" .. tostring(t:GetName())
            .. "\": it " .. shown .. ", and no clips were changed. Open \"" .. tostring(t:GetName())
            .. "\" yourself and send again" }
    end
    -- The animated caption (5l) when the highlight or an animation is on, as `build` looks it up.
    -- Not found: the timeline open before is open again, so nothing is changed.
    local clip
    clip, err = template(project:GetMediaPool(), req.template or "", req.animated == true)
    if not clip then
        if was then pcall(function() project:SetCurrentTimeline(was) end) end
        return { error = err .. " (nothing was changed on the timelines)" }
    end
    local out = describe(t, clip)
    out.cutEnd = cut_end(t) or ""
    return out
end

-- The items on video track `i` by their first frame.
local function by_start(timeline, i)
    local out = {}
    for _, item in ipairs(list(timeline:GetItemListInTrack("video", i))) do
        local ok, s = pcall(function() return item:GetStart() end)
        if ok and s then out[s] = item end
    end
    return out
end

-- An item's first frame and the frame after its last (5n). A bound that cannot be read is taken
-- as unbounded (the start as the timeline's beginning, the end as its end) and the item marked
-- `unsure`: a clip Junctur cannot measure is counted in the way of every caption it might meet,
-- never passed over, and never deleted (see Replace in `place`).
local function span(item)
    local ok, s = pcall(function() return item:GetStart() end)
    local got, e = pcall(function() return item:GetEnd() end)
    local unsure = false
    if not ok or type(s) ~= "number" then s, unsure = -math.huge, true end
    if not got or type(e) ~= "number" or e <= s then e, unsure = math.huge, true end
    return s, e, unsure
end

-- An item's Junctur recording tag, nil for one Junctur did not place.
local function tag_of(item)
    local tool = textplus(item)
    local ok, tag = pcall(function() return tool and tool:GetData("JuncturRecording") end)
    if ok then return tag end
end

-- The items on video track `i` that are not `recording`'s, with their frames (5n). With no
-- recording, those with no Junctur tag at all: the clips Junctur did not place (a video clip, a
-- still, the user's own title, or a caption whose tags were lost).
local function others(timeline, i, recording)
    local out = {}
    for _, item in ipairs(list(timeline:GetItemListInTrack("video", i))) do
        local tag = tag_of(item)
        if (recording == nil and tag == nil) or (recording ~= nil and tag ~= recording) then
            local s, e, unsure = span(item)
            out[#out + 1] = { item = item, s = s, e = e, unsure = unsure }
        end
    end
    return out
end

-- Whether video track `i` holds a clip of `recording` (5n: a track beside already in use).
local function holds(timeline, i, recording)
    for _, item in ipairs(list(timeline:GetItemListInTrack("video", i))) do
        if tag_of(item) == recording then return true end
    end
    return false
end

-- Whether an item's frames (from `others`) meet a caption's, both ends exclusive.
local function overlaps(o, c)
    return o.s < c["end"] and c.start < o.e
end

function handlers.place(req)
    local project, timeline = current()
    if not project then return { error = timeline } end
    if req.timeline and req.timeline ~= "" and timeline_id(timeline) ~= req.timeline then
        return { error = "The open timeline in Resolve changed while sending: send again" }
    end
    local pool = project:GetMediaPool()
    local clip, err = template(pool, req.template or "", req.animated == true)
    if not clip then return { error = err } end
    per_send = { helpers = {} }

    -- The video tracks by name. Each speaker's is added further on, once nothing stops the send.
    local index = {}
    local count = timeline:GetTrackCount("video")
    for i = 1, count do index[timeline:GetTrackName("video", i)] = i end

    -- Clips an earlier send placed for this recording, by their tag.
    local sent, stale = {}, {}
    for i = 1, count do
        for _, item in ipairs(list(timeline:GetItemListInTrack("video", i))) do
            local tool, comp = textplus(item)
            if tool and tool:GetData("JuncturRecording") == req.recording then
                local key = tool:GetData("JuncturKey")
                if key and not sent[key] then
                    sent[key] = { item = item, tool = tool, comp = comp }
                else
                    stale[#stale + 1] = item
                end
            end
        end
    end
    -- The captions no clip of an earlier send stands for: the ones to place.
    local todo, kept = {}, {}
    for _, c in ipairs(req.clips) do
        if sent[c.key] then kept[c.key] = true else todo[#todo + 1] = c end
    end

    -- 5n: clips Junctur did not place where those captions go, on the tracks they go on. Found
    -- before anything is changed, so asking leaves Resolve as it was. `meets[k]`: the clips in
    -- caption k's way.
    local mode = req.existing
    if mode ~= "replace" and mode ~= "skip" and mode ~= "newTrack" then mode = "ask" end
    local untagged, hits, meets, nblocked = {}, {}, {}, 0
    for k, c in ipairs(todo) do
        local i = index[req.tracks[c.track]]
        if i then
            untagged[i] = untagged[i] or others(timeline, i, nil)
            for _, o in ipairs(untagged[i]) do
                if overlaps(o, c) then
                    if not o.hit then
                        o.hit = true
                        hits[#hits + 1] = o
                    end
                    if not meets[k] then
                        meets[k] = {}
                        nblocked = nblocked + 1
                    end
                    table.insert(meets[k], o)
                end
            end
        end
    end
    if #hits > 0 and mode == "ask" then
        return { inTheWay = #hits, blocked = nblocked }
    end

    -- Everything that can stop the send is settled before anything changes: the tracks each
    -- speaker's captions go on, then the tracks added, and only then the user's clips deleted.
    local replaced, skipped, beside = 0, 0, 0
    local doomed = {}
    if mode == "skip" or mode == "replace" then
        -- Skip leaves out every caption in a clip's way. Replace deletes the clips in the way of
        -- the rest, but never one Junctur could not measure: the captions such a clip might meet
        -- are left out instead, as Skip would.
        local rest, marked = {}, {}
        for k, c in ipairs(todo) do
            local out = meets[k] and mode == "skip"
            if meets[k] and not out then
                for _, o in ipairs(meets[k]) do
                    if o.unsure then out = true end
                end
            end
            if out then
                skipped = skipped + 1
            else
                rest[#rest + 1] = c
                for _, o in ipairs(meets[k] or {}) do
                    if not marked[o] then
                        marked[o] = true
                        doomed[#doomed + 1] = o.item
                    end
                end
            end
        end
        todo = rest
    end
    local names = {}
    for k, name in ipairs(req.tracks) do names[k] = name end
    if mode == "newTrack" then
        -- A speaker with a caption in the way has all its new captions placed on "<its track>
        -- (2)", or the first of (3) to (9) where none of them meets a clip not of this recording.
        -- A speaker who has such a track holding its captions from an earlier send keeps using it
        -- while it is clear, even with nothing in the way any more, so its captions are not
        -- split back onto its own track. Its captions already placed stay where they are.
        local moving, speaking = {}, {}
        for k, c in ipairs(todo) do
            speaking[c.track] = true
            if meets[k] then moving[c.track] = true end
        end
        local function clear(t, i)
            local theirs = others(timeline, i, req.recording)
            for _, c in ipairs(todo) do
                if c.track == t then
                    for _, o in ipairs(theirs) do
                        if overlaps(o, c) then return false end
                    end
                end
            end
            return true
        end
        for t in pairs(speaking) do
            local free
            for n = 2, 9 do
                local name = req.tracks[t] .. " (" .. n .. ")"
                local i = index[name]
                if i and holds(timeline, i, req.recording) and clear(t, i) then
                    free = name
                    break
                end
            end
            if not free and moving[t] then
                for n = 2, 9 do
                    local name = req.tracks[t] .. " (" .. n .. ")"
                    local i = index[name]
                    if not i or clear(t, i) then
                        free = name
                        break
                    end
                end
                if not free then
                    return { error = "Tracks \"" .. req.tracks[t] .. " (2)\" to (9) all have clips "
                        .. "where its captions go, so nothing was changed: choose Replace or Skip" }
                end
            end
            if free then
                names[t] = free
                beside = beside + 1
            end
        end
    end

    -- Each speaker's track, by name, added when missing.
    local track = {}
    for k, name in ipairs(names) do
        if not index[name] then
            local ok, added = pcall(function() return timeline:AddTrack("video") end)
            local now = timeline:GetTrackCount("video")
            -- Without a new track, naming the top one would rename one of the user's.
            if not ok or not added or now ~= count + 1 then
                return { error = "Resolve did not add a video track for " .. name }
            end
            count = now
            timeline:SetTrackName("video", count, name)
            index[name] = count
        end
        track[k] = index[name]
    end

    -- Replace: the user's clips in the way deleted last of all that can stop the send. Refused,
    -- no caption is placed or changed (a speaker's track added above stays, empty).
    if #doomed > 0 then
        local ok, done = pcall(function() return timeline:DeleteClips(doomed, false) end)
        if not (ok and done) then
            return { error = "Resolve did not delete the " .. #doomed .. " clips in the captions' "
                .. "way, so no caption was placed or changed: unlock their track, or choose "
                .. "another way" }
        end
        replaced = #doomed
    end

    local placed, restyled, removed, failed, first = 0, 0, 0, 0, nil
    local missing = {}
    local function fail(e)
        failed = failed + 1
        first = first or tostring(e)
    end
    for _, c in ipairs(req.clips) do
        local old = sent[c.key]
        if old then
            local ok, e = pcall(restyle, old.comp, old.tool, req.look, c, missing)
            if ok then restyled = restyled + 1 else fail(e) end
        end
    end
    for key, old in pairs(sent) do
        if not kept[key] then stale[#stale + 1] = old.item end
    end
    if #stale > 0 then
        local ok, done = pcall(function() return timeline:DeleteClips(stale, false) end)
        if ok and done then
            removed = #stale
        else
            first = first or ("Resolve did not delete " .. #stale .. " captions of an earlier send")
        end
    end

    -- A locked track refuses clips: unlocked for the append, locked again after.
    local locked = {}
    for _, i in pairs(track) do
        local ok, is = pcall(function() return timeline:GetIsTrackLocked("video", i) end)
        if ok and is then
            locked[#locked + 1] = i
            timeline:SetTrackLock("video", i, false)
        end
    end
    local function info(c)
        return {
            mediaPoolItem = clip,
            mediaType = 1,
            startFrame = 0,
            endFrame = c.length,
            recordFrame = c.start,
            trackIndex = track[c.track],
        }
    end
    -- What the append placed is read back off the tracks (by track and first frame), not taken
    -- from what it returns: a batch Resolve refuses part of can return fewer items, or dead ones,
    -- while the rest are on the timeline.
    local function found()
        local tracks, out = {}, {}
        for k, c in ipairs(todo) do
            local i = track[c.track]
            tracks[i] = tracks[i] or by_start(timeline, i)
            local item = tracks[i][c.start]
            local tool = item and textplus(item)
            -- Not a clip an earlier send left there (or another recording's). An untagged clip of
            -- the user's cannot be there either (5n): one a new caption meets was asked about,
            -- deleted, skipped round or moved away from before the append.
            if tool and not tool:GetData("JuncturRecording") then out[k] = item end
        end
        return out
    end
    local items = {}
    if #todo > 0 then
        local infos = {}
        for k, c in ipairs(todo) do infos[k] = info(c) end
        pcall(function() return pool:AppendToTimeline(infos) end)
        items = found()
        -- One refused clip can fail the whole batch: the ones not there, one at a time.
        local again = false
        for k, c in ipairs(todo) do
            if not items[k] then
                pcall(function() return pool:AppendToTimeline({ info(c) }) end)
                again = true
            end
        end
        if again then items = found() end
    end
    for _, i in ipairs(locked) do timeline:SetTrackLock("video", i, true) end

    for k, c in ipairs(todo) do
        local item = items[k]
        local ok, e = pcall(function()
            if not item then
                error("Resolve did not place the caption at frame " .. c.start
                    .. " (is another clip in the way?)")
            end
            local tool, comp = textplus(item)
            if not tool then error("the template has no Text+ in it") end
            -- Tagged first: a clip whose styling fails is still found and replaced next time.
            tool:SetData("JuncturRecording", req.recording)
            tool:SetData("JuncturKey", c.key)
            restyle(comp, tool, req.look, c, missing)
        end)
        if ok then placed = placed + 1 else fail(e) end
    end
    local unstyled = {}
    for key in pairs(missing) do unstyled[#unstyled + 1] = key end
    table.sort(unstyled)
    return {
        placed = placed,
        restyled = restyled,
        removed = removed,
        failed = failed,
        firstError = first or "",
        unstyled = table.concat(unstyled, ", "),
        replaced = replaced,
        skipped = skipped,
        newTracks = beside,
    }
end

-- A test run with a stand-in Resolve (`JUNCTUR_TEST` set before loading) takes the handlers and
-- never starts the loop.
if JUNCTUR_TEST then return { handlers = handlers, reply = reply } end

-- One loop at a time: each start writes its own owner token, and a loop that sees another's stops.
local me = tostring(os.time()) .. " " .. tostring({})
pcall(function() fusion:SetPrefs(PREFS .. "Owner", me) end)
print("[Junctur] Listening for captions from Junctur. Leave this running.")

local last, tick = "", 0
while true do
    tick = tick + 1
    -- Prefs read only twice a second: each call goes through Resolve's UI queue (auto-subs saw a
    -- queued prefs call crash Resolve as it quit).
    if tick >= 5 then
        tick = 0
        local ok, owner = pcall(function() return fusion:GetPrefs(PREFS .. "Owner") end)
        if ok and owner ~= nil and owner ~= me then
            print("[Junctur] Another Junctur script took over; this one stops.")
            break
        end
    end
    if bmd.fileexists(REQUEST) then
        local chunk = loadfile(REQUEST)
        local ok, req = false, nil
        if chunk then ok, req = pcall(chunk) end
        if ok and type(req) == "table" and type(req.id) == "string" and req.id ~= last then
            last = req.id
            -- Ids are milliseconds × 1000: one older than two minutes is a leftover, not a send.
            local ms = tonumber(req.id)
            if not (ms and os.time() * 1000 - ms / 1000 > 120000) then
                write_pref("JuncturAck", req.id)
                local handler = handlers[req.kind]
                local done, result = pcall(function()
                    if not handler then return { error = "unknown request " .. tostring(req.kind) } end
                    return handler(req)
                end)
                if not done then result = { error = tostring(result) } end
                print("[Junctur] " .. tostring(req.kind) .. ": " .. (result.error or "done"))
                reply(req.id, result)
            end
        end
    end
    bmd.wait(0.1)
end
