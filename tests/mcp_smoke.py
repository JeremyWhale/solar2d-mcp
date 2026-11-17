#!/usr/bin/env python3
"""Linux-side smoke test: drive solar2d-mcp over stdio like an MCP client would.

usage: mcp_smoke.py <out_dir> <project_path_as_server_sees_it> -- <server command...>
Saves screenshots into <out_dir>. (Dev-only helper; nothing here ships to Windows.)
"""
import base64, json, os, subprocess, sys, time

out_dir = sys.argv[1]
project = sys.argv[2]
cmd = sys.argv[sys.argv.index("--") + 1:]
os.makedirs(out_dir, exist_ok=True)

p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=sys.stderr, bufsize=0)
nid = 0


def rpc(method, params=None, notify=False):
    global nid
    msg = {"jsonrpc": "2.0", "method": method}
    if params is not None:
        msg["params"] = params
    if not notify:
        nid += 1
        msg["id"] = nid
    p.stdin.write((json.dumps(msg) + "\n").encode())
    p.stdin.flush()
    if notify:
        return None
    while True:
        line = p.stdout.readline()
        if not line:
            raise SystemExit("server closed stdout")
        r = json.loads(line)
        if r.get("id") == nid:
            if "error" in r:
                raise SystemExit(f"RPC error: {r['error']}")
            return r["result"]


shot_no = 0


def call(name, **args):
    global shot_no
    t = time.time()
    r = rpc("tools/call", {"name": name, "arguments": args})
    dt = time.time() - t
    flag = " (isError)" if r.get("isError") else ""
    print(f"\n=== {name} {json.dumps(args)} [{dt:.1f}s]{flag}")
    for c in r.get("content", []):
        if c["type"] == "text":
            print(c["text"][:3000])
        elif c["type"] == "image":
            shot_no += 1
            path = os.path.join(out_dir, f"shot{shot_no}.png")
            open(path, "wb").write(base64.b64decode(c["data"]))
            print(f"[image saved: {path}]")
    return r


def text(r):
    return "\n".join(c.get("text", "") for c in r.get("content", []) if c["type"] == "text")


init = rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                          "clientInfo": {"name": "smoke", "version": "0"}})
print("server:", init.get("serverInfo"), "protocol", init.get("protocolVersion"))
rpc("notifications/initialized", notify=True)
tools = rpc("tools/list")["tools"]
print("tools:", ", ".join(t["name"] for t in tools))

failures = []


def check(cond, what):
    print(("PASS " if cond else "FAIL ") + what)
    if not cond:
        failures.append(what)


r = call("run_project", project=project)
check("bridge connected" in text(r) and "testapp ready" in text(r), "run_project connects and shows logs")
call("status")
call("screenshot", max_size=600)
tree = call("display_tree")
check("playButton" in text(tree) and "touch" in text(tree), "display_tree lists playButton with touch listener")
t = json.loads(text(tree))
btn = next(c for c in t["stage"]["children"] if c.get("name") == "playButton")
bx = (btn["bounds"][0] + btn["bounds"][2]) / 2
by = (btn["bounds"][1] + btn["bounds"][3]) / 2
call("tap", x=bx, y=by, unit="content")
# second tap in screenshot pixels (last screenshot was 270x600 for a 1080x2400 screen)
call("tap", x=bx / 4, y=by / 4)
r = call("eval_lua", code="testState.score")
check(text(r).strip() == "2", "two taps -> score 2")
r = call("read_logs", contains="button pressed")
check("score 2" in text(r), "read_logs shows the button prints")
box = next(c for c in t["stage"]["children"] if c.get("name") == "dragBox")
call("drag", x1=box["x"], y1=box["y"], x2=box["x"] + 150, y2=box["y"] + 100, unit="content", duration_ms=400)
r = call("eval_lua", code="testState.boxX")
check(abs(float(text(r)) - (box["x"] + 150)) < 2, "drag moved the box")
call("key", key="space")
r = call("eval_lua", code="testState.lastKey")
check(text(r).strip() == '"space"', "key event delivered")
shot = call("screenshot", max_size=600)
check(any(c["type"] == "image" for c in shot["content"]), "screenshot returns an image")
# tap the error button using screenshot pixel coords (percent of the 600-high image)
bomb = next(c for c in t["stage"]["children"] if c.get("name") == "errorButton")
r = call("tap", x=bomb["x"], y=bomb["y"], unit="content")
check(r.get("isError") and "threw" in text(r), "tap reports the listener's error")
r = call("read_logs", max_lines=15)
check("Runtime error in errorButton tap listener" in text(r), "listener error is logged")
call("eval_lua", code="timer.performWithDelay(1, function() local x = nil; return x.y end)")
time.sleep(1)
r = call("read_logs", max_lines=6)
check("Runtime error" in text(r) and "attempt to index" in text(r), "unhandled runtime error is logged (no dialog)")
r = call("eval_lua", code="1 + 1")
check(text(r).strip() == "2", "app still alive after the error")
r = call("eval_lua", code="error('boom')")
check(r.get("isError") and "boom" in text(r), "eval_lua reports Lua errors")
r = call("relaunch")
check("bridge connected" in text(r), "relaunch reconnects")
r = call("eval_lua", code="testState.score")
check(text(r).strip() == "0", "relaunch reset the app")
t0 = time.time()
r = call("stop")
check("stopped" in text(r) and time.time() - t0 < 3, "stop (quick)")
r = call("remove_bridge", project=project)
check("removed" in text(r), "remove_bridge cleans main.lua")

p.stdin.close()
p.wait(timeout=20)
print("\nRESULT:", "ALL PASSED" if not failures else f"{len(failures)} FAILED: {failures}")
sys.exit(1 if failures else 0)
