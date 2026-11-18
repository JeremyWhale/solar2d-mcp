"""Tiny MCP stdio client for dev tests (Linux only; never shipped)."""
import base64, json, os, subprocess, sys, time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(REPO, "target", "x86_64-pc-windows-gnu", "release", "solar2d-mcp.exe")


def wine_path(path):
    """Linux path -> the Z: path the Windows build sees under Wine."""
    return "Z:" + os.path.abspath(path).replace("/", "\\")


class Client:
    def __init__(self, cmd, out_dir, quiet=False):
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0)
        self.nid, self.shots, self.out_dir, self.quiet = 0, 0, out_dir, quiet
        os.makedirs(out_dir, exist_ok=True)
        self.init = self.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                            "clientInfo": {"name": "devtest", "version": "0"}})
        self.rpc("notifications/initialized", notify=True)
        self.failures = []

    def rpc(self, method, params=None, notify=False):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if not notify:
            self.nid += 1
            msg["id"] = self.nid
        self.p.stdin.write((json.dumps(msg) + "\n").encode())
        self.p.stdin.flush()
        if notify:
            return None
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise SystemExit("server closed stdout")
            r = json.loads(line)
            if r.get("id") == self.nid:
                if "error" in r:
                    raise SystemExit(f"RPC error: {r['error']}")
                return r["result"]

    def call(self, name, **args):
        t = time.time()
        r = self.rpc("tools/call", {"name": name, "arguments": args})
        txt = "\n".join(c.get("text", "") for c in r.get("content", []) if c["type"] == "text")
        r["text"] = txt
        flag = " (isError)" if r.get("isError") else ""
        if not self.quiet:
            print(f"\n=== {name} {json.dumps(args)} [{time.time() - t:.1f}s]{flag}")
            print(txt[:2500])
        for c in r.get("content", []):
            if c["type"] == "image":
                self.shots += 1
                path = os.path.join(self.out_dir, f"shot{self.shots}.png")
                open(path, "wb").write(base64.b64decode(c["data"]))
                r["image_path"] = path
                if not self.quiet:
                    print(f"[image saved: {path}]")
        return r

    def check(self, cond, what):
        print(("PASS " if cond else "FAIL ") + what)
        if not cond:
            self.failures.append(what)

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=20)


def find(node, name):
    """Find a node in a display_tree result by name (depth first)."""
    if node.get("name") == name:
        return node
    for c in node.get("children", []):
        f = find(c, name)
        if f:
            return f


def center(node):
    b = node["bounds"]
    return (b[0] + b[2]) / 2, (b[1] + b[3]) / 2
