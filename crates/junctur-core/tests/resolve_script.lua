-- The Junctur script (src/resolve/junctur.lua) run against a stand-in Resolve: a media pool, a
-- timeline with tracks, Text+ tools that record their inputs. Run by tests/resolve_script.rs with a
-- Lua 5.1 interpreter (lua, luajit, or Resolve's own fuscript), which puts `local SCRIPT = <path>`
-- above it.
-- The stand-ins are handed to the script as its environment (setfenv), as Resolve's script runner
-- hands it one: interpreters disagree on what a loaded chunk's globals are otherwise.

local prefs = {}
local fusion = {}
function fusion:SetPrefs(k, v) prefs[k] = v end
function fusion:SavePrefs() end
function fusion:GetPrefs(k) return prefs[k] end

local function Tool()
    local t = { inputs = {}, data = {} }
    function t:SetInput(k, v) self.inputs[k] = v end
    function t:GetData(k) return self.data[k] end
    function t:SetData(k, v) self.data[k] = v end
    return t
end

local function Item(info)
    local tool = Tool()
    local comp = {}
    function comp:FindTool(n) if n == "Template" then return tool end end
    function comp:FindToolByID() return tool end
    local it = { tool = tool, start = info.recordFrame, length = info.endFrame }
    function it:GetFusionCompCount() return 1 end
    function it:GetFusionCompByIndex() return comp end
    function it:GetStart() return self.start end
    return it
end

local tracks = { { name = "Video 1", items = {} } }
local timeline = {}
function timeline:GetTrackCount() return #tracks end
function timeline:GetTrackName(_, i) return tracks[i].name end
function timeline:AddTrack() tracks[#tracks + 1] = { name = "", items = {} } return true end
function timeline:SetTrackName(_, i, n) tracks[i].name = n return true end
function timeline:GetItemListInTrack(_, i) return tracks[i].items end
function timeline:DeleteClips(list)
    for _, d in ipairs(list) do
        for _, t in ipairs(tracks) do
            for k = #t.items, 1, -1 do if t.items[k] == d then table.remove(t.items, k) end end
        end
    end
    return true
end
function timeline:GetIsTrackLocked(_, i) return tracks[i].locked == true end
function timeline:SetTrackLock(_, i, v) tracks[i].locked = v return true end
function timeline:GetName() return "Main" end
function timeline:GetSetting(k)
    return ({ timelineFrameRate = "24", timelineResolutionWidth = "1920",
        timelineResolutionHeight = "1080" })[k]
end
function timeline:GetStartFrame() return 86400 end
function timeline:GetEndFrame() return 90000 end

local function Clip(name)
    local c = { name = name }
    function c:GetClipProperty(k)
        if k == "Clip Name" then return self.name elseif k == "FPS" then return "24" end
    end
    function c:SetClipProperty(_, v) self.name = v return true end
    return c
end
local function Folder(name, id)
    local f = { name = name, id = id, subs = {}, clips = {} }
    function f:GetSubFolderList() return self.subs end
    function f:GetClipList() return self.clips end
    function f:GetName() return self.name end
    function f:GetUniqueId() return self.id end
    return f
end
local root = Folder("Master", "0")
-- The user's own auto-subs bin, which an import must never touch.
local theirs = Folder("AutoSubs", "1")
theirs.clips = { Clip("Basic Template") }
local mine = Clip("My Title")
theirs.clips[2] = mine
root.subs = { theirs }

local function drop(list, x)
    for k = #list, 1, -1 do if list[k] == x then table.remove(list, k) end end
end
local pool = { current = root, imports = 0 }
function pool:GetRootFolder() return root end
function pool:GetCurrentFolder() return self.current end
function pool:SetCurrentFolder(f) self.current = f return true end
function pool:ImportFolderFromFile()
    self.imports = self.imports + 1
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
function pool:AppendToTimeline(infos)
    local out = {}
    for k, info in ipairs(infos) do
        assert(info.recordFrame == math.floor(info.recordFrame), "a non-integer frame")
        assert(not tracks[info.trackIndex].locked, "appended to a locked track")
        local it = Item(info)
        it.template = info.mediaPoolItem
        table.insert(tracks[info.trackIndex].items, it)
        out[k] = it
    end
    return out
end
local project = {}
function project:GetCurrentTimeline() return timeline end
function project:GetMediaPool() return pool end
local resolve = {}
function resolve:GetProjectManager()
    return { GetCurrentProject = function() return project end }
end

local chunk = assert(loadfile(SCRIPT))
setfenv(chunk, setmetatable({ JUNCTUR_TEST = true, fusion = fusion, resolve = resolve },
    { __index = _G }))
local m = chunk()

local function check(cond, what)
    if not cond then error("FAILED: " .. what, 2) end
end

-- info: the bundled bin imported once, its Text+ kept in a Junctur bin, the import's folder gone,
-- the user's own AutoSubs bin untouched.
local info = m.handlers.info({ template = "" })
check(info.error == nil, tostring(info.error))
check(info.template == "Junctur Caption" and info.fps == "24" and info.start == 86400, "info")
check(#root.subs == 2 and root.subs[1] == theirs and #theirs.clips == 2, "their bin untouched")
check(root.subs[2].name == "Junctur" and #root.subs[2].clips == 1, "Junctur bin")
m.handlers.info({ template = "" })
check(pool.imports == 1, "imported once")
check(m.handlers.info({ template = "Nope" }).error ~= nil, "a missing template is an error")
check(m.handlers.info({ template = "My Title" }).template == "My Title", "the user's own template")

local look = { font = "Open Sans", style = "Bold", size = 0.03, underline = true, outline = true,
    outlineColor = { 0, 0, 0 }, thickness = 0.04 }
local function clip(track, start, text)
    local names = { "Junctur Ana", "Junctur Bo" }
    return { track = track, key = names[track] .. "|" .. start .. "|" .. (start + 24),
        start = start, ["end"] = start + 24, length = 24, text = text,
        fill = { 1, 0, 0 }, x = 0.5, y = 0.1 * track }
end
local req = { recording = "abc", template = "", tracks = { "Junctur Ana", "Junctur Bo" },
    look = look, clips = { clip(1, 86400, "hola\ntodos"), clip(2, 86400, "qué tal") } }

local r = m.handlers.place(req)
check(r.placed == 2 and r.restyled == 0 and r.removed == 0 and r.failed == 0, "first send")
check(#tracks == 3 and tracks[2].name == "Junctur Ana" and tracks[3].name == "Junctur Bo", "tracks")
local ana = tracks[2].items[1]
check(ana.start == 86400 and ana.length == 24, "frames")
check(ana.template.name == "Junctur Caption", "placed from the template")
local t = ana.tool
check(t.inputs.StyledText == "hola\ntodos" and t.inputs.Style == "Bold" and t.inputs.Size == 0.03, "text")
check(t.inputs.Center[1] == 0.5 and t.inputs.Center[2] == 0.1, "centre")
check(t.inputs.CharacterOffset[2] == 0, "template's offset cleared")
check(t.inputs.Red1 == 1 and t.inputs.Green1 == 0 and t.inputs.Enabled2 == 1, "colours")
check(t.inputs.Thickness2 == 0.04 and t.inputs.Underline == 1, "outline and underline")
check(t.data.JuncturRecording == "abc" and t.data.JuncturKey == "Junctur Ana|86400|86424", "tag")
check(tracks[3].items[1].tool.inputs.Center[2] == 0.2, "Bo above")

-- Sent again with Ana's caption changed, Bo's gone and a new one: restyled in place, removed,
-- placed; no new tracks; a locked track unlocked for the append and locked again.
tracks[2].locked = true
req.clips = { clip(1, 86400, "hola a todos"), clip(1, 86430, "otra") }
req.look = { font = "Arial", style = "Regular", size = 0.04, underline = false, outline = false }
r = m.handlers.place(req)
check(r.restyled == 1 and r.removed == 1 and r.placed == 1 and r.failed == 0, "second send")
check(#tracks == 3 and #tracks[2].items == 2 and #tracks[3].items == 0, "updated in place")
check(tracks[2].items[1] == ana and t.inputs.StyledText == "hola a todos", "same clip, new text")
check(t.inputs.Font == "Arial" and t.inputs.Enabled2 == 0 and t.inputs.Underline == 0, "restyled")
check(tracks[2].locked == true, "locked again")

-- Another recording's clips are left alone.
local other = { recording = "zzz", template = "", tracks = { "Junctur Ana" }, look = look,
    clips = { clip(1, 87000, "x") } }
r = m.handlers.place(other)
check(r.placed == 1 and r.removed == 0 and #tracks[2].items == 3, "another recording")

-- No project, no timeline.
function project:GetCurrentTimeline() return nil end
check(m.handlers.place(req).error ~= nil, "no timeline")

-- The reply: tabs and line breaks in values flattened, hex of key<TAB>value lines.
m.reply("9", { a = "x\ty\nz" })
check(prefs["Global.JuncturBridge.JuncturReply"] == "9:" .. ("a\tx y z"):gsub(".", function(c)
    return string.format("%02x", c:byte())
end), "reply")

print("ALL OK")
