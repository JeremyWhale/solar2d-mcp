//! Shared state: the simulator process, captured logs and the bridge connection.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};

const MAX_LOG_LINES: usize = 20_000;

pub struct LogBuf {
    lines: VecDeque<(u64, String)>,
    next: u64,
}

impl LogBuf {
    fn new() -> Self {
        Self { lines: VecDeque::new(), next: 1 }
    }
    pub fn push(&mut self, line: String) {
        self.lines.push_back((self.next, line));
        self.next += 1;
        while self.lines.len() > MAX_LOG_LINES {
            self.lines.pop_front();
        }
    }
    /// Cursor that the next pushed line will get.
    pub fn next_cursor(&self) -> u64 {
        self.next
    }
    /// Lines with cursor >= `from`.
    pub fn from(&self, from: u64) -> Vec<(u64, String)> {
        self.lines.iter().filter(|(n, _)| *n >= from).cloned().collect()
    }
}

pub struct BridgeConn {
    pub generation: u64,
    pub tx: mpsc::UnboundedSender<String>,
    pub hello: Value,
}

pub struct Sim {
    pub project_dir: PathBuf,
    pub main_lua: PathBuf,
    pub simulator: PathBuf,
    pub inject_bridge: bool,
    pub suppress_error_dialogs: bool,
    pub started: Instant,
    pub log_start: u64,
    pub kill_tx: Option<oneshot::Sender<()>>,
    /// None while running, Some(exit description) once the process ended.
    pub exited: watch::Receiver<Option<String>>,
}

impl Sim {
    pub fn running(&self) -> bool {
        self.exited.borrow().is_none()
    }
}

pub type Pending = HashMap<u64, oneshot::Sender<Result<Value, String>>>;

pub struct State {
    pub port: Mutex<u16>,
    pub logs: Mutex<LogBuf>,
    pub sim: tokio::sync::Mutex<Option<Sim>>,
    pub bridge: Mutex<Option<BridgeConn>>,
    pub pending: Mutex<Pending>,
    pub next_id: AtomicU64,
    pub generation: AtomicU64,
    /// Bumped every time a bridge says hello.
    pub hello_tx: watch::Sender<u64>,
    /// Nonce for the current run; the bridge prints it to stdout so we can tell
    /// whether stdout reaches us live.
    pub nonce: Mutex<String>,
    pub probe_seen: AtomicBool,
    /// true = logs come from the bridge (stdout was not live), false = from stdout.
    pub logs_via_bridge: AtomicBool,
    /// Size of the last screenshot we returned, for unit = "screenshot".
    pub last_shot: Mutex<Option<(u32, u32)>>,
    pub cli_simulator: Option<PathBuf>,
    pub repeats: Mutex<RepeatFilter>,
    /// A newer release, once the daily update check has found one.
    pub update_available: Mutex<Option<String>>,
}

/// Hides a line once it has appeared more than REPEAT_LIMIT times within REPEAT_WINDOW
/// (an error or print firing every frame would otherwise bury everything else).
#[derive(Default)]
pub struct RepeatFilter {
    seen: HashMap<String, (u32, Instant)>,
    hidden: u64,
}
const REPEAT_LIMIT: u32 = 5;
const REPEAT_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

pub type Shared = Arc<State>;

impl State {
    pub fn new(cli_simulator: Option<PathBuf>) -> Shared {
        let (hello_tx, _) = watch::channel(0);
        Arc::new(State {
            port: Mutex::new(0),
            logs: Mutex::new(LogBuf::new()),
            sim: tokio::sync::Mutex::new(None),
            bridge: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            generation: AtomicU64::new(0),
            hello_tx,
            nonce: Mutex::new(String::new()),
            probe_seen: AtomicBool::new(false),
            logs_via_bridge: AtomicBool::new(false),
            last_shot: Mutex::new(None),
            cli_simulator,
            repeats: Mutex::new(RepeatFilter::default()),
            update_available: Mutex::new(None),
        })
    }

    pub fn log(&self, line: impl Into<String>) {
        self.logs.lock().unwrap().push(line.into());
    }

    pub fn port(&self) -> u16 {
        *self.port.lock().unwrap()
    }

    pub fn bridge_connected(&self) -> bool {
        self.bridge.lock().unwrap().is_some()
    }

    /// A line from the simulator's stdout/stderr.
    pub fn on_output_line(&self, line: String) {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() || is_noise(trimmed) {
            return;
        }
        {
            let nonce = self.nonce.lock().unwrap();
            if !nonce.is_empty() && trimmed.contains("[solar2d-mcp] bridge connected") {
                if trimmed.contains(nonce.as_str()) {
                    self.probe_seen.store(true, Ordering::SeqCst);
                }
                return;
            }
        }
        if self.logs_via_bridge.load(Ordering::SeqCst) {
            return; // the bridge forwards print() instead; avoid late duplicates
        }
        self.log_app_line(trimmed);
    }

    /// A line of the app's own output (stdout, or forwarded by the bridge).
    pub fn log_app_line(&self, line: &str) {
        {
            let mut f = self.repeats.lock().unwrap();
            let now = Instant::now();
            if f.seen.len() > 4000 {
                f.seen.retain(|_, (_, t)| now.duration_since(*t) < REPEAT_WINDOW);
            }
            let e = f.seen.entry(line.to_string()).or_insert((0, now));
            if now.duration_since(e.1) > REPEAT_WINDOW {
                *e = (0, now);
            }
            e.0 += 1;
            if e.0 > REPEAT_LIMIT {
                f.hidden += 1;
                return;
            }
        }
        self.log(line.to_string());
    }

    /// Called every couple of seconds: note how many repeated lines were hidden.
    pub fn flush_repeats(&self) {
        let hidden = std::mem::take(&mut self.repeats.lock().unwrap().hidden);
        if hidden > 0 {
            self.log(format!(
                "[solar2d-mcp] {hidden} more copies of the lines above hidden (the same output many times a second: \
                 probably an error or print() in an enterFrame/timer)"
            ));
        }
    }

    /// Send a command to the bridge and wait for its reply.
    pub async fn request(
        &self,
        mut cmd: Value,
        timeout: std::time::Duration,
    ) -> Result<Value, String> {
        let tx = match self.bridge.lock().unwrap().as_ref() {
            Some(c) => c.tx.clone(),
            None => return Err(not_connected_msg()),
        };
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        cmd["id"] = Value::from(id);
        let (rtx, rrx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, rtx);
        if tx.send(format!("{cmd}\n")).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(not_connected_msg());
        }
        match tokio::time::timeout(timeout, rrx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("the bridge disconnected before replying".into()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!(
                    "the bridge did not reply within {:.0} s (is the app stuck or paused?)",
                    timeout.as_secs_f32()
                ))
            }
        }
    }
}

pub fn not_connected_msg() -> String {
    "The bridge inside the simulator is not connected. Start the project with run_project \
     (inject_bridge true), and check read_logs for errors in main.lua that stop it loading."
        .into()
}

/// Lines that only Wine/Mesa print, never the app.
fn is_noise(line: &str) -> bool {
    line.starts_with("libEGL warning")
        || line.starts_with("pci id for fd")
        || line.starts_with("MESA: ")
        || line.contains("VkInstanceCreateInfo::pApplicationInfo::apiVersion")
}
