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
| `tap` | Touch began/ended + tap at a point, delivered to the topmost objects there (`hold_ms` for long presses) |
| `drag` | Touch began → moved every frame → ended (swipes, sliders, scroll views, picker wheels) |
| `touch_down` / `touch_move` / `touch_up` | Fingers that stay down between calls: press and hold while taking screenshots or doing other things, slide off a button, hold a game control, multi-touch with named fingers (`finger: "all"` lifts every finger) |
| `key` | Key down/up to Runtime `key` listeners |
| `type_text` | Type into a native text field/box: the `userInput` events a keyboard sends (began, editing per character, submitted) |
| `answer_alert` | Read or answer a `native.showAlert` (shown as an in-app overlay; its buttons can also be tapped) |
| `wait_for` | Wait for a Lua condition and/or a log line (after transitions, before screenshots) |
| `display_tree` | Display hierarchy: names, kinds (incl. widget type, text fields), text, bounds, visibility, offscreen/clipped, listeners, composer scene |
| `eval_lua` | Run Lua inside the app and get the result (read or change any variable) |
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
- Input follows Solar2D's rules: topmost object first, propagating down until a
  listener returns true, then Runtime; `setFocus` is honoured (a focused touch goes to
  the focus object and up through its parent groups); scroll view / container clipping
  is respected. Tested with every `widget` library widget (button, switch, slider,
  segmented control, stepper, scroll view, table view, picker wheel, tab bar).
- Multi-touch follows the app: until it calls `system.activate("multitouch")`, a second
  finger (held, tap or drag) is refused with an explanation, as a real device would
  ignore it. A touch released close to where it began also sends a `tap` event.
- `native.showAlert` is replaced (simulator only, while the bridge is in) by an in-app
  overlay, so the agent can see it in screenshots and press its buttons; the app's
  listener gets the normal `completion` event. `native.cancelAlert` works too.
- Native text fields are separate OS controls, so they don't appear in screenshots;
  `display_tree` lists them (kind `textfield`/`textbox`) and `type_text` fills them.
- A line repeated more than 5 times in 2 s (an error or print every frame) is collapsed
  into one "N more copies hidden" note so it can't bury the rest of the log.
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

## Updating

```
C:\Tools\solar2d-mcp\solar2d-mcp.exe --update
```

Downloads the latest release and swaps it in (fine while Codex is open; the new version
starts the next time Codex launches the server). `--check-update` only reports.

About once a day the server checks for a new release and, if there is one, `status` and
`run_project` mention it so the agent can tell you. Nothing is installed unless you run
`--update`. The check only downloads a small file from this repo's GitHub releases, using
the Windows certificate store and proxy setting (or `HTTPS_PROXY`). Set
`SOLAR2D_MCP_NO_UPDATE_CHECK=1` to turn it off.

Each release has a manifest (version, size, SHA-256 of the exe) signed with an Ed25519 key
held only by the maintainer. `--update` refuses a bad signature, a checksum mismatch, an
older version, or an exe that doesn't start and report the promised version.

## Building (on Linux)

```
tools/build-windows.sh        # -> target/x86_64-pc-windows-gnu/release/solar2d-mcp.exe
tests/run_smoke_wine.sh       # end-to-end test: the .exe + Solar2D 3729 under Wine, hidden Xvfb display
tests/with_wine_display.sh python3 -I "$PWD/tests/widget_smoke.py"   # widgets, typing, alerts
tests/update_test.sh          # --update against a local server: good and hostile updates
tools/release.sh --publish    # build, run all tests, sign, tag and create the GitHub release
```

Needs Rust with the `x86_64-pc-windows-gnu` target, `cargo-zigbuild` and Zig.
The dev scripts expect a tools folder (Zig, portable Wine, Xvfb, Solar2D installed in
Wine) with an `env.sh` that sets `CARGO_HOME`, `PATH`, `WINEPREFIX` and `WINE`; point
`SOLAR2D_MCP_TOOLS` at it.
`tests/mcp_smoke.py` is a dev-only test client and is not part of the Windows build.

## Licence

MIT, see [LICENSE](LICENSE).
