//! Finding, launching and stopping the Solar2D Simulator, and wiring the bridge into a project.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::{oneshot, watch};

use crate::bridge::{BRIDGE_FILE, bridge_source};
use crate::state::{Shared, Sim};

/// Put in front of main.lua's first line (same line, so line numbers in errors stay right).
pub const MAIN_PREFIX: &str = "if system.getInfo(\"environment\") == \"simulator\" then pcall(require, \"solar2d_mcp_bridge\") end; ";

pub struct RunOptions {
    pub project: String,
    pub simulator: Option<String>,
    pub inject_bridge: bool,
    pub suppress_error_dialogs: bool,
}

/// Accepts a project folder or a path to its main.lua.
pub fn resolve_project(p: &str) -> Result<(PathBuf, PathBuf), String> {
    let path = PathBuf::from(p.trim().trim_matches('"'));
    let (dir, main) = if path
        .file_name()
        .map(|f| f.eq_ignore_ascii_case("main.lua"))
        .unwrap_or(false)
    {
        (path.parent().map(Path::to_path_buf).unwrap_or_default(), path.clone())
    } else {
        (path.clone(), path.join("main.lua"))
    };
    if !main.is_file() {
        return Err(format!("No main.lua found at {}", main.display()));
    }
    let dir = std::path::absolute(&dir).unwrap_or(dir);
    let main = std::path::absolute(&main).unwrap_or(main);
    Ok((dir, main))
}

pub fn find_simulator(state: &Shared, explicit: Option<&str>) -> Result<PathBuf, String> {
    let fix = |p: PathBuf| -> PathBuf {
        if p.is_dir() {
            if cfg!(target_os = "macos") {
                p.join("Corona Simulator.app/Contents/MacOS/Corona Simulator")
            } else {
                p.join("Corona Simulator.exe")
            }
        } else {
            p
        }
    };
    let given: Vec<(String, PathBuf)> = [
        explicit.map(|s| ("the simulator argument".to_string(), PathBuf::from(s))),
        state.cli_simulator.clone().map(|p| ("--simulator".to_string(), p)),
        std::env::var_os("SOLAR2D_SIMULATOR").map(|s| ("SOLAR2D_SIMULATOR".to_string(), PathBuf::from(s))),
    ]
    .into_iter()
    .flatten()
    .collect();
    if let Some((from, p)) = given.into_iter().next() {
        let p = fix(p);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!("Solar2D Simulator not found at {} (from {from})", p.display()))
        };
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        let mut bases: Vec<PathBuf> = ["ProgramFiles(x86)", "ProgramFiles", "ProgramW6432"]
            .iter()
            .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
            .collect();
        bases.push(PathBuf::from(r"C:\Program Files (x86)"));
        bases.push(PathBuf::from(r"C:\Program Files"));
        for b in bases {
            candidates.push(b.join(r"Corona Labs\Corona\Corona Simulator.exe"));
            candidates.push(b.join(r"Solar2D\Corona Simulator.exe"));
        }
    } else if cfg!(target_os = "macos") {
        if let Ok(rd) = std::fs::read_dir("/Applications") {
            let mut dirs: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    n.starts_with("Corona") || n.starts_with("Solar2D")
                })
                .collect();
            dirs.sort();
            for d in dirs.into_iter().rev() {
                candidates.push(d.join("Corona Simulator.app/Contents/MacOS/Corona Simulator"));
            }
        }
    }
    candidates.into_iter().find(|p| p.is_file()).ok_or_else(|| {
        "Could not find the Solar2D Simulator. Set SOLAR2D_SIMULATOR (or pass --simulator) \
         to the path of \"Corona Simulator.exe\"."
            .to_string()
    })
}

/// The simulator shows a blocking "crashed last time it was run" box at startup
/// if it was killed; clear the flag before every launch.
#[cfg(windows)]
fn clear_crash_flag() {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    if let Ok((k, _)) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(r"Software\Ansca Corona\Corona Simulator\Preferences")
    {
        let _ = k.set_value("lastRunSucceeded", &1u32);
    }
}
#[cfg(not(windows))]
fn clear_crash_flag() {}

pub fn install_bridge(state: &Shared, dir: &Path, main: &Path, suppress: bool) -> Result<(), String> {
    let nonce = state.nonce.lock().unwrap().clone();
    let src = bridge_source(state.port(), suppress, &nonce);
    std::fs::write(dir.join(BRIDGE_FILE), src)
        .map_err(|e| format!("Could not write {BRIDGE_FILE}: {e}"))?;
    let text = std::fs::read_to_string(main).map_err(|e| format!("Could not read main.lua: {e}"))?;
    if text.contains("solar2d_mcp_bridge") {
        return Ok(());
    }
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text.as_str()),
    };
    std::fs::write(main, format!("{bom}{MAIN_PREFIX}{body}"))
        .map_err(|e| format!("Could not update main.lua: {e}"))
}

