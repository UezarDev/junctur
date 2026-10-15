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

-- The bundled macro's own word timing code, as it is in caption-bin.drb (auto-subs, MIT,
-- tmoroney/auto-subs: see src/resolve/LICENSE-auto-subs): what it does with the WordTiming
-- Junctur hands it whenever the text is set.
UPDATE_TEXT_CONTENT = [==[
					return function(comp, tool, text)
						local function utf8len(s)
							local _, count = s:gsub("[^\128-\191]", "")
							return count
						end

						-- Split text into words
						local words = {}
						for word in text:gmatch("%S+") do
							table.insert(words, word)
						end

						local wordTiming = tool:GetData("WordTiming")
						if not wordTiming then return end

						local oldWordCount = #wordTiming
						local newWordCount = #words
						local hasExistingWords = oldWordCount > 0

						-- Determine a frames-per-character rate to dynamically estimate new word durations.
						local defaultFrameRate = tonumber(comp:GetPrefs("Comp.FrameFormat.Rate")) or 30
						-- Default speaking speed: ~15 characters per second
						local defaultFramesPerChar = defaultFrameRate / 15
						local framesPerChar = defaultFramesPerChar

						-- Calculate pacing from existing words if available
						if hasExistingWords then
							local totalFrames = 0
							local totalChars = 0
							-- Sum up the durations and lengths of existing words to analyze the speaker's true pacing.
							for j = 1, oldWordCount do
								local duration = wordTiming[j].endFrame - wordTiming[j].startFrame
								local length = wordTiming[j].endIndex - wordTiming[j].startIndex + 1
								-- Only average words with valid timings and non-zero character counts.
								if duration > 0 and length > 0 then
									totalFrames = totalFrames + duration
									totalChars = totalChars + length
								end
							end
							
							-- If we have valid historical pacing data, use it.
							if totalChars > 0 then
								framesPerChar = totalFrames / totalChars
							end
						end

						-- If words were removed, delete their corresponding timing data from the end of the list first.
						if newWordCount < oldWordCount then
							for i = oldWordCount, newWordCount + 1, -1 do
								table.remove(wordTiming, i)
							end
						end

						-- Traverse all words to add new timings and recompute absolute character indices.
						local currentFrame = hasExistingWords and wordTiming[oldWordCount].endFrame or 0
						local charIndex = 0

						for i = 1, newWordCount do
							local wordText = words[i]
							-- Reconstruct spaces between words to match the combined subtitle text structure.
							if i > 1 then
								wordText = " " .. wordText
							end

							-- Use UTF-8 safe length to accurately handle multi-byte characters and map index boundaries.
							local length = utf8len(wordText)

							if i > oldWordCount then
								-- Estimate timing for newly added words scaled by their character length.
								local startFrame = currentFrame
								local wordLength = utf8len(words[i])
								local duration = math.max(1, math.floor(wordLength * framesPerChar))
								local endFrame = startFrame + duration

								table.insert(wordTiming, {
									["startIndex"] = charIndex,
									["endIndex"] = charIndex + length - 1,
									["startFrame"] = startFrame,
									["endFrame"] = endFrame,
								})
								
								currentFrame = endFrame
							else
								-- Update character boundaries for existing words.
								wordTiming[i].startIndex = charIndex
								wordTiming[i].endIndex = charIndex + length - 1
							end

							-- Advance the character playhead for the next word.
							charIndex = charIndex + length
						end

						-- Scale the rebuilt timing to fit the composition end without collapsing
						-- word boundaries, so markers and highlights remain in sync.
						wordTiming = loadstring(tool:GetData("ScaleWordTiming"))()(comp, wordTiming)

						loadstring(tool:GetData("ApplyWordTiming"))()(comp, tool, wordTiming)
					end
				
]==]
SCALE_WORD_TIMING = [==[
					return function(comp, wordTiming, force)
						if not wordTiming or #wordTiming == 0 then
							return wordTiming
						end

						local compositionEndFrame = comp:GetAttrs("COMPN_RenderEnd") or 1000
						local maxEndFrame = math.max(1, compositionEndFrame - 1)

						local n = #wordTiming
						local lastEndFrame = wordTiming[n].endFrame
						if lastEndFrame <= maxEndFrame and not force then
							return wordTiming
						end

						-- More words than available frames: merge trailing words into the last
						-- keepable interval so every remaining marker/highlight is on a
						-- distinct, in-range frame.
						if n > maxEndFrame then
							local keep = math.max(1, maxEndFrame)
							wordTiming[keep].endIndex = wordTiming[n].endIndex
							for i = n, keep + 1, -1 do
								table.remove(wordTiming, i)
							end
							n = keep
						end

						-- Scale the remaining timing to fit the composition. Each word gets a
						-- distinct, non-overlapping integer interval.
						lastEndFrame = wordTiming[n].endFrame
						local scale = math.min(maxEndFrame / lastEndFrame, 1)
						local prevEnd = 0
						for i, word in ipairs(wordTiming) do
							local targetStart = math.floor(word.startFrame * scale)
							local targetEnd = math.floor(word.endFrame * scale)
							local startFrame = math.max(0, math.max(targetStart, prevEnd))
							local endFrame = math.max(startFrame + 1, targetEnd)

							if endFrame > maxEndFrame then
								endFrame = maxEndFrame
								if startFrame >= endFrame then
									startFrame = math.max(0, endFrame - 1)
								end
							end

							word.startFrame = startFrame
							word.endFrame = endFrame
							prevEnd = endFrame
						end

						-- Tighten the sequence: each word must start after the previous one,
						-- end before the composition end, and stay in a valid interval.
						for i = 2, n do
							if wordTiming[i].startFrame <= wordTiming[i - 1].startFrame then
								wordTiming[i].startFrame = wordTiming[i - 1].startFrame + 1
							end
							wordTiming[i].endFrame = math.max(wordTiming[i].endFrame, wordTiming[i].startFrame + 1)
							if wordTiming[i].endFrame > maxEndFrame then
								wordTiming[i].endFrame = maxEndFrame
								if wordTiming[i].startFrame >= wordTiming[i].endFrame then
									wordTiming[i].startFrame = math.max(0, wordTiming[i].endFrame - 1)
								end
							end
							if wordTiming[i].endFrame < wordTiming[i - 1].endFrame then
								wordTiming[i].endFrame = wordTiming[i - 1].endFrame
							end
						end

						wordTiming[n].endFrame = maxEndFrame

						return wordTiming
					end
				
]==]

