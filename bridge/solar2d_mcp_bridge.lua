-- solar2d_mcp_bridge.lua
-- Written by solar2d-mcp each time it launches this project. Safe to delete
-- (the "solar2d-mcp" line at the top of main.lua is what loads it).
-- Only does anything inside the Solar2D Simulator: on a device it returns at once.
--
-- It connects to the solar2d-mcp server on 127.0.0.1 and answers its commands
-- (screenshot, tap, drag, key, tree, eval, quit) as newline-delimited JSON.

local M = {}
if system.getInfo("environment") ~= "simulator" then return M end
if _G.__solar2d_mcp_bridge then return _G.__solar2d_mcp_bridge end
_G.__solar2d_mcp_bridge = M

local PORT = __PORT__
local SUPPRESS_ERROR_DIALOGS = __SUPPRESS_ERRORS__
local NONCE = "__NONCE__"

local socket = require("socket")
local json = require("json")

local client, inbuf = nil, ""
local forwardPrint = false
local rawPrint = print
local ASYNC = {}

----------------------------------------------------------------------------
-- transport
----------------------------------------------------------------------------
local function closeClient()
  if client then pcall(client.close, client) end
  client, inbuf = nil, ""
end

local function send(t)
  if not client then return end
  local ok, s = pcall(json.encode, t)
  if not ok or type(s) ~= "string" then
    s = json.encode({ id = t.id, ok = false, error = "bridge could not encode reply: " .. tostring(s) })
  end
  client:settimeout(2)
  local sent, err = client:send(s .. "\n")
  client:settimeout(0)
  if not sent then rawPrint("[solar2d-mcp] send failed: " .. tostring(err)); closeClient() end
end

print = function(...)
  rawPrint(...)
  if forwardPrint and client then
    local n, parts = select("#", ...), {}
    for i = 1, n do parts[i] = tostring((select(i, ...))) end
    send({ type = "log", line = table.concat(parts, "\t") })
  end
end

----------------------------------------------------------------------------
-- helpers
----------------------------------------------------------------------------
local function r1(v) return v and math.floor(v * 10 + 0.5) / 10 end

local function screenInfo()
  return {
    contentWidth = display.contentWidth, contentHeight = display.contentHeight,
    actualContentWidth = display.actualContentWidth, actualContentHeight = display.actualContentHeight,
    screenOriginX = display.screenOriginX, screenOriginY = display.screenOriginY,
    pixelWidth = display.pixelWidth, pixelHeight = display.pixelHeight,
    orientation = system.orientation,
  }
end

-- unit "content": Solar2D content coordinates; "fraction": 0..1 across the whole screen
local function toContent(x, y, unit)
  if unit == "fraction" then
    return display.screenOriginX + x * display.actualContentWidth,
           display.screenOriginY + y * display.actualContentHeight
  end
  return x, y
end

local function listenerNames(o)
  local names, seen = {}, {}
  for _, key in ipairs({ "_functionListeners", "_tableListeners" }) do
    local t = rawget(o, key)
    if type(t) == "table" then
      for name, l in pairs(t) do
        if type(l) == "table" and #l > 0 and not seen[name] then seen[name] = true; names[#names + 1] = name end
      end
    end
  end
  table.sort(names)
  return names
end

local function labelOf(o)
  for _, k in ipairs({ "name", "id", "myName" }) do
    local v = o[k]
    if type(v) == "string" or type(v) == "number" then return tostring(v) end
  end
end

local function kindOf(o)
  if o.numChildren ~= nil then return "group" end
  if o.text ~= nil then return "text" end
  return "object"
end

local function shown(o)
  if o.isHitTestable then return true end
  return o.isVisible ~= false and (o.alpha or 1) > 0
end

local function brief(o)
  if o == Runtime then return { kind = "Runtime" } end
  local b = o.contentBounds
  return {
    name = labelOf(o), kind = kindOf(o), text = o.text,
    bounds = b and { r1(b.xMin), r1(b.yMin), r1(b.xMax), r1(b.yMax) } or nil,
  }
