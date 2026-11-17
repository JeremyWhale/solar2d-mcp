# solar2d-mcp

An MCP server that lets an AI agent (Codex, Claude Code, Cursor…) run, watch and
drive a Solar2D project in the **Solar2D Simulator**. It ships as one Windows
`.exe` with no runtime to install (no Python, no Node).

Tested against **Solar2D 2026.3729** (the Windows simulator, run under Wine
during development).

## Tools

| Tool | What it does |
|---|---|
| `run_project` | Start a project in the simulator; waits for it and returns status + first logs |
| `relaunch` | Restart the current project (after editing code) |
| `stop` | Close the simulator |
| `status` | Running? which project, bridge connected, screen/content size |
| `read_logs` | `print()` output, warnings, runtime errors with stack traces (`since` cursor for only-new lines) |
| `screenshot` | PNG of the screen right now (downscaled, default 1024 px longest side) |
| `tap` | Touch began/ended + tap at a point, delivered to the topmost objects there |
| `drag` | Touch began → moved every frame → ended |
| `key` | Key down/up to Runtime `key` listeners |
| `display_tree` | Display hierarchy: names, kinds, text, bounds, visibility, listeners, composer scene |
| `eval_lua` | Run Lua inside the app and get the result |
| `remove_bridge` | Take the bridge back out of a project |

Coordinates for `tap`/`drag` default to **pixels in the last screenshot**;
`unit: "content"` uses Solar2D content coordinates (what `display_tree` reports),
`unit: "percent"` is 0–100 across the screen.

## How it works

- The simulator is started as `Corona Simulator.exe -no-console <project>\main.lua`
  and its console output is read straight from stdout.
- `run_project` writes `solar2d_mcp_bridge.lua` into the project and puts one
  statement at the very start of `main.lua`'s first line (same line, so error
  line numbers don't shift):
  `if system.getInfo("environment") == "simulator" then pcall(require, "solar2d_mcp_bridge") end; `
  The bridge only runs in the simulator (on a device it does nothing). It connects
  back to the server over `127.0.0.1` with LuaSocket and handles screenshot, input,
  tree and eval commands. Run `remove_bridge` before committing, or add
  `solar2d_mcp_bridge.lua` to `.gitignore`.
- Lua runtime errors are logged instead of opening the simulator's blocking error
  dialog (`suppress_error_dialogs: false` to keep the dialog).
- Before each launch it clears the simulator's "crashed last time it was run" flag
  (`HKCU\Software\Ansca Corona\Corona Simulator\Preferences\lastRunSucceeded`),
  since `stop` has to end the process (`native.requestExit()` only closes the
  project, not the simulator).
- If the simulator's stdout turns out not to arrive live, the bridge forwards
  `print()` itself (`status` shows `logs_source`).

## Setup on Windows with Codex

1. Copy `solar2d-mcp.exe` somewhere, e.g. `C:\Tools\solar2d-mcp\solar2d-mcp.exe`.
2. Add it to Codex, either:

   ```
   codex mcp add solar2d -- C:\Tools\solar2d-mcp\solar2d-mcp.exe
   ```

   or in `%USERPROFILE%\.codex\config.toml`:

   ```toml
   [mcp_servers.solar2d]
   command = 'C:\Tools\solar2d-mcp\solar2d-mcp.exe'
   # only if the simulator isn't in C:\Program Files (x86)\Corona Labs\Corona\
   # args = ["--simulator", 'D:\Solar2D\Corona Simulator.exe']
   startup_timeout_sec = 20
   tool_timeout_sec = 120
   ```

3. Ask Codex to run your project ("run the project in C:\work\mygame in the
   Solar2D simulator and take a screenshot").

The simulator is found automatically in `Program Files (x86)\Corona Labs\Corona`;
otherwise pass `--simulator <path>` or set `SOLAR2D_SIMULATOR`.

## Building (on Linux)

```
tools/build-windows.sh        # -> target/x86_64-pc-windows-gnu/release/solar2d-mcp.exe
tests/run_smoke_wine.sh       # end-to-end test: the .exe + Solar2D 3729 under Wine, hidden Xvfb display
```

Needs Rust with the `x86_64-pc-windows-gnu` target, `cargo-zigbuild` and Zig.
The dev toolchain (Zig, portable Wine, Xvfb, the Solar2D install) lives in
`/media/jeremy/1TB SSD/solar2d-mcp-tools`; `source` its `env.sh` first.
`tests/mcp_smoke.py` is a dev-only test client and is not part of the Windows build.
