-- Junctur's bridge inside DaVinci Resolve (phase 5g). SPDX-License-Identifier: GPL-3.0-or-later
--
-- Junctur installs this into Resolve's Scripts folder. Run it once per Resolve session
-- (Workspace > Scripts > Junctur) and leave it running: when Junctur's "Send to Resolve" writes a
-- request into its mailbox, this places or restyles the captions as Text+ clips. With the word
-- highlight or an animation on (phase 5l) the clips are auto-subs' animated caption, a Fusion
-- macro round a Text+, driven through the macro's own helpers as auto-subs drives it. Junctur's
-- template picker (phase 5m) asks for the Fusion Titles in the media pool; with a template's own
-- look kept, only the text, timing and speaker colour are set on it.
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
-- Told to Junctur in `info`, so it can tell a script too old for what it asks (5m).
local SCRIPT = "5m"

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

local function each_clip(folder, f)
    for _, sub in ipairs(list(folder:GetSubFolderList())) do
        if each_clip(sub, f) then return true end
    end
    for _, clip in ipairs(list(folder:GetClipList())) do
        if f(clip) then return true end
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
        each_clip(root, function(clip)
            if clip:GetClipProperty("Clip Name") == name then
                found = clip
                return true
            end
        end)
        if found then return found end
        return nil, "No clip called \"" .. name .. "\" in Resolve's media pool: drag your Text+ "
            .. "title into the media pool and give it that name, or leave the template empty"
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

-- Resolve's id for the timeline, else its name: `place` checks it is the one `info` saw.
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

function handlers.info(req)
    local project, timeline = current()
    if not project then return { error = timeline } end
    local clip, err = template(project:GetMediaPool(), req.template or "", req.animated == true)
    if not clip then return { error = err } end
    return {
        name = timeline:GetName(),
        id = timeline_id(timeline),
        fps = timeline:GetSetting("timelineFrameRate"),
        start = timeline:GetStartFrame(),
        ["end"] = timeline:GetEndFrame(),
        width = timeline:GetSetting("timelineResolutionWidth"),
        height = timeline:GetSetting("timelineResolutionHeight"),
        template = clip:GetClipProperty("Clip Name"),
        templateFps = clip:GetClipProperty("FPS") or "",
        script = SCRIPT,
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
        local ok, kind, name = pcall(function()
            return clip:GetClipProperty("Type"), clip:GetClipProperty("Clip Name")
        end)
        name = ok and tostring(name or "") or ""
        if ok and tostring(kind or ""):lower() == "fusion title" and name ~= ""
            and name ~= TEMPLATE_NAME and name ~= ANIMATED_NAME and not seen[name] then
            seen[name] = true
            n = n + 1
            out["t" .. n] = name
        end
    end)
    out.count = n
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

    -- Each speaker's track, by name, added when missing.
    local index = {}
    local count = timeline:GetTrackCount("video")
    for i = 1, count do index[timeline:GetTrackName("video", i)] = i end
    local track = {}
    for k, name in ipairs(req.tracks) do
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

    local placed, restyled, removed, failed, first = 0, 0, 0, 0, nil
    local missing = {}
    local function fail(e)
        failed = failed + 1
        first = first or tostring(e)
    end
    local todo, kept = {}, {}
    for _, c in ipairs(req.clips) do
        local old = sent[c.key]
        if old then
            kept[c.key] = true
            local ok, e = pcall(restyle, old.comp, old.tool, req.look, c, missing)
            if ok then restyled = restyled + 1 else fail(e) end
        else
            todo[#todo + 1] = c
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
            -- Not a clip an earlier send left there (or another recording's).
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
    local names = {}
    for key in pairs(missing) do names[#names + 1] = key end
    table.sort(names)
    return {
        placed = placed,
        restyled = restyled,
        removed = removed,
        failed = failed,
        firstError = first or "",
        unstyled = table.concat(names, ", "),
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
