-- Junctur's bridge inside DaVinci Resolve (phases 5g, 5z). SPDX-License-Identifier: GPL-3.0-or-later
--
-- Junctur installs this into Resolve's Scripts folder. Run it once per Resolve session
-- (Workspace > Scripts > Junctur) and leave it running: when Junctur's "Send to Resolve" writes a
-- request into its mailbox, this makes the edited timeline from Junctur's FCPXML (5z) and places
-- or restyles the captions on it as Text+ clips.
--
-- Resolve 21 runs scripts with no io, require or os.execute, so a request is a Lua chunk read with
-- loadfile and the only way back is Fusion's prefs (SetPrefs + SavePrefs writes Fusion.prefs,
-- which Junctur reads); auto-subs (tmoroney/auto-subs, MIT) found this channel. Junctur works out
-- every frame, position, colour and input before asking; this only carries it out.
-- The paths below are filled in by Junctur when it installs the script.

local REQUEST = __JUNCTUR_REQUEST__
local TEMPLATE_BIN = __JUNCTUR_TEMPLATE_BIN__
local PREFS = "Global.JuncturBridge."
-- The root media pool bin Junctur keeps its own Text+ template in.
local BIN = "Junctur"
local TEMPLATE_NAME = "Junctur Caption"

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

-- The user's Fusion Title when one is named; else the first clip of the Junctur bin; else the
-- bundled bin imported, its plain Text+ kept in a Junctur bin and the rest of the import deleted.
-- The import's folder is touched only when it is known for sure: exactly one folder new by id
-- after the import, named as the bundled bin is ("AutoSubs", or a name Resolve made from it).
-- Anything less and nothing is moved or deleted, so an auto-subs bin of the user's (which holds
-- a "Basic Template" too) is never what is cleaned up.
local function template(pool, name)
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
    local own = bin and list(bin:GetClipList())[1]
    if own then return own end
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
    local basic
    for _, clip in ipairs(list(new:GetClipList())) do
        if clip:GetClipProperty("Clip Name") == "Basic Template" then basic = clip end
    end
    if not basic then return nil, "Junctur's caption template bin had no Text+ in it" end
    bin = bin or pool:AddSubFolder(root, BIN)
    if was then pcall(function() pool:SetCurrentFolder(was) end) end
    if not bin or not pool:MoveClips({ basic }, bin) then
        return nil, "Could not move Junctur's caption template into a \"Junctur\" bin"
    end
    pcall(function() basic:SetClipProperty("Clip Name", TEMPLATE_NAME) end)
    local rest = list(new:GetClipList())
    if #rest > 0 then pool:DeleteClips(rest) end
    pool:DeleteFolders({ new })
    return basic
end

-- Resolve's id for the timeline, else its name: `place` checks it is the one `info` saw.
local function timeline_id(timeline)
    local ok, id = pcall(function() return timeline:GetUniqueId() end)
    if ok and type(id) == "string" and id ~= "" then return id end
    return "name:" .. tostring(timeline:GetName())
end

local function textplus(item)
    local ok, tool = pcall(function()
        local n = item:GetFusionCompCount()
        if not n or n < 1 then return nil end
        local comp = item:GetFusionCompByIndex(1)
        return comp and (comp:FindTool("Template") or comp:FindToolByID("TextPlus"))
    end)
    return ok and tool or nil
end

-- Sets an input and reads it back: nil back means the tool has no such input (a user's own
-- title may lack some), and Fusion does not say so when it is set.
local function set(tool, key, value)
    pcall(function() tool:SetInput(key, value) end)
    local ok, got = pcall(function() return tool:GetInput(key) end)
    return ok and got ~= nil, got
end

-- Styles a caption's Text+; errors when the text itself does not take (nothing to show), and
-- adds the names of other inputs the tool did not take to `missing`.
local function restyle(tool, look, c, missing)
    local took, text = set(tool, "StyledText", c.text)
    if not took or tostring(text) ~= c.text then
        error("the template's Text+ did not take the caption's text")
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

-- What `place` needs to know of the timeline the captions go on, and of the template.
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

