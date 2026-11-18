"""Dev-only, run by update_test.sh: the update notice, the opt-out, and updating while running."""
import json, os, subprocess, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from mcp_client import Client

install, cur = os.environ["INSTALL"], os.environ["CUR"]
exe = os.path.join(install, "solar2d-mcp.exe")
wine = os.environ["WINE"]
prefix = os.environ["WINEPREFIX"]
fails = 0


def check(ok, what):
    global fails
    print(("PASS " if ok else "FAIL ") + what)
    fails += 0 if ok else 1


def clear_cache():
    subprocess.run(["find", os.path.realpath(prefix + "/drive_c/users"), "-name", "update-check.txt", "-delete"])


def status(c):
    return json.loads(c.call("status")["text"])


clear_cache()
os.environ["SOLAR2D_MCP_NO_UPDATE_CHECK"] = "1"
c = Client([wine, exe], "/tmp/solar2d-mcp-update-out", quiet=True); time.sleep(4)
s = status(c); c.close()
check(s["version"] == "0.1.9" and "update_available" not in s, "opt-out: no check, no notice")
del os.environ["SOLAR2D_MCP_NO_UPDATE_CHECK"]

clear_cache()
c = Client([wine, exe], "/tmp/solar2d-mcp-update-out", quiet=True)
t0 = time.time(); s = status(c)
check(time.time() - t0 < 3, "server answers at once (the check doesn't delay it)")
for _ in range(40):
    s = status(c)
    if "update_available" in s:
        break
    time.sleep(0.5)
check(f"v{cur}" in s.get("update_available", "") and "--update" in s.get("update_available", ""), "status shows the update notice")

r = subprocess.run([wine, exe, "--update"], capture_output=True, text=True)
check(f"Updated to v{cur}" in r.stderr, "--update works while the server is running")
check(status(c)["version"] == "0.1.9", "  ...the running server keeps working")
c.close(); time.sleep(2)

c = Client([wine, exe], "/tmp/solar2d-mcp-update-out", quiet=True); time.sleep(1)
s = status(c)
check(s["version"] == cur and "update_available" not in s, "next start runs the new version, no notice")
c.close(); time.sleep(1)
left = sorted(os.listdir(install))
check(left == ["solar2d-mcp.exe"], f"old copies cleaned up ({left})")
sys.exit(1 if fails else 0)
