-- solar2d-mcp test app: a button, a draggable box, a key listener and an error trigger
display.setDefault("background", 0.1, 0.12, 0.2)
print("testapp starting")

local score = 0
_G.testState = { score = 0, lastKey = nil, boxX = nil }

local title = display.newText({ text = "Score: 0", x = display.contentCenterX, y = 80, fontSize = 40 })
title.name = "scoreLabel"

local button = display.newRoundedRect(display.contentCenterX, 300, 260, 90, 16)
button:setFillColor(0.2, 0.7, 0.3)
button.name = "playButton"
local label = display.newText({ text = "TAP ME", x = button.x, y = button.y, fontSize = 32 })
label.name = "buttonLabel"
button:addEventListener("touch", function(e)
  if e.phase == "began" then
    display.getCurrentStage():setFocus(e.target, e.id)
  elseif e.phase == "ended" then
    display.getCurrentStage():setFocus(nil, e.id)
    score = score + 1
    _G.testState.score = score
    title.text = "Score: " .. score
    print("button pressed, score " .. score)
  end
  return true
end)

local box = display.newRect(display.contentCenterX, 600, 120, 120)
box:setFillColor(0.9, 0.4, 0.1)
box.name = "dragBox"
box:addEventListener("touch", function(e)
  local t = e.target
  if e.phase == "began" then
    display.getCurrentStage():setFocus(t, e.id)
    t.x0, t.y0 = e.x - t.x, e.y - t.y
  elseif e.phase == "moved" and t.x0 then
    t.x, t.y = e.x - t.x0, e.y - t.y0
  elseif e.phase == "ended" or e.phase == "cancelled" then
    display.getCurrentStage():setFocus(nil, e.id)
    _G.testState.boxX = t.x
    print(string.format("box dropped at %.0f,%.0f", t.x, t.y))
  end
  return true
end)

local bomb = display.newCircle(display.contentCenterX, 850, 50)
bomb:setFillColor(0.8, 0.1, 0.1)
bomb.name = "errorButton"
bomb:addEventListener("tap", function()
  print("about to fail")
  local t = nil
  return t.boom -- runtime error on purpose
end)

Runtime:addEventListener("key", function(e)
  if e.phase == "down" then _G.testState.lastKey = e.keyName; print("key " .. e.keyName) end
end)

print("testapp ready")
