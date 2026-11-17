//! solar2d-mcp: an MCP server that lets an AI agent run, watch and drive
//! Solar2D projects in the Solar2D Simulator.

mod bridge;
mod sim;
mod state;

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::state::{Shared, State};

const INSTRUCTIONS: &str = "\
Solar2D Simulator control. Typical loop: run_project (starts the simulator and connects \
a small Lua bridge inside the app) -> read_logs / screenshot -> tap, drag, key, \
display_tree, eval_lua -> edit code -> relaunch. \
run_project adds one line at the very start of main.lua and writes solar2d_mcp_bridge.lua \
next to it (both only act in the simulator; remove_bridge takes them out). \
Tap/drag coordinates default to pixels in the last screenshot; display_tree gives \
object bounds in content units (use unit \"content\" for those). \
type_text types into native text fields; native.showAlert appears as an in-app overlay \
(tap its buttons or use answer_alert). After an action that animates or changes scene, \
use wait_for before the next screenshot. eval_lua can read or change any variable.";

// ---------------------------------------------------------------- parameters

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RunParams {
    #[schemars(description = "Project folder (the one containing main.lua), or the path to main.lua")]
    project: String,
    #[schemars(description = "Path to \"Corona Simulator.exe\" if it isn't in the default install location")]
    simulator: Option<String>,
    #[schemars(description = "Add the bridge (needed for screenshot/tap/drag/key/display_tree/eval_lua). Default true")]
    inject_bridge: Option<bool>,
    #[schemars(description = "Log Lua runtime errors instead of showing the simulator's blocking error dialog. Default true")]
    suppress_error_dialogs: Option<bool>,
    #[schemars(description = "How long to wait for the app to start and the bridge to connect. Default 20")]
    wait_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RelaunchParams {
    #[schemars(description = "How long to wait for the app to start and the bridge to connect. Default 20")]
    wait_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct LogParams {
    #[schemars(description = "Only lines from this cursor on (use next_cursor from the previous call to get just the new lines)")]
    since: Option<u64>,
    #[schemars(description = "At most this many lines, newest kept. Default 200")]
    max_lines: Option<usize>,
    #[schemars(description = "Only lines containing this text (case-insensitive)")]
    contains: Option<String>,
    #[schemars(description = "Only the current run (default true); false includes earlier runs")]
    current_run_only: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ShotParams {
    #[schemars(description = "Longest side of the returned image in pixels. Default 1024")]
    max_size: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Unit {
    /// Pixels in the last screenshot this server returned (default)
    Screenshot,
    /// Solar2D content coordinates (same as display_tree bounds)
    Content,
    /// 0-100 across the whole screen
    Percent,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TapParams {
    x: f64,
    y: f64,
    #[schemars(description = "How long the finger stays down, in ms. Default 100; use e.g. 800 for a long press")]
    hold_ms: Option<u64>,
    #[schemars(description = "screenshot (default) | content | percent")]
    unit: Option<Unit>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct DragParams {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    #[schemars(description = "Length of the drag in milliseconds. Default 300")]
    duration_ms: Option<u64>,
    #[schemars(description = "screenshot (default) | content | percent")]
    unit: Option<Unit>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct KeyParams {
    #[schemars(description = "Solar2D keyName, e.g. \"space\", \"left\", \"a\", \"enter\", \"back\", \"escape\"")]
    key: String,
    #[schemars(description = "press (down then up, default) | down | up")]
    phase: Option<String>,
    shift: Option<bool>,
    ctrl: Option<bool>,
    alt: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TreeParams {
    #[schemars(description = "Default 8")]
    max_depth: Option<u32>,
    #[schemars(description = "Default 400")]
    max_nodes: Option<u32>,
    #[schemars(description = "Include invisible objects. Default false")]
    include_hidden: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EvalParams {
    #[schemars(description = "Lua run inside the app. An expression's value is returned (e.g. \"composer.getSceneName('current')\"), or run statements and `return` values")]
    code: String,
    #[schemars(description = "How deep to print returned tables. Default 3")]
    depth: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TypeParams {
    #[schemars(description = "The text to type")]
    text: String,
    #[schemars(description = "Name/id of the text field (see display_tree). Optional: defaults to the focused field, or the only one on screen")]
    target: Option<String>,
    #[schemars(description = "Or pick the field at a point")]
    x: Option<f64>,
    y: Option<f64>,
    #[schemars(description = "Unit for x/y: screenshot (default) | content | percent")]
    unit: Option<Unit>,
    #[schemars(description = "Replace what's in the field (default true) or append")]
    replace: Option<bool>,
    #[schemars(description = "Press return afterwards: a \"submitted\" userInput event (default true)")]
    submit: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct AlertParams {
    #[schemars(description = "Button label or 1-based index. Leave out to just read the alert")]
    button: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WaitParams {
    #[schemars(description = "Lua expression evaluated every frame until it is truthy, e.g. \"composer.getSceneName('current') == 'game'\"")]
    lua: Option<String>,
    #[schemars(description = "Wait for a log line containing this text (case-insensitive)")]
    log_contains: Option<String>,
    #[schemars(description = "For log_contains: also match lines from this read_logs cursor on (default: only new lines)")]
    since: Option<u64>,
    #[schemars(description = "Give up after this long. Default 5000. With neither lua nor log_contains, just waits this long")]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RemoveParams {
    #[schemars(description = "Project folder or main.lua path. Default: the current project")]
    project: Option<String>,
}

// ---------------------------------------------------------------- helpers

fn ok_text(s: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(s)]))
}
fn err_text(s: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![ContentBlock::text(s)]))
}
fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

#[derive(Clone)]
struct Solar2dMcp {
    state: Shared,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl Solar2dMcp {
    async fn status_json(&self) -> Value {
        let sim = self.state.sim.lock().await;
        let hello = self.state.bridge.lock().unwrap().as_ref().map(|c| c.hello.clone());
        match sim.as_ref() {
            None => json!({"running": false, "note": "nothing launched yet; use run_project"}),
            Some(s) => json!({
                "running": s.running(),
                "exit": *s.exited.borrow(),
                "project": s.project_dir.display().to_string(),
                "simulator": s.simulator.display().to_string(),
                "uptime_seconds": s.started.elapsed().as_secs(),
                "bridge_connected": hello.is_some(),
                "bridge_injected": s.inject_bridge,
                "logs_source": if self.state.logs_via_bridge.load(Ordering::SeqCst) { "bridge" } else { "stdout" },
                "screen": hello.as_ref().and_then(|h| h.get("info").cloned()),
                "device": hello.as_ref().and_then(|h| h.get("model").cloned()),
                "solar2d_build": hello.as_ref().and_then(|h| h.get("build").cloned()),
            }),
        }
    }

    /// Wait for a new bridge hello (or the process exiting), then report.
    async fn wait_started(&self, wait_seconds: u64, inject: bool) -> Result<CallToolResult, McpError> {
        let mut hello_rx = self.state.hello_tx.subscribe();
        hello_rx.mark_unchanged();
        let exited = self.state.sim.lock().await.as_ref().map(|s| s.exited.clone());
        let deadline = Duration::from_secs(wait_seconds.max(1));
        let mut outcome = "started";
        if inject {
            let wait = async {
                let mut ex = exited.clone();
                tokio::select! {
                    _ = hello_rx.changed() => "bridge connected",
                    _ = async { match ex.as_mut() { Some(e) => { let _ = e.wait_for(|x| x.is_some()).await; } None => std::future::pending().await } } => "simulator exited",
                }
            };
            outcome = tokio::time::timeout(deadline, wait)
                .await
                .unwrap_or("bridge did not connect in time (see logs: a Lua error in main.lua, or a dialog in the simulator)");
            if outcome == "bridge connected" {
                // let the probe settle and the first frame's prints arrive
                tokio::time::sleep(Duration::from_millis(1500)).await;
            }
        } else {
            tokio::time::sleep(Duration::from_secs(wait_seconds.clamp(1, 5))).await;
        }
        let log_start = self.state.sim.lock().await.as_ref().map(|s| s.log_start).unwrap_or(0);
        let lines = self.state.logs.lock().unwrap().from(log_start);
        let tail: Vec<String> = lines.iter().rev().take(80).rev().map(|(_, l)| l.clone()).collect();
        let next = self.state.logs.lock().unwrap().next_cursor();
        let status = self.status_json().await;
        ok_text(format!(
            "{outcome}\n\nstatus:\n{}\n\nlogs so far (next_cursor {next}):\n{}",
            pretty(&status),
            tail.join("\n")
        ))
    }

    fn to_bridge_coords(&self, x: f64, y: f64, unit: Option<Unit>) -> Result<(f64, f64, &'static str), String> {
        match unit.unwrap_or(Unit::Screenshot) {
            Unit::Content => Ok((x, y, "content")),
            Unit::Percent => Ok((x / 100.0, y / 100.0, "fraction")),
            Unit::Screenshot => {
                let (w, h) = self.state.last_shot.lock().unwrap().ok_or(
                    "No screenshot taken in this run yet: call screenshot first, or use unit \"content\" or \"percent\"",
                )?;
                Ok((x / w as f64, y / h as f64, "fraction"))
            }
        }
    }
}

// ---------------------------------------------------------------- tools

#[tool_router]
impl Solar2dMcp {
    fn new(state: Shared) -> Self {
        Self { state, tool_router: Self::tool_router() }
    }

    #[tool(description = "Start a Solar2D project in the Solar2D Simulator (stopping any project this server started before). Waits for the app to come up and returns status plus the first log lines.")]
    async fn run_project(&self, Parameters(p): Parameters<RunParams>) -> Result<CallToolResult, McpError> {
        let inject = p.inject_bridge.unwrap_or(true);
        let opts = sim::RunOptions {
            project: p.project,
            simulator: p.simulator,
            inject_bridge: inject,
            suppress_error_dialogs: p.suppress_error_dialogs.unwrap_or(true),
        };
        if let Err(e) = sim::launch(&self.state, opts).await {
            return err_text(e);
        }
        self.wait_started(p.wait_seconds.unwrap_or(20), inject).await
    }

    #[tool(description = "Restart the current project in the simulator (e.g. after editing its code).")]
    async fn relaunch(&self, Parameters(p): Parameters<RelaunchParams>) -> Result<CallToolResult, McpError> {
        let opts = {
            let sim = self.state.sim.lock().await;
            let Some(s) = sim.as_ref() else {
                return err_text("Nothing to relaunch yet: use run_project first.");
            };
            sim::RunOptions {
                project: s.main_lua.display().to_string(),
                simulator: Some(s.simulator.display().to_string()),
                inject_bridge: s.inject_bridge,
                suppress_error_dialogs: s.suppress_error_dialogs,
            }
        };
        let inject = opts.inject_bridge;
        if let Err(e) = sim::launch(&self.state, opts).await {
            return err_text(e);
        }
        self.wait_started(p.wait_seconds.unwrap_or(20), inject).await
    }

    #[tool(description = "Close the simulator this server started.")]
    async fn stop(&self) -> Result<CallToolResult, McpError> {
        match sim::stop(&self.state).await {
            None => ok_text("Nothing was running."),
            Some(how) => ok_text(format!("Simulator stopped ({how}).")),
        }
    }

    #[tool(description = "Whether the simulator is running, which project, whether the bridge is connected, and the screen/content size.")]
    async fn status(&self) -> Result<CallToolResult, McpError> {
        ok_text(pretty(&self.status_json().await))
    }

    #[tool(description = "Read the app's console output (print(), warnings, runtime errors with stack traces). Pass since=next_cursor to get only new lines.")]
    async fn read_logs(&self, Parameters(p): Parameters<LogParams>) -> Result<CallToolResult, McpError> {
        let run_start = if p.current_run_only.unwrap_or(true) {
            self.state.sim.lock().await.as_ref().map(|s| s.log_start).unwrap_or(0)
        } else {
            0
        };
        let from = p.since.unwrap_or(0).max(run_start);
        let (mut lines, next) = {
            let logs = self.state.logs.lock().unwrap();
            (logs.from(from), logs.next_cursor())
        };
        if let Some(c) = p.contains.as_ref().map(|c| c.to_lowercase()) {
            lines.retain(|(_, l)| l.to_lowercase().contains(&c));
        }
        let max = p.max_lines.unwrap_or(200).max(1);
        let skipped = lines.len().saturating_sub(max);
        let body: Vec<String> = lines.into_iter().skip(skipped).map(|(_, l)| l).collect();
        let mut out = format!("next_cursor: {next}\n");
        if skipped > 0 {
            out.push_str(&format!("({skipped} older lines not shown)\n"));
        }
        if body.is_empty() {
            out.push_str("(no new lines)");
        } else {
            out.push_str(&body.join("\n"));
        }
        ok_text(out)
    }

    #[tool(description = "Capture what the simulator screen shows right now. Returns a PNG; tap/drag coordinates default to pixels in this image.")]
    async fn screenshot(&self, Parameters(p): Parameters<ShotParams>) -> Result<CallToolResult, McpError> {
        let r = match self.state.request(json!({"cmd": "screenshot"}), Duration::from_secs(10)).await {
            Ok(r) => r,
            Err(e) => return err_text(e),
        };
        let Some(path) = r.get("path").and_then(Value::as_str) else {
            return err_text("bridge returned no screenshot path");
        };
        let bytes = match tokio::fs::read(path).await {
            Ok(b) => b,
            Err(e) => return err_text(format!("could not read screenshot {path}: {e}")),
        };
        let max = p.max_size.unwrap_or(1024).clamp(64, 4096);
        let encoded = tokio::task::spawn_blocking(move || -> Result<(String, u32, u32, u32, u32), String> {
            let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
            let (ow, oh) = (img.width(), img.height());
            let img = if ow.max(oh) > max {
                img.resize(max, max, image::imageops::FilterType::Triangle)
            } else {
                img
            };
            let mut png = Vec::new();
            img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .map_err(|e| e.to_string())?;
            Ok((base64::engine::general_purpose::STANDARD.encode(png), img.width(), img.height(), ow, oh))
        })
        .await;
        let (b64, w, h, ow, oh) = match encoded {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return err_text(format!("could not process screenshot: {e}")),
            Err(e) => return err_text(format!("could not process screenshot: {e}")),
        };
        *self.state.last_shot.lock().unwrap() = Some((w, h));
        let info = r.get("info").cloned().unwrap_or(Value::Null);
        let note = format!(
            "Screenshot {w}x{h} px (captured at {ow}x{oh}). Screen in content units: origin ({}, {}), size {} x {}; content area {} x {}. \
             tap/drag use pixels in this image by default.",
            info["screenOriginX"], info["screenOriginY"], info["actualContentWidth"], info["actualContentHeight"],
            info["contentWidth"], info["contentHeight"]
        );
        Ok(CallToolResult::success(vec![ContentBlock::image(b64, "image/png"), ContentBlock::text(note)]))
    }

    #[tool(description = "Tap the app at a point: sends touch began/ended and a tap event to the topmost objects there (and Runtime if none handles it). Returns which object handled each.")]
    async fn tap(&self, Parameters(p): Parameters<TapParams>) -> Result<CallToolResult, McpError> {
        let (x, y, unit) = match self.to_bridge_coords(p.x, p.y, p.unit) {
            Ok(v) => v,
            Err(e) => return err_text(e),
        };
        let hold = p.hold_ms.unwrap_or(100).min(60_000);
        let cmd = json!({"cmd": "tap", "x": x, "y": y, "unit": unit, "hold_ms": hold});
        match self.state.request(cmd, Duration::from_millis(hold + 10_000)).await {
            Ok(r) => ok_text(pretty(&r)),
            Err(e) => err_text(e),
        }
    }

    #[tool(description = "Drag/swipe from (x1,y1) to (x2,y2): touch began, moved every frame, ended.")]
    async fn drag(&self, Parameters(p): Parameters<DragParams>) -> Result<CallToolResult, McpError> {
        let (x1, y1, unit) = match self.to_bridge_coords(p.x1, p.y1, p.unit.as_ref().map(clone_unit)) {
            Ok(v) => v,
            Err(e) => return err_text(e),
        };
        let (x2, y2, _) = match self.to_bridge_coords(p.x2, p.y2, p.unit) {
            Ok(v) => v,
            Err(e) => return err_text(e),
        };
        let dur = p.duration_ms.unwrap_or(300).min(60_000);
        let cmd = json!({"cmd": "drag", "x1": x1, "y1": y1, "x2": x2, "y2": y2, "unit": unit, "duration_ms": dur});
        match self.state.request(cmd, Duration::from_millis(dur + 10_000)).await {
            Ok(r) => ok_text(pretty(&r)),
            Err(e) => err_text(e),
        }
    }

    #[tool(description = "Send a key event to the app (Runtime \"key\" listeners).")]
    async fn key(&self, Parameters(p): Parameters<KeyParams>) -> Result<CallToolResult, McpError> {
        let cmd = json!({"cmd": "key", "key": p.key, "phase": p.phase.unwrap_or_else(|| "press".into()),
                         "shift": p.shift.unwrap_or(false), "ctrl": p.ctrl.unwrap_or(false), "alt": p.alt.unwrap_or(false)});
        match self.state.request(cmd, Duration::from_secs(10)).await {
            Ok(r) => ok_text(pretty(&r)),
            Err(e) => err_text(e),
        }
    }

    #[tool(description = "The app's display hierarchy: each object's name/id, kind, text, position, content bounds, visibility and event listeners; plus the current composer scene.")]
    async fn display_tree(&self, Parameters(p): Parameters<TreeParams>) -> Result<CallToolResult, McpError> {
        let cmd = json!({"cmd": "tree", "max_depth": p.max_depth.unwrap_or(8), "max_nodes": p.max_nodes.unwrap_or(400),
                         "include_hidden": p.include_hidden.unwrap_or(false)});
        match self.state.request(cmd, Duration::from_secs(10)).await {
            Ok(r) => ok_text(serde_json::to_string(&r).unwrap_or_default()),
            Err(e) => err_text(e),
        }
    }

    #[tool(description = "Run Lua inside the running app and return the result (read game state, call functions, jump to a scene).")]
    async fn eval_lua(&self, Parameters(p): Parameters<EvalParams>) -> Result<CallToolResult, McpError> {
        let cmd = json!({"cmd": "eval", "code": p.code, "depth": p.depth.unwrap_or(3)});
        match self.state.request(cmd, Duration::from_secs(15)).await {
            Ok(r) => {
                let vals: Vec<String> = r
                    .get("values")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect())
                    .unwrap_or_default();
                ok_text(if vals.is_empty() { "(no value returned)".to_string() } else { vals.join("\n") })
            }
            Err(e) => err_text(format!("Lua error: {e}")),
        }
    }

    #[tool(description = "Type into a native text field/text box: sends the userInput events a keyboard would (began, editing per character, submitted) and sets its text. With no text field in the app, sends key presses instead.")]
    async fn type_text(&self, Parameters(p): Parameters<TypeParams>) -> Result<CallToolResult, McpError> {
        let mut cmd = json!({"cmd": "type", "text": p.text, "replace": p.replace.unwrap_or(true), "submit": p.submit.unwrap_or(true)});
        if let Some(t) = p.target {
            cmd["target"] = json!(t);
        } else if let (Some(x), Some(y)) = (p.x, p.y) {
            match self.to_bridge_coords(x, y, p.unit) {
                Ok((x, y, unit)) => {
                    cmd["x"] = json!(x);
                    cmd["y"] = json!(y);
                    cmd["unit"] = json!(unit);
                }
                Err(e) => return err_text(e),
            }
        }
        match self.state.request(cmd, Duration::from_secs(15)).await {
            Ok(r) => ok_text(pretty(&r)),
            Err(e) => err_text(e),
        }
    }

    #[tool(description = "Read or answer the alert the app is showing (native.showAlert is shown as an in-app overlay while the bridge is in; its buttons can also be tapped).")]
    async fn answer_alert(&self, Parameters(p): Parameters<AlertParams>) -> Result<CallToolResult, McpError> {
        let mut cmd = json!({"cmd": "alert"});
        if let Some(b) = p.button {
            cmd["button"] = json!(b);
        }
        match self.state.request(cmd, Duration::from_secs(10)).await {
            Ok(r) => ok_text(pretty(&r)),
            Err(e) => err_text(e),
        }
    }

    #[tool(description = "Wait until a Lua condition is true in the app and/or a log line appears (e.g. after a tap that starts a scene transition). Returns whether it happened and how long it took.")]
    async fn wait_for(&self, Parameters(p): Parameters<WaitParams>) -> Result<CallToolResult, McpError> {
        let timeout = p.timeout_ms.unwrap_or(5000).min(120_000);
        let start = std::time::Instant::now();
        let deadline = Duration::from_millis(timeout);
        let log_from = p.since.unwrap_or_else(|| self.state.logs.lock().unwrap().next_cursor());
        let needle = p.log_contains.as_ref().map(|s| s.to_lowercase());

        let log_wait = async {
            let Some(n) = needle.as_ref() else { return std::future::pending::<String>().await };
            loop {
                let found = self.state.logs.lock().unwrap().from(log_from).into_iter().find(|(_, l)| l.to_lowercase().contains(n));
                if let Some((_, l)) = found {
                    return format!("log line appeared after {} ms: {l}", start.elapsed().as_millis());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        let lua_wait = async {
            let Some(code) = p.lua.as_ref() else { return std::future::pending::<Result<String, String>>().await };
            let cmd = json!({"cmd": "wait", "lua": code, "timeout_ms": timeout});
            match self.state.request(cmd, deadline + Duration::from_secs(5)).await {
                Ok(r) if r.get("met").and_then(Value::as_bool) == Some(true) => Ok(format!(
                    "condition true after {} ms (value {})",
                    r["after_ms"], r.get("value").and_then(Value::as_str).unwrap_or("?")
                )),
                Ok(r) => Err(match r.get("last_error").and_then(Value::as_str) {
                    Some(e) => format!("condition still false after {timeout} ms (last evaluation error: {e})"),
                    None => format!("condition still false after {timeout} ms"),
                }),
                Err(e) => Err(e),
            }
        };
        if p.lua.is_none() && needle.is_none() {
            tokio::time::sleep(deadline).await;
            return ok_text(format!("waited {timeout} ms"));
        }
        tokio::select! {
            m = log_wait => ok_text(m),
            r = lua_wait => match r { Ok(m) => ok_text(m), Err(e) => err_text(e) },
            _ = tokio::time::sleep(deadline + Duration::from_millis(500)), if p.lua.is_none() =>
                err_text(format!("no matching log line within {timeout} ms")),
        }
    }

    #[tool(description = "Take the bridge out of a project: deletes solar2d_mcp_bridge.lua and the line added to main.lua.")]
    async fn remove_bridge(&self, Parameters(p): Parameters<RemoveParams>) -> Result<CallToolResult, McpError> {
        let project = match p.project {
            Some(p) => p,
            None => match self.state.sim.lock().await.as_ref() {
                Some(s) => s.main_lua.display().to_string(),
                None => return err_text("Give the project path."),
            },
        };
        let (dir, main) = match sim::resolve_project(&project) {
            Ok(v) => v,
            Err(e) => return err_text(e),
        };
        match sim::remove_bridge(&dir, &main) {
            Ok(done) if done.is_empty() => ok_text("No bridge found in that project."),
            Ok(done) => ok_text(done.join("\n")),
            Err(e) => err_text(e),
        }
    }
}

fn clone_unit(u: &Unit) -> Unit {
    match u {
        Unit::Screenshot => Unit::Screenshot,
        Unit::Content => Unit::Content,
        Unit::Percent => Unit::Percent,
    }
}

#[tool_handler]
impl ServerHandler for Solar2dMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(INSTRUCTIONS)
            .with_server_info(Implementation::new("solar2d-mcp", env!("CARGO_PKG_VERSION")))
    }
}

// ---------------------------------------------------------------- main

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut simulator: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--simulator" => simulator = args.next().map(PathBuf::from),
            "--version" | "-V" => {
                eprintln!("solar2d-mcp {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                eprintln!(
                    "solar2d-mcp {}: MCP server (stdio) for the Solar2D Simulator\n\n\
                     Options:\n  --simulator <path>   path to \"Corona Simulator.exe\" (or set SOLAR2D_SIMULATOR)\n",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            other => eprintln!("solar2d-mcp: ignoring unknown argument {other}"),
        }
    }

    let state = State::new(simulator);
    bridge::start(state.clone()).await?;
    {
        let st = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            loop {
                tick.tick().await;
                st.flush_repeats();
            }
        });
    }
    let server = Solar2dMcp::new(state.clone());
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    sim::stop(&state).await;
    Ok(())
}