local function world(opts)
    opts = opts or {}
    local w = { prefs = {}, tracks = { { name = "Video 1", items = {} } }, imports = 0 }

    local fusion = {}
    function fusion:SetPrefs(k, v) w.prefs[k] = v end
    function fusion:SavePrefs() end
    function fusion:GetPrefs(k) return w.prefs[k] end

    -- A Text+ tool; inputs in `lacks` are not there (set does nothing, get gives nil), and with
    -- `known` only those inputs are there.
    local function Tool(lacks, known)
        local t = { inputs = {}, data = {} }
        function t:SetInput(k, v)
            if not (lacks and lacks[k]) and not (known and not known[k]) then self.inputs[k] = v end
        end
        function t:GetInput(k) return self.inputs[k] end
        function t:GetData(k) return self.data[k] end
        function t:SetData(k, v) self.data[k] = v end
        return t
    end
    -- The animated caption's macro (5l): its published inputs only (the bundled macro's
    -- InputKeys), its SetInputValues helper recording the settings it is handed (and counting how
    -- often it is compiled), and, as the bundled macro's own `Text` control does when the text
    -- changes, its real `UpdateTextContent` and `ScaleWordTiming` (below) re-counting the word
    -- timing from the text. Its Text+ has only the inputs Junctur may set on it.
    local HELPER = [[
        _G.COMPILES = (_G.COMPILES or 0) + 1
        return function(comp, tool, settings)
            for k, v in pairs(settings) do tool:SetInput(k, v) end
            tool.applied = (tool.applied or 0) + 1
        end
    ]]
    local APPLY = [[
        return function(comp, tool, wordTiming)
            tool:SetData("WordTiming", wordTiming)
            tool.recounted = wordTiming
        end
    ]]
    local function set_of(keys)
        local out = {}
        for _, k in ipairs(keys) do out[k] = true end
        return out
    end
    local MACRO_INPUTS = set_of({ "Font", "Style", "TextSize", "TextPosition", "FadeEnabled",
        "PopInEnabled", "SlideUpEnabled", "AnimationLength", "AnimationLevel", "AnimationMode",
        "HighlightExtendHorizontal", "HighlightExtendVertical", "HighlightRound",
        "HighlightColorRed", "HighlightColorGreen", "HighlightColorBlue", "FillEnabled",
        "FillColorRed", "FillColorGreen", "FillColorBlue", "OutlineEnabled", "OutlineThickness",
        "OutlineColorRed", "OutlineColorGreen", "OutlineColorBlue", "ShadowEnabled",
        "ShadowColorRed", "ShadowColorGreen", "ShadowColorBlue", "HighlightStyle",
        "HighlightEnabled", "WrapEnabled", "WrapBoxWidth" })
    local function Item(info)
        local comp = {}
        local tool, macro, cls
        if info.mediaPoolItem.macro then
            tool = Tool(nil, set_of(opts.no_text_control and { "StyledText", "Underline" }
                or { "Text", "StyledText", "Underline" }))
            macro = Tool(nil, MACRO_INPUTS)
            cls = Tool()
            if not opts.no_helper then macro.data.SetInputValues = HELPER end
            macro.data.UpdateTextContent = UPDATE_TEXT_CONTENT
            macro.data.ScaleWordTiming = SCALE_WORD_TIMING
            macro.data.ApplyWordTiming = APPLY
            if opts.no_data then
                function macro:SetData() error("no data here") end
            end
            local set = tool.SetInput
            function tool:SetInput(k, v)
                local before = self.inputs[k]
                set(self, k, v)
                if k == "Text" and self.inputs.Text == v then
                    self.timing = macro.data.WordTiming
                    if before ~= v then
                        assert(loadstring(macro.data.UpdateTextContent))()(comp, macro, v)
                    end
                end
            end
        else
            tool = Tool(info.mediaPoolItem.lacks)
        end
        function comp:GetAttrs(k) if k == "COMPN_RenderEnd" then return info.endFrame end end
        function comp:GetPrefs(k) if k == "Comp.FrameFormat.Rate" then return 24 end end
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

    -- A media pool clip: a Fusion Title unless `kind` says otherwise; the third argument is the
    -- animated caption's macro (true, 5l) or the file the clip is of (a path, 5z).
    local function Clip(name, lacks, x)
        local c = { name = name, lacks = lacks, macro = x == true,
            path = type(x) == "string" and x or nil }
        -- `props` overrides a property (Type, In, Out); `marks` is what GetMarkInOut gives.
        function c:GetClipProperty(k)
            if self.props and self.props[k] ~= nil then return self.props[k] end
            if k == "Clip Name" then return self.name elseif k == "FPS" then return "24"
            elseif k == "File Path" then return self.path or ""
            elseif k == "Type" then
                return self.kind or (self.path and "Video + Audio" or "Fusion Title")
            elseif k == "In" or k == "Out" then return "" end
        end
        function c:GetMarkInOut() return self.marks or {} end
        function c:SetClipProperty(_, v)
            if opts.no_rename then return false end
            self.name = v
            return true
        end
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
        return { GetCurrentProject = function()
            if opts.no_project or w.no_project then return nil end
            return project
        end }
    end

    local chunk = assert(loadfile(SCRIPT))
    -- `no_compile`: a script runner without loadstring or load.
    setfenv(chunk, setmetatable({ JUNCTUR_TEST = true, fusion = fusion, resolve = resolve },
        { __index = function(_, k)
            if opts.no_compile and (k == "loadstring" or k == "load") then return nil end
            return _G[k]
        end }))
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
-- A clip Junctur did not place (5n): a video clip or a still, with no Fusion comp.
local function stub(start, length)
    return { start = start, length = length,
        GetStart = function(self) return self.start end,
        GetEnd = function(self) return self.start + self.length end,
        GetFusionCompCount = function() return 0 end }
