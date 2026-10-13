-- The Junctur script (src/resolve/junctur.lua) run against a stand-in Resolve: a media pool, a
-- timeline with tracks, Text+ tools that record their inputs. Run by tests/resolve_script.rs with a
-- Lua 5.1 interpreter (lua, luajit, or Resolve's own fuscript), which puts `local SCRIPT = <path>`
-- above it.
-- The stand-ins are handed to the script as its environment (setfenv), as Resolve's script runner
-- hands it one: interpreters disagree on what a loaded chunk's globals are otherwise.
-- Each `world` is a fresh Resolve with the script loaded into it; `opts` turn on its failures.

local function drop(list, x)
    for k = #list, 1, -1 do if list[k] == x then table.remove(list, k) end end
end

local function hex(s)
    return (s:gsub(".", function(c) return string.format("%02x", c:byte()) end))
end

local function check(cond, what)
    if not cond then error("FAILED: " .. what, 2) end
end

local function world(opts)
    opts = opts or {}
    local w = { prefs = {}, tracks = { { name = "Video 1", items = {} } }, imports = 0 }

    local fusion = {}
    function fusion:SetPrefs(k, v) w.prefs[k] = v end
    function fusion:SavePrefs() end
    function fusion:GetPrefs(k) return w.prefs[k] end

    -- A Text+ tool; inputs in `lacks` are not there (set does nothing, get gives nil).
    local function Tool(lacks)
        local t = { inputs = {}, data = {} }
        function t:SetInput(k, v) if not (lacks and lacks[k]) then self.inputs[k] = v end end
        function t:GetInput(k) return self.inputs[k] end
        function t:GetData(k) return self.data[k] end
        function t:SetData(k, v) self.data[k] = v end
        return t
    end
    -- The animated caption's macro (5l): its helper records the settings it is handed, and the
    -- Text+ records what word timing the macro held when its Text was set.
    local HELPER = [[
        return function(comp, tool, settings)
            for k, v in pairs(settings) do tool:SetInput(k, v) end
            tool.applied = (tool.applied or 0) + 1
        end
    ]]
    local function Item(info)
        local tool = Tool(info.mediaPoolItem.lacks)
        local comp = {}
        local macro, cls
        if info.mediaPoolItem.macro then
            macro = Tool()
            cls = Tool()
            if not opts.no_helper then macro.data.SetInputValues = HELPER end
            local set = tool.SetInput
            function tool:SetInput(k, v)
                if k == "Text" then self.timing = macro.data.WordTiming end
                set(self, k, v)
            end
        end
        function comp:FindTool(n)
            if n == "Template" then return tool end
            if n == "AutoSubs" then return macro end
            if n == "CharacterLevelStyling1" then return cls end
        end
        function comp:FindToolByID() return tool end
        local it = { tool = tool, macro = macro, cls = cls, start = info.recordFrame,
            length = info.endFrame, template = info.mediaPoolItem }
        function it:GetFusionCompCount() return 1 end
        function it:GetFusionCompByIndex() return comp end
        function it:GetStart() return self.start end
        return it
    end

    local tracks = w.tracks
    local timeline = {}
    w.timeline = timeline
    function timeline:GetTrackCount() return #tracks end
    function timeline:GetTrackName(_, i) return tracks[i].name end
    function timeline:AddTrack()
        if opts.no_tracks then return false end
        tracks[#tracks + 1] = { name = "", items = {} }
        return true
    end
    function timeline:SetTrackName(_, i, n) tracks[i].name = n return true end
    function timeline:GetItemListInTrack(_, i) return tracks[i].items end
    function timeline:DeleteClips(list)
        if opts.no_delete then return false end
        for _, d in ipairs(list) do for _, t in ipairs(tracks) do drop(t.items, d) end end
        return true
    end
    function timeline:GetIsTrackLocked(_, i) return tracks[i].locked == true end
    function timeline:SetTrackLock(_, i, v) tracks[i].locked = v return true end
    function timeline:GetName() return "Main" end
    function timeline:GetUniqueId() return w.timeline_id or "T-1" end
    function timeline:GetSetting(k)
        return ({ timelineFrameRate = "24", timelineResolutionWidth = "1920",
            timelineResolutionHeight = "1080" })[k]
    end
    function timeline:GetStartFrame() return 86400 end
    function timeline:GetEndFrame() return 90000 end

    local function Clip(name, lacks, macro)
        local c = { name = name, lacks = lacks, macro = macro }
        function c:GetClipProperty(k)
            if k == "Clip Name" then return self.name elseif k == "FPS" then return "24" end
            if k == "Type" then return self.kind or "Fusion Title" end
        end
        function c:SetClipProperty(_, v) self.name = v return true end
        return c
    end
    local function Folder(name, id)
        local f = { name = name, id = id, subs = {}, clips = {} }
        function f:GetSubFolderList() return self.subs end
        function f:GetClipList() return self.clips end
        function f:GetName() return self.name end
        function f:GetUniqueId()
            if opts.no_ids then error("no such method") end
            return self.id
        end
        return f
    end
    local root = Folder("Master", "0")
    w.root = root
    -- The user's own auto-subs bin, which an import must never touch, and a title of their own
    -- that has no Underline.
    w.theirs = Folder("AutoSubs", "1")
    w.theirs.clips = { Clip("Basic Template"), Clip("My Title", { Underline = true }) }
    root.subs = { w.theirs }

    local pool = { current = root }
    function pool:GetRootFolder() return root end
    function pool:GetCurrentFolder() return self.current end
    function pool:SetCurrentFolder(f) self.current = f return true end
    function pool:ImportFolderFromFile()
        w.imports = w.imports + 1
        local f = Folder("AutoSubs", "2")
        f.clips = { Clip("Basic Template"), Clip("AutoSubs Caption 2026-09-16", nil, true),
            Clip("Other") }
        table.insert(self.current.subs, f)
        return true
    end
    function pool:AddSubFolder(parent, name)
        local f = Folder(name, "3")
        table.insert(parent.subs, f)
        return f
    end
    function pool:MoveClips(clips, to)
        for _, c in ipairs(clips) do
            for _, f in ipairs(root.subs) do drop(f.clips, c) end
            table.insert(to.clips, c)
        end
        return true
    end
    function pool:DeleteClips(clips)
        for _, c in ipairs(clips) do for _, f in ipairs(root.subs) do drop(f.clips, c) end end
        return true
    end
    function pool:DeleteFolders(fs)
        for _, d in ipairs(fs) do drop(root.subs, d) end
        return true
    end
    -- Places what does not collide with a clip already at that frame on that track; returns the
    -- placed items, or (opts.short) nothing at all for a batch, though it placed them.
    w.appends = 0
    function pool:AppendToTimeline(infos)
        w.appends = w.appends + 1
        local out = {}
        for _, info in ipairs(infos) do
            assert(info.recordFrame == math.floor(info.recordFrame), "a non-integer frame")
            assert(not tracks[info.trackIndex].locked, "appended to a locked track")
            local busy = false
            for _, it in ipairs(tracks[info.trackIndex].items) do
                if it.start == info.recordFrame then busy = true end
            end
            if not busy then
                local it = Item(info)
                table.insert(tracks[info.trackIndex].items, it)
                out[#out + 1] = it
            end
        end
        if opts.short and #infos > 1 then return {} end
        return out
    end
    local project = {}
    w.project = project
    function project:GetCurrentTimeline() return timeline end
    function project:GetMediaPool() return pool end
    local resolve = {}
    function resolve:GetProjectManager()
        return { GetCurrentProject = function()
            if opts.no_project then return nil end
            return project
        end }
    end

    local chunk = assert(loadfile(SCRIPT))
    setfenv(chunk, setmetatable({ JUNCTUR_TEST = true, fusion = fusion, resolve = resolve },
        { __index = _G }))
    w.m = chunk()
    w.Folder, w.Clip = Folder, Clip
    return w
end

local look = { font = "Open Sans", style = "Bold", size = 0.03, underline = true, outline = true,
    outlineColor = { 0, 0, 0 }, thickness = 0.04 }
local function clip(track, start, text)
    local names = { "Junctur 1 Ana", "Junctur 2 Bo" }
    return { track = track, key = names[track] .. "|" .. start .. "|" .. (start + 24),
        start = start, ["end"] = start + 24, length = 24, text = text,
        fill = { 1, 0, 0 }, x = 0.5, y = 0.1 * track }
end
local function request(clips, extra)
    local r = { recording = "abc", template = "", timeline = "T-1",
        tracks = { "Junctur 1 Ana", "Junctur 2 Bo" }, look = look, clips = clips }
    for k, v in pairs(extra or {}) do r[k] = v end
    return r
end

-- info: the bundled bin imported once, its Text+ kept in a Junctur bin, the import's folder gone,
-- the user's own AutoSubs bin untouched.
local w = world()
local info = w.m.handlers.info({ template = "" })
check(info.error == nil, tostring(info.error))
check(info.template == "Junctur Caption" and info.fps == "24" and info.start == 86400, "info")
check(info.id == "T-1", "timeline id")
check(#w.root.subs == 2 and w.root.subs[1] == w.theirs and #w.theirs.clips == 2, "their bin untouched")
check(w.root.subs[2].name == "Junctur" and #w.root.subs[2].clips == 2, "Junctur bin")
check(w.root.subs[2].clips[1].name == "Junctur Caption", "plain template renamed")
check(w.root.subs[2].clips[2].name == "Junctur Animated Caption", "animated template kept (5l)")
w.m.handlers.info({ template = "" })
check(w.imports == 1, "imported once")
info = w.m.handlers.info({ template = "", animated = true })
check(info.template == "Junctur Animated Caption" and w.imports == 1, "animated template, no import")
check(w.m.handlers.info({ template = "Nope" }).error ~= nil, "a missing template is an error")
check(w.m.handlers.info({ template = "My Title" }).template == "My Title", "the user's own template")

local tracks = w.tracks
local r = w.m.handlers.place(request({ clip(1, 86400, "hola\ntodos"), clip(2, 86400, "qué tal") }))
check(r.placed == 2 and r.restyled == 0 and r.removed == 0 and r.failed == 0, "first send")
check(r.unstyled == "", "every input taken")
check(#tracks == 3 and tracks[2].name == "Junctur 1 Ana" and tracks[3].name == "Junctur 2 Bo", "tracks")
local ana = tracks[2].items[1]
check(ana.start == 86400 and ana.length == 24, "frames")
check(ana.template.name == "Junctur Caption", "placed from the template")
local t = ana.tool
check(t.inputs.StyledText == "hola\ntodos" and t.inputs.Style == "Bold" and t.inputs.Size == 0.03, "text")
check(t.inputs.Center[1] == 0.5 and t.inputs.Center[2] == 0.1, "centre")
check(t.inputs.CharacterOffset[2] == 0, "template's offset cleared")
check(t.inputs.Red1 == 1 and t.inputs.Green1 == 0 and t.inputs.Enabled2 == 1, "colours")
check(t.inputs.Thickness2 == 0.04 and t.inputs.Underline == 1, "outline and underline")
check(t.data.JuncturRecording == "abc" and t.data.JuncturKey == "Junctur 1 Ana|86400|86424", "tag")
check(tracks[3].items[1].tool.inputs.Center[2] == 0.2, "Bo above")

-- Sent again with Ana's caption changed, Bo's gone and a new one: restyled in place, removed,
-- placed; no new tracks; a locked track unlocked for the append and locked again.
tracks[2].locked = true
r = w.m.handlers.place(request({ clip(1, 86400, "hola a todos"), clip(1, 86430, "otra") },
    { look = { font = "Arial", style = "Regular", size = 0.04, underline = false, outline = false } }))
check(r.restyled == 1 and r.removed == 1 and r.placed == 1 and r.failed == 0, "second send")
check(#tracks == 3 and #tracks[2].items == 2 and #tracks[3].items == 0, "updated in place")
check(tracks[2].items[1] == ana and t.inputs.StyledText == "hola a todos", "same clip, new text")
check(t.inputs.Font == "Arial" and t.inputs.Enabled2 == 0 and t.inputs.Underline == 0, "restyled")
check(tracks[2].locked == true, "locked again")

-- Another recording's clips are left alone.
r = w.m.handlers.place(request({ clip(1, 87000, "x") }, { recording = "zzz", tracks = { "Junctur 1 Ana" } }))
check(r.placed == 1 and r.removed == 0 and #tracks[2].items == 3, "another recording")

-- The timeline switched between info and place: refused, nothing touched.
w.timeline_id = "T-2"
r = w.m.handlers.place(request({ clip(1, 88000, "y") }))
check(r.error ~= nil and #tracks[2].items == 3, "another timeline")
w.timeline_id = nil

-- A user's own title lacking an input: placed, and the input named in the reply.
r = w.m.handlers.place(request({ clip(1, 88000, "z") }, { template = "My Title", recording = "own" }))
check(r.placed == 1 and r.failed == 0 and r.unstyled == "Underline", "unstyled input: " .. tostring(r.unstyled))

-- No timeline.
local current = w.project.GetCurrentTimeline
function w.project:GetCurrentTimeline() return nil end
check(w.m.handlers.place(request({})).error ~= nil, "no timeline")
w.project.GetCurrentTimeline = current

-- The reply: tabs and line breaks in values flattened, hex of key<TAB>value lines.
w.m.reply("9", { a = "x\ty\nz" })
check(w.prefs["Global.JuncturBridge.JuncturReply"] == "9:" .. hex("a\tx y z"), "reply")

-- A batch Resolve placed but reported short: the clips are found on the track and tagged, none
-- appended twice.
w = world({ short = true })
r = w.m.handlers.place(request({ clip(1, 86400, "a"), clip(1, 86430, "b"), clip(2, 86400, "c") }))
check(r.placed == 3 and r.failed == 0, "short batch: " .. tostring(r.firstError))
check(#w.tracks[2].items == 2 and #w.tracks[3].items == 1, "nothing placed twice")
check(w.appends == 1, "no retries for clips already there")
for _, it in ipairs(w.tracks[2].items) do check(it.tool.data.JuncturKey ~= nil, "tagged") end

-- A clip in the way: that one fails, the rest are placed.
w = world()
local blocker = { start = 86430, GetStart = function(self) return self.start end,
    GetFusionCompCount = function() return 0 end }
w.m.handlers.place(request({ clip(1, 86400, "a") }))
table.insert(w.tracks[2].items, blocker)
r = w.m.handlers.place(request({ clip(1, 86400, "a"), clip(1, 86430, "b"), clip(1, 86460, "c") }))
check(r.restyled == 1 and r.placed == 1 and r.failed == 1, "a clip in the way")
check(r.firstError:find("86430") ~= nil, tostring(r.firstError))

-- Resolve refuses a new track: an error, and none of the user's tracks renamed.
w = world({ no_tracks = true })
r = w.m.handlers.place(request({ clip(1, 86400, "a") }))
check(r.error ~= nil and w.tracks[1].name == "Video 1", "no new track")

-- Resolve refuses to delete: not counted as removed.
w = world({ no_delete = true })
w.m.handlers.place(request({ clip(1, 86400, "a") }))
r = w.m.handlers.place(request({ clip(1, 86500, "b") }))
check(r.removed == 0 and r.firstError:find("delete") ~= nil, "failed delete: " .. tostring(r.firstError))

-- 5l: the word highlight and the animation through auto-subs' macro.
local function animated(track, start, text, extra)
    local c = clip(track, start, text)
    c.key = c.key .. "|highlight"
    c.base, c.highlight = { 1, 1, 1 }, { 1, 0, 0 }
    c.words = { { startIndex = 0, endIndex = 3, startFrame = 0, endFrame = 12 },
        { startIndex = 4, endIndex = 9, startFrame = 12, endFrame = 24 } }
    for k, v in pairs(extra or {}) do c[k] = v end
    return c
end
local moving = { font = "Open Sans", style = "Bold", size = 0.03, underline = true,
    outline = true, outlineColor = { 0, 0, 0 }, thickness = 0.04, highlight = true,
    animation = "pop", animationLength = 0.3 }

-- A Junctur bin from before 5l, the plain template only: the animated one is imported into it
-- and the plain one is not doubled.
w = world()
local bin = w.Folder("Junctur", "9")
bin.clips = { w.Clip("Junctur Caption") }
table.insert(w.root.subs, bin)
info = w.m.handlers.info({ template = "", animated = true })
check(info.error == nil and info.template == "Junctur Animated Caption", "animated: " .. tostring(info.error))
check(w.imports == 1 and #bin.clips == 2 and bin.clips[1].name == "Junctur Caption", "bin gains the animated one")
check(#w.root.subs == 2, "the import's folder deleted")

r = w.m.handlers.place(request({ animated(1, 86400, "hola todos") }, { look = moving, animated = true }))
check(r.placed == 1 and r.failed == 0, "animated send: " .. tostring(r.firstError))
check(r.unstyled == "", "every macro input taken: " .. tostring(r.unstyled))
local it = w.tracks[2].items[1]
check(it.template.name == "Junctur Animated Caption" and it.macro ~= nil, "placed from the macro")
check(it.tool.timing ~= nil and it.tool.timing[2].startIndex == 4, "word timing set before the text")
check(it.tool.inputs.Text == "hola todos" and it.cls.inputs.Text == "hola todos", "text and styling text")
local m = it.macro.inputs
check(it.macro.applied == 1, "the macro's own helper ran")
check(m.HighlightEnabled == 1 and m.HighlightStyle == 0 and m.HighlightColorRed == 1
    and m.HighlightColorGreen == 0, "highlight")
check(m.FillColorGreen == 1 and m.TextSize == 0.03 and m.TextPosition[2] == 0.1, "fill and place")
check(m.PopInEnabled == 1 and m.FadeEnabled == 0 and m.SlideUpEnabled == 0, "pop in")
check(m.AnimationLength == 0.3 and m.AnimationLevel == 0 and m.AnimationMode == 2, "in and out")
check(m.OutlineThickness == 0.04 and m.WrapEnabled == 0, "outline, no wrap")
check(it.tool.inputs.Underline == 1, "underline on the Text+")
check(it.tool.data.JuncturKey == "Junctur 1 Ana|86400|86424|highlight", "tagged")

-- Sent again: restyled in place through the macro.
moving.animation = "fade"
r = w.m.handlers.place(request({ animated(1, 86400, "hola amigos") }, { look = moving, animated = true }))
check(r.restyled == 1 and r.placed == 0 and w.tracks[2].items[1] == it, "animated restyle")
check(it.macro.applied == 2 and m.FadeEnabled == 1 and m.PopInEnabled == 0, "fade")
check(it.tool.inputs.Text == "hola amigos", "new text")

-- Turned off: the key differs, so the macro clip is replaced by a plain Text+.
r = w.m.handlers.place(request({ clip(1, 86400, "hola amigos") }))
check(r.removed == 1 and r.placed == 1, "back to plain")
check(w.tracks[2].items[1].template.name == "Junctur Caption", "plain again")

-- A plain Text+ of the user's asked to highlight and animate: placed as 5g, and said so.
r = w.m.handlers.place(request({ animated(1, 87000, "x") },
    { template = "My Title", recording = "own2", look = moving, animated = true }))
check(r.placed == 1 and r.unstyled == "Underline, animation, word highlight",
    "own template: " .. tostring(r.unstyled))

-- A macro that lost its helper: that caption fails, said so.
w = world({ no_helper = true })
r = w.m.handlers.place(request({ animated(1, 86400, "a") }, { look = moving, animated = true }))
check(r.failed == 1 and r.firstError:find("SetInputValues") ~= nil, "no helper: " .. tostring(r.firstError))

-- Folders without ids: the import is not trusted, nothing moved or deleted.
w = world({ no_ids = true })
info = w.m.handlers.info({ template = "" })
check(info.error ~= nil and w.imports == 0, "no ids, no import")
check(#w.root.subs == 1 and #w.theirs.clips == 2, "their bin untouched without ids")

-- 5m: the template picker's list. The user's titles each once (the first by the lookup's walk:
-- sub-bins first), Junctur's own two and clips that are no title left out.
w = world()
w.m.handlers.info({ template = "" })
local video = w.Clip("Interview.mp4")
video.kind = "Video"
local nested = w.Folder("Titles", "7")
nested.clips = { w.Clip("Lower third"), w.Clip("My Title") }
table.insert(w.theirs.subs, nested)
table.insert(w.root.clips, video)
table.insert(w.root.clips, w.Clip("Lower third"))
local list = w.m.handlers.templates()
local names = {}
for k = 1, list.count do names[k] = list["t" .. k] end
check(table.concat(names, ",") == "Lower third,My Title,Basic Template",
    "templates: " .. table.concat(names, ","))
check(w.m.handlers.info({ template = "" }).script == "5m", "the script says which it is")
-- No project open: an error, not an empty list.
check(world({ no_project = true }).m.handlers.templates().error ~= nil, "no project")

-- 5m: a picked template whose look is kept: only the text and the speaker's colour are set.
w = world()
local styled = w.Clip("Styled")
table.insert(w.theirs.clips, styled)
local keep = { font = "Open Sans", style = "Bold", size = 0.03, underline = true, outline = true,
    outlineColor = { 0, 0, 0 }, thickness = 0.04, keep = true }
local uncoloured = clip(2, 86400, "sin color")
uncoloured.colored = false
r = w.m.handlers.place(request({ clip(1, 86400, "hola"), uncoloured },
    { template = "Styled", look = keep }))
check(r.placed == 2 and r.failed == 0 and r.unstyled == "", "kept look: " .. tostring(r.firstError))
t = w.tracks[2].items[1].tool
check(t.inputs.StyledText == "hola" and t.inputs.Red1 == 1 and t.inputs.Green1 == 0, "text and colour")
for _, key in ipairs({ "Font", "Style", "Size", "Center", "CharacterOffset", "Underline",
    "Enabled1", "Enabled2", "Thickness2", "Red2" }) do
    check(t.inputs[key] == nil, "kept look, yet " .. key .. " set")
end
local u = w.tracks[3].items[1].tool
check(u.inputs.StyledText == "sin color" and u.inputs.Red1 == nil, "no speaker colour: the template's fill")

-- A copy of the animated caption picked, its look kept: the motion and the colour through the
-- macro, not the look.
w = world()
table.insert(w.theirs.clips, w.Clip("My Animated", nil, true))
local moving_kept = { font = "Open Sans", style = "Bold", size = 0.03, underline = true,
    outline = true, outlineColor = { 0, 0, 0 }, thickness = 0.04, highlight = true,
    animation = "fade", animationLength = 0.3, keep = true }
r = w.m.handlers.place(request({ animated(1, 86400, "hola todos") },
    { template = "My Animated", look = moving_kept, animated = true }))
check(r.placed == 1 and r.failed == 0 and r.unstyled == "", "kept macro: " .. tostring(r.firstError))
it = w.tracks[2].items[1]
m = it.macro.inputs
check(it.tool.timing ~= nil and it.tool.inputs.Text == "hola todos", "timing and text")
check(m.HighlightEnabled == 1 and m.FadeEnabled == 1 and m.FillColorRed == 1, "motion and colour")
for _, key in ipairs({ "Font", "Style", "TextSize", "TextPosition", "OutlineEnabled",
    "OutlineThickness", "WrapEnabled" }) do
    check(m[key] == nil, "kept look, yet the macro's " .. key .. " set")
end
check(it.tool.inputs.Underline == nil, "kept look, yet Underline set")

print("ALL OK")
