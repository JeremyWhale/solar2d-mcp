#!/usr/bin/env python3
"""Widget-library + text input + alert test for solar2d-mcp (dev-only, Linux/Wine)."""
import json, os, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from mcp_client import Client, EXE, find, center

PROJECT = r'Z:\media\jeremy\1TB SSD\solar2d-mcp\tests\widgetapp'
c = Client([os.environ["WINE"], EXE], os.path.join(os.path.dirname(os.path.abspath(__file__)), "out", "widgets"), quiet=True)


def W(k):
    return c.call("eval_lua", code="W." + k)["text"].strip()


def tap(x, y, **kw):
    return c.call("tap", x=x, y=y, unit=kw.pop("unit", "content"), **kw)


r = c.call("run_project", project=PROJECT)
c.check("widgetapp ready" in r["text"], "widget app starts")
tree = json.loads(c.call("display_tree")["text"])
st = tree["stage"]
c.check(find(st, "okButton")["kind"] == "widget:button", "display_tree names widget kinds")
c.check(find(st, "nameField")["kind"] == "textfield", "display_tree marks native text fields")
c.check(find(st, "scrollLine8").get("offscreen") is True and "offscreen" not in find(st, "scrollLine1"),
        "rows scrolled out of a scroll view are flagged offscreen")

tap(*center(find(st, "okButton")));                       c.check(W("button") == "1", "button onRelease")
tap(*center(find(st, "soundSwitch")))
c.check("condition true" in c.call("wait_for", lua="W.switch ~= nil", timeout_ms=3000)["text"] and W("switch") == "true", "switch toggles")
c.call("drag", x1=461, y1=270, x2=800, y2=270, unit="content", duration_ms=400)
c.check(float(W("slider")) > 50, "slider drag")
tap(740, 440);                                            c.check(W("segment") == '"Hard"', "segmented control")
tap(565, 610); time.sleep(1.2)
c.check(W("stepper") == "4", "stepper + once (no runaway auto-repeat)")
tap(*center(find(st, "innerButton")));                    c.check(W("scrollButton") == "1", "button inside scroll view")
tap(540, 960)  # inside the scroll view, over hidden content: must not hit offscreen rows
c.call("drag", x1=540, y1=930, x2=540, y2=730, unit="content", duration_ms=400)
c.call("wait_for", timeout_ms=1500)
pos = c.call("eval_lua", code="(function() local s=display.getCurrentStage(); for i=1,s.numChildren do if s[i].id=='scroller' then local x,y=s[i]:getContentPosition(); return y end end end)()")["text"]
c.check(float(pos) < -100, "scroll view scrolls with a drag")
tap(540, 1165)
c.check("condition true" in c.call("wait_for", lua="W.row ~= nil", timeout_ms=3000)["text"] and W("row") == "2", "table view row tap")
c.call("drag", x1=290, y1=1560, x2=290, y2=1440, unit="content", duration_ms=400)
r = c.call("wait_for", lua="W.picker ~= nil", timeout_ms=5000)
c.check("condition true" in r["text"] and W("picker") not in ('"Red"', "nil"), "picker wheel drag selects a value")
tap(540, 2340);                                           c.check(W("tab") == '"shop"', "tab bar")
tap(790, 1800);                                           c.check(W("longPress") == "0", "short tap is not a long press")
tap(790, 1800, hold_ms=800);                              c.check(W("longPress") == "1", "tap hold_ms=800 is a long press")

# ---- press and hold across calls
okc = center(find(st, "okButton"))
c.call("touch_down", x=okc[0], y=okc[1], unit="content")
mid = c.call("screenshot", max_size=600)
c.check(W("button") == "1" and "image_path" in mid, "touch_down holds OK down (no release yet, screenshot mid-press)")
stat = c.call("status")["text"]
c.check('"fingers_down"' in stat and '"finger": "1"' in stat, "status lists the held finger")
c.call("touch_up");                                       c.check(W("button") == "2", "touch_up releases OK -> onRelease")
c.call("touch_down", x=okc[0], y=okc[1], unit="content")
c.call("touch_move", x=okc[0], y=okc[1] + 300, unit="content", duration_ms=200)
c.call("touch_up");                                       c.check(W("button") == "2", "press OK, slide off, release -> no onRelease")
before = int(W("stepper"))
c.call("touch_down", x=565, y=610, unit="content")
c.call("wait_for", timeout_ms=1500)
during = int(W("stepper"))
c.call("touch_up"); after = int(W("stepper")); time.sleep(0.8)
c.check(during > before + 1 and int(W("stepper")) == after, f"holding stepper + auto-repeats ({before}->{during}) and stops on release")