end
-- A Text+ title on the timeline: the user's own (no tag), or with `recording`, another
-- recording's caption.
local function title_item(start, length, recording)
    local tool = { data = { JuncturRecording = recording }, inputs = {} }
    function tool:GetData(k) return self.data[k] end
    function tool:SetData(k, v) self.data[k] = v end
    function tool:SetInput(k, v) self.inputs[k] = v end
    function tool:GetInput(k) return self.inputs[k] end
    local comp = {}
    function comp:FindTool(n) if n == "Template" then return tool end end
    function comp:FindToolByID() return tool end
    local it = stub(start, length)
    it.tool = tool
    it.GetFusionCompCount = function() return 1 end
    it.GetFusionCompByIndex = function() return comp end
    return it
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

-- What 5l's and 5m's cases asked of `info` (gone in 5z): the template a send finds, through a
-- `build` under a name of its own, the open timeline left open after it so `place` goes on there.
local function info_of(w, req)
    local was = w.current
    local r = w.m.handlers.build(build({ name = "probe " .. (#w.made + 1),
        template = req.template, animated = req.animated }))
    w.current = was
    return r
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
check(w.root.subs[2].name == "Junctur" and #w.root.subs[2].clips == 2, "Junctur bin")
check(w.root.subs[2].clips[1].name == "Junctur Caption", "plain template renamed")
check(w.root.subs[2].clips[2].name == "Junctur Animated Caption", "animated template kept (5l)")
c = w.m.handlers.check({})
check(c.timelines == 2 and c.timeline2 == "rec (Junctur)", "check sees it")

-- 5l and 5m: the template a build finds. The bundled bin imported once; the animated caption (the
-- highlight or an animation on) from the Junctur bin without importing again; a named Fusion
-- Title of the user's, or an error when there is none.
do
    local wt = world()
    info_of(wt, { template = "" })
    info_of(wt, { template = "" })
    check(wt.imports == 1, "imported once")
    local found = info_of(wt, { template = "", animated = true })
    check(found.template == "Junctur Animated Caption" and wt.imports == 1, "animated template, no import")
    check(info_of(wt, { template = "Nope" }).error ~= nil, "a missing template is an error")
    check(info_of(wt, { template = "My Title" }).template == "My Title", "the user's own template")
end

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

-- 5n on 5ab's update: the timeline there holds a clip of the user's on Junctur's track where a new
-- caption goes. Asked: nothing changed; then skip (the answer, sent again as an update) leaves that
-- caption out and the clip where it is.
local mine = stub(100, 30)
table.insert(there.tracks[2].items, mine)
local again = { clip(1, 0, "hola otra vez"), clip(1, 48, "nuevo"), clip(1, 110, "choca") }
r = w.m.handlers.open({ name = "rec (Junctur)", template = "" })
r = w.m.handlers.place(request(again, { timeline = r.id }))
check(r.inTheWay == 1 and r.blocked == 1 and r.placed == nil and #there.tracks[2].items == 3,
    "update asks: " .. tostring(r.inTheWay))
r = w.m.handlers.open({ name = "rec (Junctur)", template = "" })
r = w.m.handlers.place(request(again, { timeline = r.id, existing = "skip" }))
check(r.skipped == 1 and r.restyled == 2 and r.placed == 0 and #there.tracks[2].items == 3,
    "update skips: " .. tostring(r.skipped) .. " " .. tostring(r.firstError))
local kept_mine = false
for _, it in ipairs(there.tracks[2].items) do if it == mine then kept_mine = true end end
check(kept_mine and #w.made == made_before, "the user's clip kept, nothing made")
r = w.m.handlers.open({ name = "rec (Junctur)", template = "", animated = true })
check(r.error == nil and r.template == "Junctur Animated Caption", "open finds the animated caption (5l)")

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
check(r.error ~= nil and r.error:find("did not switch") and r.error:find("now has \"Main\" open")
    and w.current == w.main, "not opened: " .. tostring(r.error))
-- Refused before the template is looked up, so the bundled bin is not imported either.
check(w.imports == 0 and #w.root.subs == 1, "not opened, nothing imported: " .. w.imports)

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

-- Another recording's caption in the way: that one fails, the rest are placed. (A clip Junctur
-- did not place is asked about instead, 5n, below.)
w = world()
local blocker = title_item(86430, 10, "zzz")
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
        { startIndex = 4, endIndex = 9, startFrame = 12, endFrame = 23 } }
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
info = info_of(w, { template = "", animated = true })
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

-- The macro re-counted the word timing from the text when it was set, and it is unchanged:
-- Junctur counts as it does.
local function same(a, b)
    if #a ~= #b then return false end
    for k = 1, #a do
        for _, f in ipairs({ "startIndex", "endIndex", "startFrame", "endFrame" }) do
            if a[k][f] ~= b[k][f] then return false end
        end
    end
    return true
end
check(it.macro.recounted ~= nil and same(it.macro.recounted, animated(1, 0, "").words),
    "the macro's re-count kept the word timing")
check(_G.COMPILES == 1, "the helper compiled once")

-- Characters beyond ASCII and a line break: the macro's utf8len counts "¿" and "é" as one each,
-- as Junctur does, so the ranges survive its re-count.
local accented = animated(2, 86400, "¿qué tal\ntodos", { key = "Junctur 2 Bo|86400|86424|highlight",
    words = { { startIndex = 0, endIndex = 3, startFrame = 0, endFrame = 8 },
        { startIndex = 4, endIndex = 7, startFrame = 8, endFrame = 16 },
        { startIndex = 8, endIndex = 13, startFrame = 16, endFrame = 23 } } })
local two = animated(1, 86450, "hola todos", { key = "Junctur 1 Ana|86450|86474|highlight" })
_G.COMPILES = 0
local w2 = world()
r = w2.m.handlers.place(request({ animated(1, 86400, "hola todos"), accented, two },
    { look = moving, animated = true }))
check(r.placed == 3 and r.failed == 0, "accented: " .. tostring(r.firstError))
local bo = w2.tracks[3].items[1]
check(same(bo.macro.recounted, accented.words), "accented ranges kept by the macro's re-count")
check(_G.COMPILES == 1, "one compile a send, not one a caption: " .. tostring(_G.COMPILES))

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

-- A macro whose Text+ has no Text control: the caption still shows (StyledText), and the reply
-- says it is neither lit nor animated.
w = world({ no_text_control = true })
r = w.m.handlers.place(request({ animated(1, 86400, "a") }, { look = moving, animated = true }))
check(r.placed == 1 and r.unstyled == "animation, word highlight", "no Text: " .. tostring(r.unstyled))
check(w.tracks[2].items[1].tool.inputs.StyledText == "a", "shown through StyledText")

-- A macro that does not keep the word timing: placed, the highlight said not to apply.
w = world({ no_data = true })
r = w.m.handlers.place(request({ animated(1, 86400, "a") }, { look = moving, animated = true }))
check(r.placed == 1 and r.unstyled == "word highlight", "no data: " .. tostring(r.unstyled))

-- No loadstring or load: the caption fails, and the reason says what to do.
w = world({ no_compile = true })
r = w.m.handlers.place(request({ animated(1, 86400, "a") }, { look = moving, animated = true }))
check(r.failed == 1 and r.firstError:find("cannot compile") ~= nil and r.firstError:find("turn") ~= nil,
    "no compile: " .. tostring(r.firstError))

-- The import's renames did not take: the animated caption is still never the plain template.
w = world({ no_rename = true })
info = info_of(w, { template = "" })
check(info.template == "Basic Template", "plain, unrenamed: " .. tostring(info.template))
info = info_of(w, { template = "", animated = true })
check(info.template == "AutoSubs Caption 2026-09-16" and w.imports == 1,
    "animated, unrenamed: " .. tostring(info.template))

-- A macro that lost its helper: that caption fails, said so.
w = world({ no_helper = true })
r = w.m.handlers.place(request({ animated(1, 86400, "a") }, { look = moving, animated = true }))
check(r.failed == 1 and r.firstError:find("SetInputValues") ~= nil, "no helper: " .. tostring(r.firstError))

-- Folders without ids: the import is not trusted, nothing moved or deleted.
w = world({ no_ids = true })
info = w.m.handlers.build(build())
check(info.error ~= nil and w.imports == 0 and #w.made == 0, "no ids, no import, no timeline")
check(#w.root.subs == 1 and #w.theirs.clips == 2, "their bin untouched without ids")

-- 5m: the template picker's list. The user's titles each once (the first by the lookup's walk:
-- sub-bins first), Junctur's own two and clips that are no title left out.
w = world()
info_of(w, { template = "" })
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
check(w.m.handlers.check({}).script == "5ab", "the script says which it is, in check (5z on)")
-- No project open: an error, not an empty list.
check(world({ no_project = true }).m.handlers.templates().error ~= nil, "no project")

-- 5m repair: one name rule on both sides. A title named with spaces round it is listed trimmed,
-- and a send naming it trimmed (as Junctur sends it) finds it; "A" and "A " are listed once.
w = world()
local spaced = w.Clip(" Spaced title  ")
w.theirs.clips = { spaced, w.Clip("Spaced title") }
list = w.m.handlers.templates()
names = {}
for k = 1, list.count do names[k] = list["t" .. k] end
check(table.concat(names, ",") == "Spaced title", "trimmed once: " .. table.concat(names, ","))
info = info_of(w, { template = "Spaced title" })
check(info.error == nil and info.template == " Spaced title  ", "found by its trimmed name: "
    .. tostring(info.error))

-- 5m repair: a video clip or a still of the template's name, earlier in the lookup's walk, is not
-- taken for it: the Fusion Title of that name is.
w = world()
local clash = w.Folder("Footage", "8")
local footage = w.Clip("Lower third")
footage.kind = "Video"
clash.clips = { footage }
table.insert(w.root.subs, 1, clash)
local title = w.Clip("Lower third")
table.insert(w.theirs.clips, title)
r = w.m.handlers.place(request({ clip(1, 86400, "hola") }, { template = "Lower third" }))
check(r.placed == 1 and r.failed == 0, "shadowed title: " .. tostring(r.firstError or r.error))
check(w.tracks[2].items[1].template == title, "the title, not the video clip of its name")
-- Only a clip that is no title by that name: not found, said so.
w = world()
footage = w.Clip("Only footage")
footage.kind = "Still"
table.insert(w.theirs.clips, footage)
check(info_of(w, { template = "Only footage" }).error ~= nil, "a still is no template")

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

-- 5n: clips Junctur did not place where new captions go on its tracks. Each world: a first send
-- placed Ana's caption at 86400 and Bo's at 86500; then the user put on Ana's track a title of
-- their own starting on the very frame the next caption does, a video clip over two more
-- captions, and a clip clear of every caption. The next send keeps Ana's, drops Bo's, and adds
-- three captions for Ana (two of them meeting the video clip) and one for Bo.
local function crowded(opts)
    local w = world(opts)
    w.m.handlers.place(request({ clip(1, 86400, "a"), clip(2, 86500, "old") }))
    w.title, w.video, w.clear = title_item(86430, 10), stub(86470, 30), stub(87000, 10)
    for _, it in ipairs({ w.title, w.video, w.clear }) do table.insert(w.tracks[2].items, it) end
    w.ana = w.tracks[2].items[1]
    return w
end
local next_send = { clip(1, 86400, "a2"), clip(1, 86430, "b"), clip(1, 86460, "c"),
    clip(1, 86490, "d"), clip(2, 86400, "e") }
local function has(items, x)
    for _, it in ipairs(items) do if it == x then return true end end
    return false
end
local function untouched(w, what)
    check(#w.tracks == 3 and #w.tracks[2].items == 4 and #w.tracks[3].items == 1, what .. ": tracks")
    check(w.ana.tool.inputs.StyledText == "a", what .. ": Ana's caption restyled")
    check(w.tracks[3].items[1].tool.inputs.StyledText == "old", what .. ": Bo's old caption removed")
    check(w.title.tool.data.JuncturKey == nil and next(w.title.tool.inputs) == nil,
        what .. ": the user's title touched")
end

-- Asked (no choice sent, or one not known): the clips and the captions they are in the way of
-- counted, and nothing changed.
w = crowded()
r = w.m.handlers.place(request(next_send))
check(r.inTheWay == 2 and r.blocked == 3 and r.placed == nil and r.error == nil,
    "asked: " .. tostring(r.inTheWay) .. " " .. tostring(r.blocked))
untouched(w, "asked")
r = w.m.handlers.place(request(next_send, { existing = "merge" }))
check(r.inTheWay == 2, "an unknown choice asks")
untouched(w, "unknown choice")

-- A clip of the user's clear of every caption is no reason to ask.
w = crowded()
r = w.m.handlers.place(request({ clip(1, 86400, "a"), clip(1, 86600, "far") }))
check(r.inTheWay == nil and r.placed == 1 and r.removed == 1, "clear clip: " .. tostring(r.inTheWay))
check(has(w.tracks[2].items, w.clear), "the clear clip kept")

-- Skip: the three captions meeting the user's clips are left out, the rest done as 5g.
w = crowded()
r = w.m.handlers.place(request(next_send, { existing = "skip" }))
check(r.skipped == 3 and r.placed == 1 and r.restyled == 1 and r.removed == 1 and r.failed == 0,
    "skip: " .. tostring(r.firstError))
check(r.replaced == 0 and r.newTracks == 0, "skip did nothing else")
check(#w.tracks[2].items == 4 and has(w.tracks[2].items, w.video), "the user's clips stay")
check(w.title.tool.data.JuncturKey == nil and next(w.title.tool.inputs) == nil,
    "skip: the title is not taken for the caption starting on its frame")
check(w.tracks[3].items[1].tool.inputs.StyledText == "e", "Bo's placed")

-- Replace: only the user's clips a caption meets are deleted, then every caption placed.
w = crowded()
r = w.m.handlers.place(request(next_send, { existing = "replace" }))
check(r.replaced == 2 and r.placed == 4 and r.restyled == 1 and r.failed == 0,
    "replace: " .. tostring(r.firstError))
check(not has(w.tracks[2].items, w.title) and not has(w.tracks[2].items, w.video), "deleted")
check(has(w.tracks[2].items, w.clear) and #w.tracks[2].items == 5, "the clear clip kept")
for _, it in ipairs(w.tracks[2].items) do
    if it ~= w.clear then check(it.tool.data.JuncturRecording == "abc", "replace: placed and tagged") end
end
-- Resolve refusing the delete: an error, and nothing changed.
w = crowded({ no_delete = true })
r = w.m.handlers.place(request(next_send, { existing = "replace" }))
check(r.error ~= nil and r.error:find("no caption was placed") ~= nil, "refused delete: " .. tostring(r.error))
untouched(w, "refused delete")
-- 5n repair: Resolve refusing a new speaker's track after Replace was chosen: an error, and the
-- user's clips are still there (they are deleted only once every track is in place).
local opts = {}
w = crowded(opts)
opts.no_tracks = true
local with_cy = { { track = 3, key = "Junctur 3 Cy|86400|86424", start = 86400, ["end"] = 86424,
    length = 24, text = "cy", fill = { 1, 0, 0 }, x = 0.5, y = 0.3 } }
for _, c in ipairs(next_send) do with_cy[#with_cy + 1] = c end
r = w.m.handlers.place(request(with_cy, { existing = "replace",
    tracks = { "Junctur 1 Ana", "Junctur 2 Bo", "Junctur 3 Cy" } }))
check(r.error ~= nil and r.error:find("track") ~= nil, "track refused: " .. tostring(r.error))
check(has(w.tracks[2].items, w.title) and has(w.tracks[2].items, w.video), "the user's clips kept")
untouched(w, "track refused after replace")

-- 5n repair: a clip whose end or start cannot be read is counted in the way of every caption it
-- might meet, and Replace does not delete it: the captions it might meet are left out instead.
local function unmeasured(start, stop)
    local it = stub(start or 0, 0)
    if not start then it.GetStart = function() error("no start") end end
    if stop then
        it.GetEnd = function() return stop end
    else
        it.GetEnd = function() error("no end") end
    end
    return it
end
for _, case in ipairs({ { "no end", unmeasured(86410, nil) }, { "no start", unmeasured(nil, 86440) } }) do
    local name, odd = case[1], case[2]
    w = world()
    w.m.handlers.place(request({ clip(1, 86400, "a") }))
    table.insert(w.tracks[2].items, odd)
    r = w.m.handlers.place(request({ clip(1, 86400, "a"), clip(1, 86430, "b") }))
    check(r.inTheWay == 1 and r.blocked == 1, name .. ": asked " .. tostring(r.inTheWay))
    r = w.m.handlers.place(request({ clip(1, 86400, "a"), clip(1, 86430, "b") }, { existing = "replace" }))
    check(r.replaced == 0 and r.skipped == 1 and r.placed == 0 and r.restyled == 1,
        name .. ": replace " .. tostring(r.replaced) .. " " .. tostring(r.skipped))
    check(has(w.tracks[2].items, odd), name .. ": not deleted")
end

-- A new track: Ana's new captions on "Junctur 1 Ana (2)", her caption already placed restyled
-- where it is, Bo's on his own; the user's clips untouched.
w = crowded()
r = w.m.handlers.place(request(next_send, { existing = "newTrack" }))
check(r.newTracks == 1 and r.placed == 4 and r.restyled == 1 and r.removed == 1 and r.failed == 0,
    "new track: " .. tostring(r.firstError))
check(#w.tracks == 4 and w.tracks[4].name == "Junctur 1 Ana (2)" and #w.tracks[4].items == 3,
    "the new track")
check(#w.tracks[2].items == 4 and w.ana.tool.inputs.StyledText == "a2", "Ana's own track")
check(w.title.tool.data.JuncturKey == nil and next(w.title.tool.inputs) == nil, "title untouched")
-- Sent again, remembered: everything restyled in place, no further track.
r = w.m.handlers.place(request(next_send, { existing = "newTrack" }))
check(r.restyled == 5 and r.placed == 0 and r.newTracks == 0 and #w.tracks == 4, "resent")
-- A new caption meeting a clip of the user's again goes on the same track beside.
local more = { clip(1, 86995, "f") }
for _, c in ipairs(next_send) do more[#more + 1] = c end
r = w.m.handlers.place(request(more, { existing = "newTrack" }))
check(r.placed == 1 and r.newTracks == 1 and #w.tracks == 4 and #w.tracks[4].items == 4,
    "the same track beside: " .. tostring(r.firstError))
-- 5n repair: the user's clips gone from Ana's track, her new captions still go on the track
-- beside, which holds hers, rather than splitting back onto her own.
for _, it in ipairs({ w.title, w.video, w.clear }) do drop(w.tracks[2].items, it) end
more[#more + 1] = clip(1, 86600, "g")
r = w.m.handlers.place(request(more, { existing = "newTrack" }))
check(r.placed == 1 and r.newTracks == 1 and #w.tracks[4].items == 5 and #w.tracks[2].items == 1,
    "kept beside: " .. tostring(r.firstError))

-- A track "(2)" holding a clip of the user's where Ana's captions go: "(3)" is used.
w = crowded()
w.tracks[4] = { name = "Junctur 1 Ana (2)", items = { stub(86440, 60) } }
r = w.m.handlers.place(request(next_send, { existing = "newTrack" }))
check(r.placed == 4 and #w.tracks == 5 and w.tracks[5].name == "Junctur 1 Ana (3)", "(3)")
check(#w.tracks[4].items == 1, "(2) left alone")

print("ALL OK")
