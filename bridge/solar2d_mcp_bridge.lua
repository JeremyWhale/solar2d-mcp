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
  local w = rawget(o, "_widgetType")
  if type(w) == "string" then return "widget:" .. w end
  if o.numChildren ~= nil then return "group" end
  if o.isSecure ~= nil then return "textfield" end
  if o.isEditable ~= nil then return "textbox" end
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
local function inBounds(b, x, y)
  return b and x >= b.xMin and x <= b.xMax and y >= b.yMin and y <= b.yMax
end

-- Topmost-first list of objects under the point, groups after the children that were hit
-- (the order Solar2D propagates touch/tap events in). A group's contentBounds is the union
-- of its children, except for containers (scroll views, table views...), where it is the
-- clip rectangle, so requiring the point to be inside each group also respects clipping.
local function hitList(group, x, y, out)
  for i = group.numChildren, 1, -1 do
    local o = group[i]
    if o and shown(o) then
      if o.numChildren ~= nil then
        if inBounds(o.contentBounds, x, y) then
          local before = #out
          hitList(o, x, y, out)
          if #out > before then out[#out + 1] = o end
        end
      elseif inBounds(o.contentBounds, x, y) then
        out[#out + 1] = o
      end
    end
  end
  return out
end

-- Mirror the stage's touch focus so synthetic touches follow it like real ones:
--   setFocus(nil) clears all; setFocus(obj, nil) removes obj's focus;
--   setFocus(obj, id) focuses one touch; setFocus(obj) focuses every touch.
local stage = display.getCurrentStage()
local focus = {}
do
  local origSetFocus = stage.setFocus
  stage.setFocus = function(self, obj, ...)
    local n, id = select("#", ...), ...
    if obj == nil then
      focus = {}
    elseif n >= 1 and id == nil then
      for k, v in pairs(focus) do if v == obj then focus[k] = nil end end
    else
      focus[id or "all"] = obj
    end
    return origSetFocus(self, obj, ...)
  end
end

-- Dispatch like the real input system would. A listener that throws is logged the way
-- the simulator logs runtime errors, and the error is passed on to the tool call.
local function dispatch(o, ev)
  local ok, res = xpcall(function() return o:dispatchEvent(ev) end, debug.traceback)
  if ok then return res end
  local what = (labelOf(o) or (o == Runtime and "Runtime") or kindOf(o)) .. " " .. ev.name .. " listener"
  local msg = tostring(res)
  print("[solar2d-mcp] Runtime error in " .. what .. ": " .. msg)
  error("the app's " .. what .. " threw: " .. msg:match("^[^\n]*"), 0)
end

local function deliver(ev, x, y)
  local fo = ev.name == "touch" and (focus[ev.id] or focus.all)
  if fo and fo.parent then
    -- a focused touch goes to the focus object, then up through its parent groups
    local o = fo
    while o and o ~= stage do
      ev.target = o
      if dispatch(o, ev) then return { brief(o) } end
      o = o.parent
    end
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
  if type(rawget(v, "_proxy")) == "userdata" then
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
  -- hold like a real finger (widgets tell a tap from a press by timing)
  timer.performWithDelay(math.max(tonumber(msg.hold_ms) or 100, 1), function()
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
  local function clipTo(c, b)
    if not b then return c end
    return { xMin = math.max(c.xMin, b.xMin), yMin = math.max(c.yMin, b.yMin),
             xMax = math.min(c.xMax, b.xMax), yMax = math.min(c.yMax, b.yMax) }
  end
  local screen = { xMin = display.screenOriginX, yMin = display.screenOriginY,
                   xMax = display.screenOriginX + display.actualContentWidth,
                   yMax = display.screenOriginY + display.actualContentHeight }
  local function node(o, depth, clip)
    count = count + 1
    local b = o.contentBounds
    local d = {
      name = labelOf(o), kind = kindOf(o), text = o.text,
      x = r1(o.x), y = r1(o.y),
      bounds = b and { r1(b.xMin), r1(b.yMin), r1(b.xMax), r1(b.yMax) } or nil,
    }
    -- not visible: scrolled out of its scroll view/container, or off screen
    if b and (b.xMax <= clip.xMin or b.xMin >= clip.xMax or b.yMax <= clip.yMin or b.yMin >= clip.yMax) then
      d.offscreen = true
    end
    if o.numChildren ~= nil then clip = clipTo(clip, b) end
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
          if c and (hidden or shown(c)) then d.children[#d.children + 1] = node(c, depth + 1, clip) end
        end
      end
    end
    return d
  end
  local root = node(stage, 0, screen)
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

----------------------------------------------------------------------------
-- finding objects
----------------------------------------------------------------------------
local function walk(fn)
  local function rec(g)
    for i = 1, g.numChildren do
      local o = g[i]
      if o then
        if fn(o) then return o end
        if o.numChildren ~= nil then
          local f = rec(o)
          if f then return f end
        end
      end
    end
  end
  return rec(stage)
end

local function isField(o) return o.isSecure ~= nil or o.isEditable ~= nil end

----------------------------------------------------------------------------
-- typing into native text fields
----------------------------------------------------------------------------
local keyboardFocus = nil
do
  local orig = native.setKeyboardFocus
  native.setKeyboardFocus = function(o, ...)
    keyboardFocus = o
    return orig(o, ...)
  end
end

local function keyNameFor(ch)
  if ch == " " then return "space" end
  if ch == "\n" then return "enter" end
  if ch == "\t" then return "tab" end
  return ch:lower()
end

H.type = function(msg)
  local text = tostring(msg.text or "")
  local target
  if msg.target then
    target = walk(function(o) return labelOf(o) == tostring(msg.target) end)
    if not target then error("no object named " .. tostring(msg.target), 0) end
    if not isField(target) then error(tostring(msg.target) .. " is not a native text field or text box", 0) end
  elseif msg.x then
    local x, y = toContent(msg.x, msg.y, msg.unit)
    for _, o in ipairs(hitList(stage, x, y, {})) do
      if isField(o) then target = o; break end
    end
    if not target then error("no native text field at that point", 0) end
  elseif keyboardFocus and keyboardFocus.parent and isField(keyboardFocus) then
    target = keyboardFocus
  else
    local fields = {}
    walk(function(o) if isField(o) and shown(o) then fields[#fields + 1] = o end end)
    if #fields == 1 then
      target = fields[1]
    elseif #fields > 1 then
      local names = {}
      for i, f in ipairs(fields) do names[i] = labelOf(f) or ("unnamed at " .. r1(f.x) .. "," .. r1(f.y)) end
      error("there are " .. #fields .. " text fields (" .. table.concat(names, ", ") .. "): pass target (its name) or x/y", 0)
    end
  end

  if not target then
    -- no text field anywhere: type as key presses (for apps that read the keyboard)
    for ch in text:gmatch("[%z\1-\127\194-\244][\128-\191]*") do
      local base = { name = "key", keyName = keyNameFor(ch), nativeKeyCode = 0,
                     isShiftDown = ch:match("%u") ~= nil, isCtrlDown = false, isAltDown = false, isCommandDown = false }
      base.phase = "down"; dispatch(Runtime, base)
      local up = {}; for k, v in pairs(base) do up[k] = v end
      up.phase = "up"; dispatch(Runtime, up)
    end
    return { mode = "key events (no text field found)", typed = text }
  end
  if target.isEditable == false then error("that text box is not editable", 0) end

  -- the same userInput events the keyboard would produce
  local function ev(t)
    t.name = "userInput"; t.target = target
    dispatch(target, t)
  end
  ev({ phase = "began" })
  local cur = target.text or ""
  if msg.replace ~= false and cur ~= "" then
    local old = cur
    target.text = ""; cur = ""
    ev({ phase = "editing", text = "", oldText = old, newCharacters = "", startPosition = 0, numDeleted = #old })
  end
  for ch in text:gmatch("[%z\1-\127\194-\244][\128-\191]*") do
    local old = cur
    cur = cur .. ch
    target.text = cur
    ev({ phase = "editing", text = cur, oldText = old, newCharacters = ch, startPosition = #old, numDeleted = 0 })
  end
  if msg.submit ~= false then ev({ phase = "submitted" }) end
  return { field = brief(target), text = target.text, submitted = msg.submit ~= false }
end

----------------------------------------------------------------------------
-- native.showAlert -> an in-app overlay the agent can see and tap
----------------------------------------------------------------------------
local alerts = {}

local function closeAlert(a, event)
  for i, x in ipairs(alerts) do if x == a then table.remove(alerts, i); break end end
  if a.group and a.group.removeSelf then a.group:removeSelf() end
  a.group = nil
  if alerts[1] and not alerts[1].group then alerts[1].show() end
  if type(a.listener) == "function" or type(a.listener) == "table" then
    event.name = "completion"
    local ok, err = xpcall(function()
      if type(a.listener) == "function" then a.listener(event) else a.listener:completion(event) end
    end, debug.traceback)
    if not ok then print("[solar2d-mcp] Runtime error in alert listener: " .. tostring(err)) end
  end
end

local function answerAlert(a, index)
  print(string.format("[solar2d-mcp] alert \"%s\": pressed %d (%s)", a.title, index, tostring(a.buttons[index])))
  closeAlert(a, { action = "clicked", index = index })
end

local function buildAlert(a)
  local W, Hh = display.actualContentWidth, display.actualContentHeight
  local ox, oy = display.screenOriginX, display.screenOriginY
  local fs = math.max(W * 0.042, 8)
  local pad = fs
  local pw = W * 0.8
  local g = display.newGroup()
  g.name = "solar2d-mcp alert"
  local shade = display.newRect(g, ox + W / 2, oy + Hh / 2, W, Hh)
  shade:setFillColor(0, 0, 0, 0.45)
  shade:addEventListener("touch", function() return true end)
  shade:addEventListener("tap", function() return true end)
  local panel = display.newRoundedRect(g, ox + W / 2, 0, pw, 10, fs * 0.6)
  panel:setFillColor(0.97, 0.97, 0.98)
  panel:setStrokeColor(0.6); panel.strokeWidth = 2
  local title = display.newText({ parent = g, text = a.title, width = pw - 2 * pad, fontSize = fs * 1.15,
                                  font = native.systemFontBold, align = "center" })
  title:setFillColor(0.1); title.name = "alert title"
  local body = display.newText({ parent = g, text = a.message, width = pw - 2 * pad, fontSize = fs, align = "center" })
  body:setFillColor(0.2); body.name = "alert message"
  local bh = fs * 2.4
  local h = pad + title.height + pad * 0.6 + body.height + pad + #a.buttons * (bh + pad * 0.5) + pad * 0.5
  local top = oy + (Hh - h) / 2
  panel.y = top + h / 2; panel.height = h
  title.x, title.y = panel.x, top + pad + title.height / 2
  body.x, body.y = panel.x, title.y + title.height / 2 + pad * 0.6 + body.height / 2
  local y = body.y + body.height / 2 + pad + bh / 2
  for i, label in ipairs(a.buttons) do
    local b = display.newRoundedRect(g, panel.x, y, pw - 2 * pad, bh, fs * 0.4)
    b:setFillColor(0.2, 0.45, 0.9)
    b.name = "alert button " .. i .. ": " .. tostring(label)
    local t = display.newText({ parent = g, text = tostring(label), x = b.x, y = b.y, fontSize = fs })
    t:setFillColor(1)
    b:addEventListener("touch", function(e)
      if e.phase == "ended" then answerAlert(a, i) end
      return true
    end)
    y = y + bh + pad * 0.5
  end
  a.group = g
end

do
  local origShow, origCancel = native.showAlert, native.cancelAlert
  native.showAlert = function(title, message, buttons, listener)
    if type(buttons) == "function" then buttons, listener = nil, buttons end
    if type(buttons) ~= "table" or #buttons == 0 then buttons = { "OK" } end
    local a = { title = tostring(title or ""), message = tostring(message or ""), buttons = buttons,
                listener = listener, isSolar2dMcpAlert = true }
    a.show = function() buildAlert(a) end
    alerts[#alerts + 1] = a
    if #alerts == 1 then a.show() end
    local labels = {}
    for i, b in ipairs(buttons) do labels[i] = tostring(b) end
    print(string.format("[solar2d-mcp] native.showAlert \"%s\": \"%s\" [%s] (shown in-app: tap a button or use answer_alert)",
      a.title, a.message, table.concat(labels, " | ")))
    return a
  end
  native.cancelAlert = function(ref)
    if type(ref) == "table" and ref.isSolar2dMcpAlert then
      closeAlert(ref, { action = "cancelled" })
      return
    end
    return origCancel(ref)
  end
  Runtime:addEventListener("enterFrame", function()
    local a = alerts[1]
    if a and a.group and a.group.toFront then a.group:toFront() end
  end)
end

H.alert = function(msg)
  local a = alerts[1]
  if not a then error("no alert is showing", 0) end
  if msg.button == nil then return { title = a.title, message = a.message, buttons = a.buttons } end
  local index = tonumber(msg.button)
  if not index then
    for i, b in ipairs(a.buttons) do
      if tostring(b):lower() == tostring(msg.button):lower() then index = i end
    end
  end
  if not index or not a.buttons[index] then
    error("the alert has no button " .. tostring(msg.button) .. " (buttons: " .. table.concat(a.buttons, ", ") .. ")", 0)
  end
  answerAlert(a, index)
  return { pressed = a.buttons[index], index = index, title = a.title }
end

----------------------------------------------------------------------------
-- wait until a Lua expression is true
----------------------------------------------------------------------------
H.wait = function(msg, reply)
  local f, err = loadstring("return " .. tostring(msg.lua), "=wait_for")
  if not f then error(err, 0) end
  local t0 = system.getTimer()
  local timeout = tonumber(msg.timeout_ms) or 5000
  local function check()
    local ok, v = pcall(f)
    local el = math.floor(system.getTimer() - t0)
    if ok and v then
      Runtime:removeEventListener("enterFrame", check)
      reply(true, { met = true, after_ms = el, value = dump(v, 2, {}) })
    elseif el >= timeout then
      Runtime:removeEventListener("enterFrame", check)
      reply(true, { met = false, after_ms = el, last_error = (not ok) and tostring(v) or nil })
    end
  end
  Runtime:addEventListener("enterFrame", check)
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
  -- Solar2D prints the error and stack trace itself; returning true only stops the
  -- blocking error dialog. When stdout isn't live, pass errors on (once per 2 s each).
  local lastErr, lastT = nil, -1e9
  Runtime:addEventListener("unhandledError", function(e)
    local m = tostring(e.errorMessage)
    if forwardPrint and client and (m ~= lastErr or system.getTimer() - lastT > 2000) then
      send({ type = "error", message = m, stack = tostring(e.stackTrace) })
      lastErr, lastT = m, system.getTimer()
    end
    return true
  end)
end

return M
