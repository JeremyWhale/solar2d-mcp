-- solar2d-mcp widget test app: every widget-library widget, a native text field and a native alert.
-- Each control records what happened in the global table W.
local widget = require("widget")
display.setDefault("background", 0.95, 0.95, 0.97)
W = { button = 0, switch = nil, slider = nil, segment = nil, stepper = nil, row = nil,
      scrollButton = 0, tab = nil, picker = nil, field = "", submitted = nil, alert = nil, longPress = 0 }

local cx = display.contentCenterX
local function y(n) return 100 + n * 170 end

local btn = widget.newButton({ id = "okButton", label = "OK", x = cx, y = y(0), width = 300, height = 100,
  shape = "roundedRect", fillColor = { default = { 0.2, 0.5, 0.9 }, over = { 0.1, 0.3, 0.7 } },
  labelColor = { default = { 1 }, over = { 0.8 } }, fontSize = 40,
  onRelease = function(e) W.button = W.button + 1; print("button released " .. W.button) end })

local sw = widget.newSwitch({ id = "soundSwitch", x = cx - 250, y = y(1), style = "onOff",
  onRelease = function(e) W.switch = e.target.isOn; print("switch " .. tostring(e.target.isOn)) end })

local slider = widget.newSlider({ id = "volumeSlider", x = cx + 120, y = y(1), width = 500, value = 10,
  listener = function(e) W.slider = e.value end })

local seg = widget.newSegmentedControl({ id = "modeSeg", x = cx, y = y(2), segmentWidth = 200,
  segments = { "Easy", "Normal", "Hard" }, defaultSegment = 1,
  onPress = function(e) W.segment = e.target.segmentLabel; print("segment " .. e.target.segmentLabel) end })

local stepper = widget.newStepper({ id = "livesStepper", x = cx, y = y(3), initialValue = 3, minimumValue = 0, maximumValue = 9,
  onPress = function(e) W.stepper = e.value; print("stepper " .. e.phase .. " " .. e.value) end })

local sv = widget.newScrollView({ id = "scroller", x = cx, y = y(4) + 60, width = 800, height = 260,
  scrollWidth = 800, scrollHeight = 1200, horizontalScrollDisabled = true, backgroundColor = { 0.85, 0.9, 1 } })
for i = 1, 8 do
  local t = display.newText({ text = "Scroll line " .. i, x = 400, y = i * 140 - 70, fontSize = 36 })
  t:setFillColor(0); t.name = "scrollLine" .. i
  sv:insert(t)
end
local inner = widget.newButton({ id = "innerButton", label = "Inner", x = 650, y = 70, width = 200, height = 80,
  shape = "rect", fillColor = { default = { 0.3, 0.7, 0.3 }, over = { 0.2, 0.5, 0.2 } },
  onRelease = function() W.scrollButton = W.scrollButton + 1; print("inner button " .. W.scrollButton) end })
sv:insert(inner)

local tv = widget.newTableView({ id = "levelTable", x = cx, y = y(6) + 60, width = 800, height = 300,
  onRowRender = function(e)
    local row = e.row
    local t = display.newText({ parent = row, text = "Level " .. row.index, x = 40, y = row.contentHeight / 2, fontSize = 36 })
    t.anchorX = 0; t:setFillColor(0)
  end,
  onRowTouch = function(e)
    print("row " .. e.phase .. " " .. e.target.index)
    if e.phase == "release" or e.phase == "tap" then W.row = e.target.index end
  end })
for i = 1, 20 do tv:insertRow({ rowHeight = 90 }) end

local colors = { "Red", "Green", "Blue", "Yellow", "Purple" }
local picker
picker = widget.newPickerWheel({ id = "colorPicker", left = 40, top = y(8) - 60, style = "resizable", width = 500, rowHeight = 60,
  fontSize = 32, columns = { { align = "center", labels = colors, startIndex = 1 } },
  onValueSelected = function(e) W.picker = colors[e.row]; print("picker " .. tostring(W.picker)) end })

local field = native.newTextField(cx + 260, y(8) + 10, 380, 80)
field.name = "nameField"; field.placeholder = "Your name"
field:addEventListener("userInput", function(e)
  if e.phase == "editing" then W.field = e.text
  elseif e.phase == "submitted" or e.phase == "ended" then W.submitted = e.target.text; print("field " .. e.phase .. ": " .. e.target.text) end
end)

local alertBtn = widget.newButton({ id = "quitButton", label = "Quit?", x = cx - 250, y = y(10), width = 300, height = 90,
  shape = "roundedRect", fillColor = { default = { 0.8, 0.3, 0.3 }, over = { 0.6, 0.2, 0.2 } }, labelColor = { default = { 1 } },
  onRelease = function()
    native.showAlert("Quit game?", "Progress will be lost.", { "Cancel", "Quit" }, function(e)
      if e.action == "clicked" then W.alert = e.index; print("alert clicked " .. e.index) end
    end)
  end })

local hold = display.newRect(cx + 250, y(10), 300, 90); hold:setFillColor(0.5, 0.3, 0.7); hold.name = "holdPad"
local holdTimer
hold:addEventListener("touch", function(e)
  if e.phase == "began" then
    display.getCurrentStage():setFocus(e.target, e.id)
    holdTimer = timer.performWithDelay(500, function() W.longPress = W.longPress + 1; print("long press") end)
  elseif e.phase == "ended" or e.phase == "cancelled" then
    display.getCurrentStage():setFocus(nil, e.id)
    if holdTimer then timer.cancel(holdTimer); holdTimer = nil end
  end
  return true
end)

local tabs = widget.newTabBar({ id = "tabs", left = 0, top = display.contentHeight - 120, width = display.contentWidth, height = 120,
  buttons = {
    { id = "home", label = "Home", size = 30, selected = true, onPress = function() W.tab = "home"; print("tab home") end },
    { id = "shop", label = "Shop", size = 30, onPress = function() W.tab = "shop"; print("tab shop") end },
    { id = "settings", label = "Settings", size = 30, onPress = function() W.tab = "settings"; print("tab settings") end },
  } })

print("widgetapp ready")
