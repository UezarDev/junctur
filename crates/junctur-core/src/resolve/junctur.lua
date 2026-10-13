-- Junctur's bridge inside DaVinci Resolve (phase 5g). SPDX-License-Identifier: GPL-3.0-or-later
--
-- Junctur installs this into Resolve's Scripts folder. Run it once per Resolve session
-- (Workspace > Scripts > Junctur) and leave it running: when Junctur's "Send to Resolve" writes a
-- request into its mailbox, this places or restyles the captions as Text+ clips.
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

local function folder_id(f)
    local ok, id = pcall(function() return f:GetUniqueId() end)
    return ok and id or tostring(f)
end

-- The user's Fusion Title when one is named; else the first clip of the Junctur bin; else the
-- bundled bin imported, its plain Text+ kept in a Junctur bin and the rest of the import deleted
-- (only a folder the import itself made, never one that was there before).
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
    local before = {}
    for _, sub in ipairs(list(root:GetSubFolderList())) do before[folder_id(sub)] = true end
    local was = pool:GetCurrentFolder()
    pool:SetCurrentFolder(root)
    pcall(function() return pool:ImportFolderFromFile(TEMPLATE_BIN) end)
    local new
    for _, sub in ipairs(list(root:GetSubFolderList())) do
        if not before[folder_id(sub)] then new = sub end
    end
    if not new then
        if was then pool:SetCurrentFolder(was) end
        return nil, "Could not import Junctur's caption template into the media pool"
    end
    local basic
    for _, clip in ipairs(list(new:GetClipList())) do
        if clip:GetClipProperty("Clip Name") == "Basic Template" then basic = clip end
    end
    if not basic then
        if was then pool:SetCurrentFolder(was) end
        return nil, "Junctur's caption template bin had no Text+ in it"
    end
    bin = bin or pool:AddSubFolder(root, BIN)
    pool:MoveClips({ basic }, bin)
    pcall(function() basic:SetClipProperty("Clip Name", TEMPLATE_NAME) end)
    local rest = list(new:GetClipList())
    if #rest > 0 then pool:DeleteClips(rest) end
    pool:DeleteFolders({ new })
    if was then pcall(function() pool:SetCurrentFolder(was) end) end
    return basic
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

-- An input a template lacks (a user's own title) is skipped, not an error.
local function set(tool, key, value)
    pcall(function() tool:SetInput(key, value) end)
end

local function restyle(tool, look, c)
    set(tool, "StyledText", c.text)
    set(tool, "Font", look.font)
    set(tool, "Style", look.style)
    set(tool, "Size", look.size)
    set(tool, "Center", { c.x, c.y })
    set(tool, "CharacterOffset", { 0, 0 })
    set(tool, "Enabled1", 1)
    set(tool, "Red1", c.fill[1])
    set(tool, "Green1", c.fill[2])
    set(tool, "Blue1", c.fill[3])
    set(tool, "Underline", look.underline and 1 or 0)
    set(tool, "Enabled2", look.outline and 1 or 0)
    if look.outline then
        set(tool, "Red2", look.outlineColor[1])
        set(tool, "Green2", look.outlineColor[2])
        set(tool, "Blue2", look.outlineColor[3])
        set(tool, "Thickness2", look.thickness)
    end
end

local handlers = {}

function handlers.info(req)
    local project, timeline = current()
    if not project then return { error = timeline } end
    local clip, err = template(project:GetMediaPool(), req.template or "")
    if not clip then return { error = err } end
    return {
        name = timeline:GetName(),
        fps = timeline:GetSetting("timelineFrameRate"),
        start = timeline:GetStartFrame(),
        ["end"] = timeline:GetEndFrame(),
        width = timeline:GetSetting("timelineResolutionWidth"),
        height = timeline:GetSetting("timelineResolutionHeight"),
        template = clip:GetClipProperty("Clip Name"),
        templateFps = clip:GetClipProperty("FPS") or "",
    }
end

function handlers.place(req)
    local project, timeline = current()
    if not project then return { error = timeline } end
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
            timeline:AddTrack("video")
            count = timeline:GetTrackCount("video")
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

    local placed, restyled, failed, first = 0, 0, 0, nil
    local todo, kept = {}, {}
    for _, c in ipairs(req.clips) do
        local old = sent[c.key]
        if old then
            kept[c.key] = true
            restyle(old.tool, req.look, c)
            restyled = restyled + 1
        else
            todo[#todo + 1] = c
        end
    end
    for key, old in pairs(sent) do
        if not kept[key] then stale[#stale + 1] = old.item end
    end
    if #stale > 0 then pcall(function() timeline:DeleteClips(stale, false) end) end

    -- A locked track refuses clips: unlocked for the append, locked again after.
    local locked = {}
    for _, i in pairs(track) do
        local ok, is = pcall(function() return timeline:GetIsTrackLocked("video", i) end)
        if ok and is then
            locked[#locked + 1] = i
            timeline:SetTrackLock("video", i, false)
        end
    end
    local infos = {}
    for k, c in ipairs(todo) do
        infos[k] = {
            mediaPoolItem = clip,
            mediaType = 1,
            startFrame = 0,
            endFrame = c.length,
            recordFrame = c.start,
            trackIndex = track[c.track],
        }
    end
    local items = {}
    if #infos > 0 then
        local ok, got = pcall(function() return pool:AppendToTimeline(infos) end)
        if ok and type(got) == "table" and #got == #infos then
            items = got
        else
            -- One refused clip can fail the whole batch: one at a time instead.
            for k, info in ipairs(infos) do
                local ok1, one = pcall(function() return pool:AppendToTimeline({ info }) end)
                if ok1 and type(one) == "table" then items[k] = one[1] end
            end
        end
    end
    for _, i in ipairs(locked) do timeline:SetTrackLock("video", i, true) end

    for k, c in ipairs(todo) do
        local item = items[k]
        local ok, e = pcall(function()
            -- A refused append can hand back an item with nothing behind it.
            if not item or not item:GetStart() then
                error("Resolve did not place the caption at frame " .. c.start
                    .. " (is another clip in the way?)")
            end
            local tool = textplus(item)
            if not tool then error("the template has no Text+ in it") end
            restyle(tool, req.look, c)
            tool:SetData("JuncturRecording", req.recording)
            tool:SetData("JuncturKey", c.key)
        end)
        if ok then
            placed = placed + 1
        else
            failed = failed + 1
            first = first or tostring(e)
        end
    end
    return {
        placed = placed,
        restyled = restyled,
        removed = #stale,
        failed = failed,
        firstError = first or "",
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