local function project_timelines(project)
    local out = {}
    local ok, n = pcall(function() return project:GetTimelineCount() end)
    for i = 1, (ok and tonumber(n) or 0) do
        local t = project:GetTimelineByIndex(i)
        if t then out[#out + 1] = t end
    end
    return out
end

-- Timeline names are compared as Junctur compares them: spaces round them and case ignored.
local function same_name(a, b)
    local function norm(s) return (tostring(s or ""):gsub("^%s+", ""):gsub("%s+$", ""):lower()) end
    return norm(a) == norm(b)
end

local function timeline_named(project, name)
    for _, t in ipairs(project_timelines(project)) do
        if same_name(t:GetName(), name) then return t end
    end
end

-- 5z: the open project's timeline names, so Junctur can ask before a send would meet one.
function handlers.check(req)
    local project = resolve:GetProjectManager():GetCurrentProject()
    if not project then return { error = "No project is open in Resolve: open the one to send to" } end
    local out = {}
    local all = project_timelines(project)
    for k, t in ipairs(all) do out["timeline" .. k] = t:GetName() end
    out.timelines = #all
    return out
end

-- Paths as Resolve and Windows may write them: slashes and case aside, the same file.
local function same_path(a, b)
    local function norm(s) return (tostring(s or ""):gsub("\\", "/"):lower()) end
    return norm(a) == norm(b)
end

-- 5z: makes the edit's timeline from Junctur's FCPXML (req.file) and makes it the current one.
-- The recording already in the media pool is linked to rather than imported again. Replace
-- imports under a free name first and deletes the old timeline only once the new one is there,
-- so a failed import never costs it.
function handlers.build(req)
    local project = resolve:GetProjectManager():GetCurrentProject()
    if not project then return { error = "No project is open in Resolve: open the one to send to" } end
    local pool = project:GetMediaPool()
    local old = timeline_named(project, req.name)
    if old and not req.replace then
        return { error = "Resolve now has a timeline called \"" .. req.name .. "\": send again" }
    end
    local clip
    if req.captions then
        local err
        clip, err = template(pool, req.template or "")
        if not clip then return { error = err } end
    end
    local as = req.name
    if old then
        local k = 1
        repeat
            as = req.name .. " (new" .. (k > 1 and " " .. k or "") .. ")"
            k = k + 1
        until not timeline_named(project, as)
    end
    local media, folder
    each_clip(pool:GetRootFolder(), function(c, f)
        local ok, p = pcall(function() return c:GetClipProperty("File Path") end)
        if ok and p and p ~= "" and same_path(p, req.source) then
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
        return { error = "Resolve could not make the timeline from Junctur's FCPXML (" .. req.file .. ")" }
    end
    pcall(function() project:SetCurrentTimeline(made) end)
    local note, replaced = "", false
    if old then
        local ok, gone = pcall(function() return pool:DeleteTimelines({ old }) end)
        if not (ok and gone) then
            note = "Resolve did not delete the old \"" .. req.name .. "\", so the new timeline is \""
                .. as .. "\""
        else
            replaced = true
            local named = pcall(function() return made:SetName(req.name) end)
            if not named or not same_name(made:GetName(), req.name) then
                note = "Resolve did not rename the new timeline: it is \"" .. tostring(made:GetName()) .. "\""
            end
        end
    end
    local out = describe(made, clip)
    out.note = note
    out.replaced = replaced
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
    local clip, err = template(pool, req.template or "")
    if not clip then return { error = err } end

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
            local tool = textplus(item)
            if tool and tool:GetData("JuncturRecording") == req.recording then
                local key = tool:GetData("JuncturKey")
                if key and not sent[key] then
                    sent[key] = { item = item, tool = tool }
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
            local ok, e = pcall(restyle, old.tool, req.look, c, missing)
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
            local tool = textplus(item)
            if not tool then error("the template has no Text+ in it") end
            -- Tagged first: a clip whose styling fails is still found and replaced next time.
            tool:SetData("JuncturRecording", req.recording)
            tool:SetData("JuncturKey", c.key)
            restyle(tool, req.look, c, missing)
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