# ---- second finger: refused until the app activates multitouch
hp = center(find(st, "holdPad"))
c.call("touch_down", x=hp[0], y=hp[1], unit="content", finger="hold")
r = c.call("touch_down", x=okc[0], y=okc[1], unit="content", finger="2")
c.check(r.get("isError") and "multitouch" in r["text"], "second finger refused without system.activate('multitouch')")
r = c.call("tap", x=okc[0], y=okc[1], unit="content")
c.check(r.get("isError") and "multitouch" in r["text"], "tap refused while a finger is held (no multitouch)")
c.call("touch_up", finger="all")
c.call("eval_lua", code="system.activate('multitouch')")
lp = int(W("longPress"))
tree3 = json.loads(c.call("display_tree")["text"])
handle = find(tree3["stage"], "volumeSlider")["children"][-1]
hx, hy = center(handle)
slider0 = float(W("slider"))
c.call("touch_down", x=hp[0], y=hp[1], unit="content", finger="hold")
c.call("touch_down", x=hx, y=hy, unit="content", finger="slide")
c.call("touch_move", x=450, y=hy, unit="content", finger="slide", duration_ms=300)
c.call("touch_up", finger="slide")
c.call("wait_for", timeout_ms=700)
c.check(float(W("slider")) < slider0 - 20 and int(W("longPress")) == lp + 1,
        "multitouch: hold the pad (long press fires) while sliding the slider with a second finger")
c.call("touch_up", finger="hold")
r = c.call("touch_up", finger="hold")
c.check(r.get("isError") and "not down" in r["text"], "touch_up on a finger that isn't down explains")

r = c.call("type_text", text="Jeremy Ü")
c.check(not r.get("isError") and W("submitted") == '"Jeremy Ü"' and W("field") == '"Jeremy Ü"', "type_text into the only text field (+ submitted)")
r = c.call("type_text", text="!", replace=False, submit=False, target="nameField")
c.check(W("field") == '"Jeremy Ü!"', "type_text append by name without submit")

buttons_before_alert = W("button")
tap(*center(find(st, "quitButton")))
r = c.call("wait_for", log_contains="native.showAlert", since=0, timeout_ms=3000)
c.check("Quit game?" in r["text"], "showAlert is logged")
shot = c.call("screenshot", max_size=900)
c.check("image_path" in shot, "screenshot with alert overlay")
r = c.call("answer_alert")
c.check('"Quit"' in r["text"], "answer_alert reads the alert")
c.call("answer_alert", button="Quit");                    c.check(W("alert") == "2", "answer_alert presses Quit (index 2)")
tap(*center(find(st, "quitButton")))
c.call("wait_for", lua="(function() local s=display.getCurrentStage(); return s[s.numChildren].name == 'solar2d-mcp alert' end)()", timeout_ms=3000)
t2 = json.loads(c.call("display_tree")["text"])
cancel = find(t2["stage"], "alert button 1: Cancel")
c.check(cancel is not None, "alert buttons appear in display_tree")
tap(*center(cancel));                                     c.check(W("alert") == "1", "tapping the overlay's Cancel button")
c.check(W("button") == buttons_before_alert, "taps on the alert didn't reach the app behind it")

# an error every frame must not drown the logs or stall the bridge
c.call("eval_lua", code="_G.flood = function() local x = nil; return x.y end; Runtime:addEventListener('enterFrame', _G.flood)")
time.sleep(3)
t0 = time.time(); r = c.call("eval_lua", code="1 + 1")
c.check(r["text"].strip() == "2" and time.time() - t0 < 3, "bridge stays responsive during an every-frame error")
c.call("eval_lua", code="Runtime:removeEventListener('enterFrame', _G.flood)")
time.sleep(2.5)
logs = c.call("read_logs", max_lines=100000)["text"]
n = logs.count("attempt to index local 'x'")
c.check(0 < n <= 12 and "copies of the lines above hidden" in logs, f"repeated errors collapsed ({n} shown)")

c.call("stop")
c.call("remove_bridge", project=PROJECT)
c.close()
print("\nRESULT:", "ALL PASSED" if not c.failures else f"{len(c.failures)} FAILED: {c.failures}")
sys.exit(1 if c.failures else 0)