end

----------------------------------------------------------------------------
-- hit testing + synthetic input
----------------------------------------------------------------------------
-- Topmost-first list of objects under the point, groups after the children that were hit
-- (the order Solar2D propagates touch/tap events in).
local function hitList(group, x, y, out)
  for i = group.numChildren, 1, -1 do
    local o = group[i]
    if o and shown(o) then
      if o.numChildren ~= nil then
        local before = #out
        hitList(o, x, y, out)
        if #out > before then out[#out + 1] = o end
      else
        local b = o.contentBounds
        if b and x >= b.xMin and x <= b.xMax and y >= b.yMin and y <= b.yMax then out[#out + 1] = o end
      end
    end
  end
  return out
end

local stage = display.getCurrentStage()
local focus = {}
do
  local origSetFocus = stage.setFocus
  stage.setFocus = function(self, obj, id)
    focus[id or "all"] = obj
    if obj == nil then focus = {} end
    return origSetFocus(self, obj, id)
  end
end

-- Dispatch like the real input system would. A listener that throws is logged the way
-- the simulator logs runtime errors, and the error is passed on to the tool call.
local function dispatch(o, ev)
  local ok, res = xpcall(function() return o:dispatchEvent(ev) end, debug.traceback)
  if ok then return res end
  local what = (labelOf(o) or (o == Runtime and "Runtime") or kindOf(o)) .. " " .. ev.name .. " listener"
  local msg = tostring(res)
  rawPrint("[solar2d-mcp] Runtime error in " .. what .. ": " .. msg)
  if client and forwardPrint then send({ type = "error", message = what .. ": " .. msg, stack = "" }) end
  error("the app's " .. what .. " threw: " .. msg:match("^[^\n]*"), 0)
end

local function deliver(ev, x, y)
  local fo = focus[ev.id] or focus.all
  if fo and fo.parent then
    ev.target = fo
    dispatch(fo, ev)
    return { brief(fo) }
  end
  local handledBy = nil
  for _, o in ipairs(hitList(stage, x, y, {})) do
    ev.target = o
    if dispatch(o, ev) then handledBy = o; break end
  end
  if not handledBy then
    ev.target = Runtime
    dispatch(Runtime, ev)
    return nil
  end
  return { brief(handledBy) }
end

local touchSeq = 0
local function newTouchId() touchSeq = touchSeq + 1; return "mcp-touch-" .. touchSeq end

local function touchEvent(phase, id, x, y, x0, y0)
  return { name = "touch", phase = phase, id = id, x = x, y = y, xStart = x0, yStart = y0,
           time = system.getTimer(), pressure = 1 }
end

----------------------------------------------------------------------------
-- eval serialisation
----------------------------------------------------------------------------
local function dump(v, depth, seen)
  local t = type(v)
  if t == "string" then return string.format("%q", v) end
  if t ~= "table" then return tostring(v) end
  if seen[v] then return "<cycle>" end
  if v == Runtime then return "<Runtime>" end
  if v._proxy ~= nil or v.contentBounds ~= nil then
    local b = brief(v)
    return string.format("<%s%s>", b.kind, b.name and (" " .. b.name) or "")
  end
  if depth <= 0 then return "{...}" end
  seen[v] = true
  local parts, n = {}, 0
  for k, val in pairs(v) do
    n = n + 1
    if n > 50 then parts[#parts + 1] = "..."; break end
    local key = type(k) == "string" and k:match("^[%a_][%w_]*$") and k or ("[" .. dump(k, 0, seen) .. "]")
    parts[#parts + 1] = key .. " = " .. dump(val, depth - 1, seen)
  end
  seen[v] = nil
  return "{ " .. table.concat(parts, ", ") .. " }"
end

----------------------------------------------------------------------------
-- commands
----------------------------------------------------------------------------
local H = {}

H.ping = function() return { info = screenInfo() } end

H.config = function(msg)
  if msg.forward_print ~= nil then forwardPrint = msg.forward_print and true or false end
  return { forward_print = forwardPrint }
end

H.screenshot = function(msg, reply)
  local name = "__solar2d_mcp_shot.png"
  local cap = display.captureScreen(false)
  if not cap then error("display.captureScreen returned nothing", 0) end
  display.save(cap, { filename = name, baseDir = system.TemporaryDirectory, captureOffscreenArea = true })
  cap:removeSelf()
  return { path = system.pathForFile(name, system.TemporaryDirectory), info = screenInfo() }
end

H.tap = function(msg, reply)
  local x, y = toContent(msg.x, msg.y, msg.unit)
  local id = newTouchId()
  local r = { x = r1(x), y = r1(y) }
  r.began = deliver(touchEvent("began", id, x, y, x, y), x, y)
  -- end the touch on the next frame, like a real quick tap
  timer.performWithDelay(1, function()
    local ok, err = pcall(function()
      r.ended = deliver(touchEvent("ended", id, x, y, x, y), x, y)
      focus[id] = nil
      r.tap = deliver({ name = "tap", numTaps = 1, x = x, y = y, time = system.getTimer() }, x, y)
    end)
    if ok then reply(true, r) else reply(false, err) end
  end)
  return ASYNC
end

H.drag = function(msg, reply)
  local x0, y0 = toContent(msg.x1, msg.y1, msg.unit)
  local x1, y1 = toContent(msg.x2, msg.y2, msg.unit)
  local dur = math.max(tonumber(msg.duration_ms) or 300, 1)
  local id = newTouchId()
  local r = { from = { r1(x0), r1(y0) }, to = { r1(x1), r1(y1) } }
  r.began = deliver(touchEvent("began", id, x0, y0, x0, y0), x0, y0)
  local t0 = system.getTimer()
  local function step()
    local ok, err = pcall(function()
      local f = math.min((system.getTimer() - t0) / dur, 1)
      local x, y = x0 + (x1 - x0) * f, y0 + (y1 - y0) * f
      if f < 1 then
        deliver(touchEvent("moved", id, x, y, x0, y0), x, y)
      else
        Runtime:removeEventListener("enterFrame", step)
        deliver(touchEvent("moved", id, x1, y1, x0, y0), x1, y1)
        r.ended = deliver(touchEvent("ended", id, x1, y1, x0, y0), x1, y1)
        focus[id] = nil
        reply(true, r)
      end
    end)
    if not ok then Runtime:removeEventListener("enterFrame", step); reply(false, err) end
  end
  Runtime:addEventListener("enterFrame", step)
  return ASYNC
end

H.key = function(msg, reply)
  local phases = msg.phase == "down" and { "down" } or msg.phase == "up" and { "up" } or { "down", "up" }
  local function send1(phase)
    dispatch(Runtime, { name = "key", keyName = msg.key, phase = phase, nativeKeyCode = 0,
      isShiftDown = msg.shift or false, isCtrlDown = msg.ctrl or false, isAltDown = msg.alt or false,
      isCommandDown = false, device = nil, descriptor = "Keyboard: " .. tostring(msg.key) })
  end
  send1(phases[1])
  if #phases == 1 then return { sent = phases } end
  timer.performWithDelay(1, function()
    local ok, err = pcall(send1, phases[2])
    if ok then reply(true, { sent = phases }) else reply(false, err) end
  end)
  return ASYNC
end

H.tree = function(msg)
  local maxDepth = tonumber(msg.max_depth) or 8
  local maxNodes = tonumber(msg.max_nodes) or 400
  local hidden = msg.include_hidden and true or false
  local count, truncated = 0, false
  local function node(o, depth)
    count = count + 1
    local b = o.contentBounds
    local d = {
      name = labelOf(o), kind = kindOf(o), text = o.text,
      x = r1(o.x), y = r1(o.y),
      bounds = b and { r1(b.xMin), r1(b.yMin), r1(b.xMax), r1(b.yMax) } or nil,
    }
    if o.isVisible == false then d.visible = false end
    if (o.alpha or 1) < 1 then d.alpha = r1(o.alpha) end
    local l = listenerNames(o)
    if #l > 0 then d.listeners = l end
    if o.numChildren ~= nil and o.numChildren > 0 then
      if depth >= maxDepth then
        d.children_not_shown = o.numChildren
      else
        d.children = {}
        for i = 1, o.numChildren do
          local c = o[i]
          if count >= maxNodes then truncated = true; break end
          if c and (hidden or shown(c)) then d.children[#d.children + 1] = node(c, depth + 1) end
        end
      end
    end
    return d
  end
  local root = node(stage, 0)
  local out = { stage = root, truncated = truncated, nodes = count, info = screenInfo(),
                runtime_listeners = listenerNames(Runtime) }
  local composer = package.loaded["composer"]
  if composer and composer.getSceneName then
    out.composer_scene = composer.getSceneName("current")
    out.composer_overlay = composer.getSceneName("overlay")
  end
  return out
end

H.eval = function(msg)
  local code = tostring(msg.code or "")
  local f, err = loadstring("return " .. code, "=eval")
  if not f then f, err = loadstring(code, "=eval") end
  if not f then error(err, 0) end
  local res = { n = 0 }
  local function pack(...) res = { n = select("#", ...), ... } end
  pack(pcall(f))
  if not res[1] then error(tostring(res[2]), 0) end
  local parts = {}
  for i = 2, res.n do parts[#parts + 1] = dump(res[i], tonumber(msg.depth) or 3, {}) end
  return { values = parts }
end

H.quit = function(msg, reply)
  reply(true, { quitting = true })
  timer.performWithDelay(50, function() native.requestExit() end)
  return ASYNC
end

local function handle(line)
  local ok, msg = pcall(json.decode, line)
  if not ok or type(msg) ~= "table" then return end
  local id = msg.id
  local done = false
  local function reply(success, result)
    if done then return end
    done = true
    if success then send({ id = id, ok = true, result = result })
    else send({ id = id, ok = false, error = tostring(result) }) end
  end
  local h = H[msg.cmd]
  if not h then return reply(false, "unknown command: " .. tostring(msg.cmd)) end
  local okh, res = pcall(h, msg, reply)
  if not okh then reply(false, res)
  elseif res ~= ASYNC then reply(true, res) end
end

----------------------------------------------------------------------------
-- connect / poll
----------------------------------------------------------------------------
local function tryConnect()
  if client then return end
  local c = socket.tcp()
  c:settimeout(0.25)
  local ok = c:connect("127.0.0.1", PORT)
  if not ok then c:close(); return end
  c:settimeout(0)
  pcall(c.setoption, c, "tcp-nodelay", true)
  client, inbuf = c, ""
  send({ type = "hello", nonce = NONCE, info = screenInfo(),
         build = system.getInfo("build"), model = system.getInfo("model"),
         platform = system.getInfo("platform") })
  -- lets the server check whether stdout reaches it live
  rawPrint("[solar2d-mcp] bridge connected " .. NONCE)
end

local function poll()
  if not client then return end
  while client do
    local line, err, partial = client:receive("*l")
    if line then
      local full = inbuf .. line
      inbuf = ""
      handle(full)
    else
      if partial and #partial > 0 then inbuf = inbuf .. partial end
      if err == "closed" then closeClient() end
      return
    end
  end
end

Runtime:addEventListener("enterFrame", poll)
timer.performWithDelay(2000, tryConnect, 0)
tryConnect()

if SUPPRESS_ERROR_DIALOGS then
  Runtime:addEventListener("unhandledError", function(e)
    rawPrint("[solar2d-mcp] Runtime error: " .. tostring(e.errorMessage) .. "\n" .. tostring(e.stackTrace))
    if client then send({ type = "error", message = tostring(e.errorMessage), stack = tostring(e.stackTrace) }) end
    return true -- handled: no blocking error dialog
  end)
end

return M
