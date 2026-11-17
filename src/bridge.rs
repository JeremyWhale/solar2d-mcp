//! TCP server on 127.0.0.1 that the Lua bridge inside the simulator connects to.

use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::state::{BridgeConn, Shared};

pub const BRIDGE_FILE: &str = "solar2d_mcp_bridge.lua";
const BRIDGE_TEMPLATE: &str = include_str!("../bridge/solar2d_mcp_bridge.lua");

pub fn bridge_source(port: u16, suppress_error_dialogs: bool, nonce: &str) -> String {
    BRIDGE_TEMPLATE
        .replace("__PORT__", &port.to_string())
        .replace("__SUPPRESS_ERRORS__", if suppress_error_dialogs { "true" } else { "false" })
        .replace("__NONCE__", nonce)
}

pub async fn start(state: Shared) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    *state.port.lock().unwrap() = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(handle_conn(state.clone(), stream));
                }
                Err(e) => {
                    eprintln!("solar2d-mcp: bridge accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    });
    Ok(())
}

async fn handle_conn(state: Shared, stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let (rd, mut wr) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(s) = rx.recv().await {
            if wr.write_all(s.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    let mut reader = BufReader::new(rd);
    let mut buf = Vec::new();
    let mut my_generation: Option<u64> = None;
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&buf);
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if let Some(id) = v.get("id").and_then(Value::as_u64) {
            if let Some(waiter) = state.pending.lock().unwrap().remove(&id) {
                let r = if v.get("ok").and_then(Value::as_bool) == Some(true) {
                    Ok(v.get("result").cloned().unwrap_or(Value::Null))
                } else {
                    Err(v
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown bridge error")
                        .to_string())
                };
                let _ = waiter.send(r);
            }
            continue;
        }
        match v.get("type").and_then(Value::as_str) {
            Some("hello") => {
                let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
                my_generation = Some(generation);
                *state.bridge.lock().unwrap() = Some(BridgeConn {
                    generation,
                    tx: tx.clone(),
                    hello: v.clone(),
                });
                state.log(format!(
                    "[solar2d-mcp] bridge connected (build {}, device {})",
                    v.get("build").and_then(Value::as_str).unwrap_or("?"),
                    v.get("model").and_then(Value::as_str).unwrap_or("?")
                ));
                state.hello_tx.send_replace(generation);
                tokio::spawn(check_stdout_probe(state.clone(), generation));
            }
            Some("log") => {
                if state.logs_via_bridge.load(Ordering::SeqCst) {
                    if let Some(l) = v.get("line").and_then(Value::as_str) {
                        for part in l.lines() {
                            state.log(part.to_string());
                        }
                    }
                }
            }
            Some("error") => {
                // Also printed to stdout by the bridge; only log it here when stdout isn't live.
                if state.logs_via_bridge.load(Ordering::SeqCst) {
                    let msg = v.get("message").and_then(Value::as_str).unwrap_or("?");
                    let stack = v.get("stack").and_then(Value::as_str).unwrap_or("");
                    state.log(format!("[solar2d-mcp] Runtime error: {msg}"));
                    for l in stack.lines() {
                        state.log(l.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(g) = my_generation {
        let mut b = state.bridge.lock().unwrap();
        if b.as_ref().map(|c| c.generation) == Some(g) {
            *b = None;
            drop(b);
            // Any request still waiting was sent over this connection.
            state.pending.lock().unwrap().clear();
            state.log("[solar2d-mcp] bridge disconnected");
        }
    }
}

/// After a hello, see whether the bridge's probe line showed up on stdout.
/// If it didn't, stdout isn't reaching us live, so have the bridge forward print().
async fn check_stdout_probe(state: Shared, generation: u64) {
    for _ in 0..30 {
        if state.probe_seen.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if state.generation.load(Ordering::SeqCst) != generation {
        return;
    }
    let via_bridge = !state.probe_seen.load(Ordering::SeqCst);
    if via_bridge && !state.logs_via_bridge.swap(true, Ordering::SeqCst) {
        state.log(
            "[solar2d-mcp] simulator stdout is not arriving live; print() output now comes through the bridge",
        );
    }
    let _ = state
        .request(json!({"cmd": "config", "forward_print": via_bridge}), Duration::from_secs(5))
        .await;
}