/// Returns what was removed.
pub fn remove_bridge(dir: &Path, main: &Path) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    let bf = dir.join(BRIDGE_FILE);
    if bf.exists() {
        std::fs::remove_file(&bf).map_err(|e| format!("Could not delete {BRIDGE_FILE}: {e}"))?;
        done.push(format!("deleted {BRIDGE_FILE}"));
    }
    let text = std::fs::read_to_string(main).map_err(|e| format!("Could not read main.lua: {e}"))?;
    if text.contains(MAIN_PREFIX) {
        std::fs::write(main, text.replacen(MAIN_PREFIX, "", 1))
            .map_err(|e| format!("Could not update main.lua: {e}"))?;
        done.push("removed the bridge line from main.lua".into());
    }
    Ok(done)
}

fn new_nonce() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", t, std::process::id())
}

async fn pump(state: Shared, r: impl AsyncRead + Unpin) {
    let mut reader = BufReader::new(r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => state.on_output_line(String::from_utf8_lossy(&buf).into_owned()),
        }
    }
}

/// Stop the running simulator. The app is asked to exit first (so its "applicationExit"
/// handlers run), but native.requestExit() only closes the project, not the simulator,
/// so the process is then ended.
pub async fn stop(state: &Shared) -> Option<String> {
    let mut guard = state.sim.lock().await;
    let sim = guard.as_mut()?;
    if sim.running() {
        if state.bridge_connected()
            && state.request(json!({"cmd": "quit"}), Duration::from_secs(2)).await.is_ok()
        {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        if sim.running() {
            if let Some(k) = sim.kill_tx.take() {
                let _ = k.send(());
            }
            let mut rx = sim.exited.clone();
            let _ = tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|e| e.is_some())).await;
        }
    }
    let how = sim.exited.borrow().clone();
    *state.bridge.lock().unwrap() = None;
    how
}

pub async fn launch(state: &Shared, opts: RunOptions) -> Result<(), String> {
    let (dir, main) = resolve_project(&opts.project)?;
    let simulator = find_simulator(state, opts.simulator.as_deref())?;

    stop(state).await;

    *state.nonce.lock().unwrap() = new_nonce();
    state.probe_seen.store(false, Ordering::SeqCst);
    state.logs_via_bridge.store(false, Ordering::SeqCst);
    *state.last_shot.lock().unwrap() = None;

    if opts.inject_bridge {
        install_bridge(state, &dir, &main, opts.suppress_error_dialogs)?;
    }
    clear_crash_flag();

    let log_start = {
        let mut logs = state.logs.lock().unwrap();
        logs.push(format!("[solar2d-mcp] ===== launching {} =====", main.display()));
        logs.next_cursor() - 1
    };

    let mut child = Command::new(&simulator)
        .arg("-no-console")
        .arg(&main)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Could not start {}: {e}", simulator.display()))?;

    if let Some(out) = child.stdout.take() {
        tokio::spawn(pump(state.clone(), out));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(pump(state.clone(), err));
    }

    let (kill_tx, kill_rx) = oneshot::channel::<()>();
    let (exit_tx, exit_rx) = watch::channel::<Option<String>>(None);
    let st = state.clone();
    let kill_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    tokio::spawn(async move {
        let status = tokio::select! {
            s = child.wait() => s,
            _ = kill_rx => {
                kill_requested.store(true, Ordering::SeqCst);
                let _ = child.kill().await;
                child.wait().await
            }
        };
        let killed = kill_requested.load(Ordering::SeqCst);
        let desc = match status {
            _ if killed => "closed by solar2d-mcp".to_string(),
            Ok(s) => match s.code() {
                Some(c) => format!("exit code {c}"),
                None => "killed".to_string(),
            },
            Err(e) => format!("error: {e}"),
        };
        if killed {
            // so the user's next manual launch doesn't show "crashed last time it was run"
            clear_crash_flag();
        }
        st.log(format!("[solar2d-mcp] simulator exited ({desc})"));
        let _ = exit_tx.send(Some(desc));
    });

    *state.sim.lock().await = Some(Sim {
        project_dir: dir,
        main_lua: main,
        simulator,
        inject_bridge: opts.inject_bridge,
        suppress_error_dialogs: opts.suppress_error_dialogs,
        started: Instant::now(),
        log_start,
        kill_tx: Some(kill_tx),
        exited: exit_rx,
    });
    Ok(())
}
