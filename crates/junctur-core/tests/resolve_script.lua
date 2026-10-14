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
    local function Item(info)
        local tool = Tool(info.mediaPoolItem.lacks)
        local comp = {}
        function comp:FindTool(n) if n == "Template" then return tool end end
        function comp:FindToolByID() return tool end
        local it = { tool = tool, start = info.recordFrame, length = info.endFrame,
            template = info.mediaPoolItem }
        function it:GetFusionCompCount() return 1 end
        function it:GetFusionCompByIndex() return comp end
        function it:GetStart() return self.start end
        function it:GetEnd() return self.start + self.length end
        return it
    end

    -- A timeline with its own tracks; `w.tracks` stays the first one's ("Main", the one open).
    local function Timeline(name, id, frames)
        local tracks = { { name = "Video 1", items = {} } }
        local audio = {}
        local timeline = { name = name, id = id, tracks = tracks, audio = audio, frames = frames }
        local function kind(k) return k == "audio" and audio or tracks end
        function timeline:GetTrackCount(k) return #kind(k) end
        function timeline:GetTrackName(k, i) return kind(k)[i].name end
        function timeline:AddTrack()
            if opts.no_tracks then return false end
            tracks[#tracks + 1] = { name = "", items = {} }
            return true
        end
        function timeline:SetTrackName(_, i, n) tracks[i].name = n return true end
        function timeline:GetItemListInTrack(k, i) return kind(k)[i].items end
        function timeline:DeleteClips(list)
            if opts.no_delete then return false end
            for _, d in ipairs(list) do for _, t in ipairs(tracks) do drop(t.items, d) end end
            return true
        end
        function timeline:GetIsTrackLocked(_, i) return tracks[i].locked == true end
        function timeline:SetTrackLock(_, i, v) tracks[i].locked = v return true end
        function timeline:GetName() return self.name end
        function timeline:SetName(n)
            if opts.no_rename then return false end
            self.name = n
            return true
        end
        function timeline:GetUniqueId() return (self == w.main and w.timeline_id) or self.id end
        function timeline:GetSetting(k)
            return ({ timelineFrameRate = "24", timelineResolutionWidth = "1920",
                timelineResolutionHeight = "1080" })[k]
        end
        function timeline:GetStartFrame() return self.frames and 0 or 86400 end
        function timeline:GetEndFrame() return self.frames or 90000 end
        return timeline
    end
    w.main = Timeline("Main", "T-1")
    w.timeline = w.main
    w.tracks = w.main.tracks
    w.timelines = { w.main }
    w.current = w.main
    local function tracks_now() return w.current.tracks end

    local function Clip(name, lacks, path)
        local c = { name = name, lacks = lacks, path = path }
        -- `props` overrides a property (Type, In, Out); `marks` is what GetMarkInOut gives.
        function c:GetClipProperty(k)
            if self.props and self.props[k] ~= nil then return self.props[k] end
            if k == "Clip Name" then return self.name elseif k == "FPS" then return "24"
            elseif k == "File Path" then return self.path or ""
            elseif k == "Type" then return self.path and "Video + Audio" or "Fusion Title"
            elseif k == "In" or k == "Out" then return "" end
        end
        function c:GetMarkInOut() return self.marks or {} end
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
        f.clips = { Clip("Basic Template"), Clip("AutoSubs Caption 2026-09-16") }
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
        local tracks = tracks_now()
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
    -- 5z: a timeline made from a file. Importing the source clips puts the recording in the
    -- current folder; linking finds it only in the folders handed over (or, opts.link_fails, not
    -- at all).
    w.made = {}
    function pool:ImportTimelineFromFile(file, o)
        w.made[#w.made + 1] = { file = file, opts = o }
        if opts.no_import then return nil end
        if o.importSourceClips == false then
            local found = false
            for _, f in ipairs(o.sourceClipsFolders or {}) do
                for _, c in ipairs(f.clips) do
                    if c.path then found = true end
                end
            end
            if opts.link_fails or not found then return nil end
        else
            table.insert(self.current.clips, Clip("rec.mp4", nil, w.source))
        end
        for _, t in ipairs(w.timelines) do
            assert(t.name ~= o.timelineName, "imported under a taken name")
        end
        w.ids = w.ids + 1
        local t = Timeline(o.timelineName, "T-" .. w.ids, 2400)
        w.timelines[#w.timelines + 1] = t
        return t
    end
    w.ids = 1
    w.Clip = Clip
    function pool:DeleteTimelines(list)
        if opts.no_delete_timelines then return false end
        for _, t in ipairs(list) do drop(w.timelines, t) end
        return true
    end
    w.source = [[C:\Videos\rec.mp4]]
    local project = {}
    w.project = project
    function project:GetCurrentTimeline() return w.current end
    function project:SetCurrentTimeline(t)
        if opts.no_switch then return false end
        w.current = t
        return true
    end
    function project:GetTimelineCount()
        if opts.no_count then error("no such method") end
        return #w.timelines
    end
    function project:GetTimelineByIndex(i) return w.timelines[i] end
    function project:GetMediaPool() return pool end
    local resolve = {}
    function resolve:GetProjectManager()
        return { GetCurrentProject = function() if not w.no_project then return project end end }
    end

    local chunk = assert(loadfile(SCRIPT))
    setfenv(chunk, setmetatable({ JUNCTUR_TEST = true, fusion = fusion, resolve = resolve },
        { __index = _G }))
    w.m = chunk()
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

-- A build request (5z) as Junctur sends it.
local function build(extra)
    local r = { file = [[C:\J\timeline.fcpxml]], name = "rec (Junctur)", replace = false,
        source = [[c:/videos/REC.mp4]], captions = true, template = "" }
    for k, v in pairs(extra or {}) do r[k] = v end
    return r
end
local function names(w)
    local out = {}
    for _, t in ipairs(w.timelines) do out[#out + 1] = t.name end
    return table.concat(out, ", ")
end

-- 5z check: the project's timeline names; no project, or no count from Resolve, is an error
-- (never "no timelines").
local w = world()
local c = w.m.handlers.check({})
check(c.timelines == 1 and c.timeline1 == "Main", "check lists the timelines")
w.no_project = true
check(w.m.handlers.check({}).error ~= nil, "check without a project")
check(w.m.handlers.build(build()).error ~= nil, "build without a project")
w.no_project = nil
check(world({ no_count = true }).m.handlers.check({}).error ~= nil, "no count is an error")
local nw = world({ no_count = true })
check(nw.m.handlers.build(build()).error ~= nil and #nw.made == 0, "build without a count makes nothing")

-- 5z build: the timeline made from the file under the name, the current one now; the recording
-- imported by Resolve the first time. The template: the bundled bin imported once, its Text+ kept
-- in a Junctur bin, the import's folder gone, the user's own AutoSubs bin untouched.
local info = w.m.handlers.build(build())
check(info.error == nil, tostring(info.error))
check(info.name == "rec (Junctur)" and info.id == "T-2" and w.current.name == "rec (Junctur)", "made, current")
check(info.template == "Junctur Caption" and info.fps == "24" and info.start == 0 and info["end"] == 2400, "info")
check(w.made[1].file == [[C:\J\timeline.fcpxml]] and w.made[1].opts.timelineName == "rec (Junctur)", "the file")
check(w.made[1].opts.importSourceClips == true, "the recording imported the first time")
check(names(w) == "Main, rec (Junctur)" and w.main.name == "Main", "Main untouched")
check(#w.root.subs == 2 and w.root.subs[1] == w.theirs and #w.theirs.clips == 2, "their bin untouched")
check(w.root.subs[2].name == "Junctur" and #w.root.subs[2].clips == 1, "Junctur bin")
check(w.imports == 1, "template imported")
c = w.m.handlers.check({})
check(c.timelines == 2 and c.timeline2 == "rec (Junctur)", "check sees it")

-- The name taken and not replacing (another send took it meanwhile): refused, nothing made.
local r = w.m.handlers.build(build({ name = " REC (junctur)\t" }))
check(r.error ~= nil and r.error:find("nothing was changed") ~= nil and #w.made == 1, "a taken name is refused")

-- Create new: another timeline beside it; the recording already in the pool (its path written
-- with other slashes and ASCII case) is linked to, not imported again; the template not imported
-- again.
info = w.m.handlers.build(build({ name = "rec (Junctur) 2" }))
check(info.error == nil and info.name == "rec (Junctur) 2", tostring(info.error))
check(w.made[2].opts.importSourceClips == false and w.made[2].opts.sourceClipsFolders[1] == w.root,
    "the recording linked to")
check(#w.root.clips == 1 and w.imports == 1, "nothing imported twice")
check(names(w) == "Main, rec (Junctur), rec (Junctur) 2", "beside it")

-- Linking refused: Resolve imports the recording itself.
w = world({ link_fails = true })
w.m.handlers.build(build())
info = w.m.handlers.build(build({ name = "again" }))
check(info.error == nil and #w.made == 3 and w.made[3].opts.importSourceClips == true, "link failed, imported")

-- Only the whole recording is linked to: a sub-clip (a Type of its own), a clip with an In, one
-- with marks, or a path differing past ASCII case is passed over, and Resolve imports instead.
for _, odd in ipairs({ { props = { Type = "Subclip" } }, { props = { In = "00:00:10:00" } },
    { marks = { video = { ["in"] = 10, out = 20 } } }, { path = [[C:\Vidéos\rec.mp4]], source = [[C:\VIDÉOS\rec.mp4]] } }) do
    w = world()
    local clip = w.Clip("rec.mp4", nil, odd.path or w.source)
    clip.props, clip.marks = odd.props, odd.marks
    w.root.clips = { clip }
    info = w.m.handlers.build(build({ source = odd.source }))
    check(info.error == nil and w.made[1].opts.importSourceClips == true, "not the whole recording")
end
w = world()
w.source = [[C:\Vidéos\rec.mp4]]
w.root.clips = { w.Clip("rec.mp4", nil, w.source) }
w.m.handlers.build(build({ source = [[c:/vidéos/REC.mp4]] }))
check(w.made[1].opts.importSourceClips == false, "a non-ASCII path matches byte for byte, ASCII case aside")

-- Replace: build only makes the new one beside the old, under a free name, and opens it; the old
-- is still there (the captions come next). Then finish deletes the old and renames the new.
w = world()
w.m.handlers.build(build())
local old = w.current
w.m.handlers.build(build({ name = "rec (Junctur) (new)" }))
w.current = old
info = w.m.handlers.build(build({ replace = true }))
check(info.error == nil and info.name == "rec (Junctur) (new 2)", tostring(info.error))
check(w.current ~= old and w.current.name == "rec (Junctur) (new 2)", "the new one open")
check(names(w) == "Main, rec (Junctur), rec (Junctur) (new), rec (Junctur) (new 2)", "the old one kept: " .. names(w))
r = w.m.handlers.place(request({ clip(1, 0, "hola") }, { timeline = info.id }))
check(r.placed == 1 and #old.tracks == 1, "captions on the new one, the old untouched")
local fin = w.m.handlers.finish({ name = "rec (Junctur)", made = info.id })
check(fin.error == nil and fin.replaced == true and fin.note == "" and fin.name == "rec (Junctur)", tostring(fin.note))
check(names(w) == "Main, rec (Junctur) (new), rec (Junctur)", "the old one gone: " .. names(w))
check(w.current.tracks[2].items[1].start == 0, "the captions went with it")

-- Replace refused at build when two timelines have the name (as Junctur compares names): nothing made.
w = world()
w.m.handlers.build(build())
w.m.handlers.build(build({ name = "other" }))
w.timelines[#w.timelines].name = " REC (Junctur)	"
local made = #w.made
r = w.m.handlers.build(build({ replace = true }))
check(r.error ~= nil and r.error:find("2 timelines") ~= nil and #w.made == made, "two of the name: " .. tostring(r.error))

-- finish with the delete refused: both kept, the new one under its own name, the send says so.
w = world({ no_delete_timelines = true })
w.m.handlers.build(build())
info = w.m.handlers.build(build({ replace = true }))
fin = w.m.handlers.finish({ name = "rec (Junctur)", made = info.id })
check(fin.replaced == false and fin.name == "rec (Junctur) (new)", "delete refused")
check(fin.note:find("did not delete") ~= nil and names(w) == "Main, rec (Junctur), rec (Junctur) (new)", fin.note)

-- finish with the rename refused: the old one gone, the new one says its name.
w = world({ no_rename = true })
w.m.handlers.build(build())
info = w.m.handlers.build(build({ replace = true }))
fin = w.m.handlers.finish({ name = "rec (Junctur)", made = info.id })
check(fin.replaced == true and fin.note:find("did not rename") ~= nil, "rename refused")

-- finish when the old one is gone or doubled meanwhile, or the new one is gone: nothing deleted.
w = world()
w.m.handlers.build(build())
info = w.m.handlers.build(build({ replace = true }))
drop(w.timelines, w.timelines[2])
fin = w.m.handlers.finish({ name = "rec (Junctur)", made = info.id })
check(fin.replaced == false and fin.note:find("0 timelines") ~= nil, "the old one gone meanwhile")
check(w.m.handlers.finish({ name = "rec (Junctur)", made = "T-99" }).error ~= nil, "the new one gone")

-- The import refused: an error, the old timeline kept and still open; no template looked for
-- when there are no captions.
w = world({ no_import = true })
r = w.m.handlers.build(build({ captions = false }))
check(r.error ~= nil and w.current == w.main and names(w) == "Main" and w.imports == 0, "import refused")
w = world()
info = w.m.handlers.build(build({ captions = false }))
check(info.template == "" and w.imports == 0, "no captions, no template")

-- A missing named template stops the build before anything is made; the user's own is used.
w = world()
check(w.m.handlers.build(build({ template = "Nope" })).error ~= nil and #w.made == 0, "a missing template")
check(w.m.handlers.build(build({ template = "My Title" })).template == "My Title", "the user's own template")

-- The captions land on the made timeline, not on the one open before.
w = world()
info = w.m.handlers.build(build())
r = w.m.handlers.place(request({ clip(1, 0, "hola") }, { timeline = info.id }))
check(r.placed == 1 and #w.current.tracks == 3 and #w.main.tracks == 1, "captions on the made timeline")
check(w.current.tracks[2].items[1].start == 0, "at its frames")

-- 5ab open: the one timeline of the name made current, nothing on it changed, and where its own
-- cut ends (video and audio tracks), Junctur's caption tracks left out. Then place on it updates
-- only the captions: restyled, removed, placed; the cut's clips and the other timelines untouched.
w = world()
info = w.m.handlers.build(build())
local there = w.current
w.m.handlers.place(request({ clip(1, 0, "hola"), clip(2, 0, "bo"), clip(2, 3000, "late") },
    { timeline = info.id }))
local function cut(e)
    return { start = 0, GetStart = function(self) return self.start end,
        GetEnd = function() return e end, GetFusionCompCount = function() return 0 end }
end
local video, sound = cut(2000), cut(2010)
there.tracks[1].items = { video }
there.audio[1] = { name = "Audio 1", items = { sound } }
w.current = w.main
local made_before = #w.made
r = w.m.handlers.open({ name = " REC (junctur)", template = "" })
check(r.error == nil and r.id == info.id and r.name == "rec (Junctur)", tostring(r.error))
check(w.current == there, "made current")
check(r.cutEnd == 2010 and r.fps == "24" and r.template == "Junctur Caption", "cut end " .. tostring(r.cutEnd))
check(#w.made == made_before and names(w) == "Main, rec (Junctur)", "nothing made")
r = w.m.handlers.place(request({ clip(1, 0, "hola otra vez"), clip(1, 48, "nuevo") }, { timeline = r.id }))
check(r.restyled == 1 and r.removed == 2 and r.placed == 1 and r.failed == 0, "updated in place")
check(#there.tracks == 3 and there.tracks[1].items[1] == video and #there.tracks[1].items == 1, "the cut untouched")
check(there.audio[1].items[1] == sound and #w.main.tracks == 1, "audio and Main untouched")
check(there.tracks[2].items[1].tool.inputs.StyledText == "hola otra vez" and #there.tracks[3].items == 0, "captions")

-- open with no clips to measure answers no cut end; it refuses with nothing changed when there is
-- no project, the timeline is gone or doubled, the template is missing, or Resolve will not open it.
w = world()
w.m.handlers.build(build({ captions = false }))
w.current = w.main
r = w.m.handlers.open({ name = "rec (Junctur)", template = "" })
check(r.error == nil and r.cutEnd == "", "no cut end: " .. tostring(r.cutEnd))
w.current = w.main
r = w.m.handlers.open({ name = "gone (Junctur)", template = "" })
check(r.error ~= nil and r.error:find("no longer has") and w.current == w.main, "gone")
r = w.m.handlers.open({ name = "rec (Junctur)", template = "Nope" })
check(r.error ~= nil and r.error:find("nothing was changed") and w.current == w.main, "a missing template")
w.no_project = true
check(w.m.handlers.open({ name = "rec (Junctur)" }).error ~= nil, "open without a project")
w.no_project = nil
w.m.handlers.build(build({ name = "other", captions = false }))
w.timelines[#w.timelines].name = "REC (Junctur)"
w.current = w.main
r = w.m.handlers.open({ name = "rec (Junctur)", template = "" })
check(r.error ~= nil and r.error:find("2 timelines") and w.current == w.main, "two of the name")
w = world({ no_switch = true })
w.m.handlers.build(build({ captions = false }))
r = w.m.handlers.open({ name = "rec (Junctur)", template = "" })
check(r.error ~= nil and r.error:find("did not open") and w.current == w.main, "not opened")

-- 5g's place, on the timeline open.
w = world()
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

-- Folders without ids: the import is not trusted, nothing moved or deleted.
w = world({ no_ids = true })
info = w.m.handlers.build(build())
check(info.error ~= nil and w.imports == 0 and #w.made == 0, "no ids, no import, no timeline")
check(#w.root.subs == 1 and #w.theirs.clips == 2, "their bin untouched without ids")

print("ALL OK")
