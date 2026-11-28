//! Automation API: lets other programs control the running SerialLoom app,
//! for example a test script that needs a fresh virtual null-modem pair.
//!
//! Transport: a local named pipe, `\\.\pipe\SerialLoom.Automation.<SID>`,
//! carrying one JSON request per line and one JSON response per line. Any
//! language can use it; `serialloom-cli` wraps it for shell scripts.
//!
//! Security:
//! * Off unless the user turns it on in Settings.
//! * Only programs of the same Windows user can connect: the pipe's access
//!   list names that user (and SYSTEM) only, and remote clients are refused.
//!   Programs that the user runs can already do anything the user can, so
//!   the API adds no new rights; administrator steps still go through
//!   SerialLoom Helper or a UAC prompt exactly as they do from the window.
//! * The pipe is created first-instance-only, and clients check that the
//!   server runs as the same user before sending anything.
//! * Requests are a fixed set of typed commands (unknown commands or fields
//!   are rejected) with size, time and connection limits. Settings, driver
//!   installation and the helper service cannot be changed through it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Engine;
use crate::error::{Error, Result};
use crate::model::{CreateHubRequest, EndpointRequest, HubKind};
use crate::winpipe::{PipeListener, PipeStream, current_user_sid, process_user_sid};

/// Version of the request/response format, reported by `ping`.
pub const API_VERSION: u32 = 1;

const MAX_REQUEST: usize = 64 * 1024;
const MAX_CONNECTIONS: usize = 8;
/// A connection with no request for this long is closed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Pipe name for the user with SID `sid`.
pub fn pipe_name_for(sid: &str) -> String {
    format!(r"\\.\pipe\SerialLoom.Automation.{sid}")
}

/// Pipe name for the user running this process.
pub fn pipe_name() -> std::io::Result<String> {
    Ok(pipe_name_for(&current_user_sid()?))
}

fn yes() -> bool {
    true
}

fn any_port() -> String {
    "COM#".into()
}

/// Every command the API accepts. JSON: `{"command": "createPair", "a": "COM9", "b": "COM10"}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "command",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Request {
    /// Checks the connection; returns the app and API versions.
    Ping,
    /// All COM ports: real, SerialLoom virtual, and other virtual ones.
    ListPorts,
    /// SerialLoom's virtual ports with their driver state.
    ListVirtualPorts,
    /// All connections (hubs) with their live status.
    ListHubs,
    GetHub {
        id: String,
    },
    /// A virtual null-modem pair: two new virtual ports wired together.
    /// `COM#` (the default) takes the next free number.
    CreatePair {
        #[serde(default = "any_port")]
        a: String,
        #[serde(default = "any_port")]
        b: String,
        #[serde(default)]
        name: Option<String>,
        /// Start the pair again whenever SerialLoom starts.
        #[serde(default)]
        autostart: bool,
    },
    /// Any connection: split, join, bridge or bus (same shape as the app uses).
    CreateHub {
        request: CreateHubRequest,
    },
    StartHub {
        id: String,
    },
    StopHub {
        id: String,
    },
    UpdateHub {
        id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        autostart: Option<bool>,
    },
    /// Deletes a connection; by default also removes the virtual ports it created.
    DeleteHub {
        id: String,
        #[serde(default = "yes")]
        remove_virtual_ports: bool,
    },
    AddMember {
        id: String,
        request: EndpointRequest,
    },
    RemoveMember {
        id: String,
        index: usize,
        #[serde(default = "yes")]
        remove_virtual_port: bool,
    },
    /// Changes a GPS simulator's settings; a running one takes them on at
    /// once. `index` 0 is the master, then the members.
    UpdateSimulator {
        id: String,
        index: usize,
        gps: Box<crate::model::GpsSettings>,
        #[serde(default)]
        cycle_ms: Option<u32>,
    },
    CreateVirtualPorts {
        names: Vec<String>,
    },
    RenameVirtualPort {
        name: String,
        new_name: String,
    },
    RemoveVirtualPort {
        name: String,
    },
}

impl Request {
    /// True for commands that change ports or connections.
    pub fn changes_topology(&self) -> bool {
        !matches!(
            self,
            Request::Ping
                | Request::ListPorts
                | Request::ListVirtualPorts
                | Request::ListHubs
                | Request::GetHub { .. }
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    /// The command's result (absent for commands that return nothing).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub result: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    fn from_result(r: Result<Value>) -> Self {
        match r {
            Ok(v) => Response {
                ok: true,
                result: v,
                error: None,
            },
            Err(e) => Response {
                ok: false,
                result: Value::Null,
                error: Some(e.to_string()),
            },
        }
    }
}

fn json<T: Serialize>(v: T) -> Result<Value> {
    Ok(serde_json::to_value(v)?)
}

/// Runs one request against the engine.
pub fn execute(engine: &Engine, req: Request) -> Result<Value> {
    match req {
        Request::Ping => json(serde_json::json!({
            "app": "SerialLoom",
            "version": env!("CARGO_PKG_VERSION"),
            "api": API_VERSION,
        })),
        Request::ListPorts => json(engine.ports()?),
        Request::ListVirtualPorts => json(engine.virtual_ports()?),
        Request::ListHubs => json(engine.hubs()),
        Request::GetHub { id } => json(
            engine
                .hubs()
                .into_iter()
                .find(|h| h.spec.id == id)
                .ok_or_else(|| Error::NotFound(format!("connection {id}")))?,
        ),
        Request::CreatePair {
            a,
            b,
            name,
            autostart,
        } => {
            let custom_name = name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(String::from);
            let hub = engine.create_hub(CreateHubRequest {
                name: custom_name.clone().unwrap_or_else(|| "Virtual null-modem".into()),
                kind: HubKind::Bridge,
                master: EndpointRequest::NewVirtual { name: a },
                members: vec![EndpointRequest::NewVirtual { name: b }],
                autostart,
                start: true,
            })?;
            if custom_name.is_some() {
                return json(hub);
            }
            // Name it after its ports, as the app does.
            let ports: Vec<String> = hub
                .spec
                .endpoints()
                .filter_map(|e| e.spec.port_name().map(String::from))
                .collect();
            json(engine.update_hub(&hub.spec.id, Some(ports.join(" ⇄ ")), None)?)
        }
        Request::CreateHub { request } => json(engine.create_hub(request)?),
        Request::StartHub { id } => json(engine.start_hub(&id)?),
        Request::StopHub { id } => json(engine.stop_hub(&id)?),
        Request::UpdateHub { id, name, autostart } => json(engine.update_hub(&id, name, autostart)?),
        Request::DeleteHub {
            id,
            remove_virtual_ports,
        } => json(engine.delete_hub(&id, remove_virtual_ports)?),
        Request::AddMember { id, request } => json(engine.add_member(&id, request)?),
        Request::RemoveMember {
            id,
            index,
            remove_virtual_port,
        } => json(engine.remove_member(&id, index, remove_virtual_port)?),
        Request::UpdateSimulator {
            id,
            index,
            gps,
            cycle_ms,
        } => json(engine.update_simulator(&id, index, *gps, cycle_ms)?),
        Request::CreateVirtualPorts { names } => json(engine.create_virtual_ports(names)?),
        Request::RenameVirtualPort { name, new_name } => json(engine.rename_virtual_port(&name, &new_name)?),
        Request::RemoveVirtualPort { name } => json(engine.remove_virtual_port(&name)?),
    }
}

/// Parses and runs one request line.
pub fn handle_line(engine: &Engine, line: &[u8]) -> (Response, bool) {
    match serde_json::from_slice::<Request>(line) {
        Ok(req) => {
            let changes = req.changes_topology();
            let resp = Response::from_result(execute(engine, req));
            let changed = changes && resp.ok;
            (resp, changed)
        }
        Err(e) => (
            Response::from_result(Err(Error::Invalid(format!("invalid request: {e}")))),
            false,
        ),
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Called after a command changed ports or connections, so the window can refresh.
pub type ChangeHook = Arc<dyn Fn() + Send + Sync>;

/// Serves the automation pipe on a background thread until dropped.
pub struct AutomationServer {
    pipe: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AutomationServer {
    /// Starts serving on this user's pipe.
    pub fn start(engine: Arc<Engine>, on_change: ChangeHook) -> Result<Self> {
        let sid = current_user_sid()?;
        Self::start_on(&pipe_name_for(&sid), &sid, engine, on_change)
    }

    /// Starts serving on `pipe`, accessible to the user `sid` only.
    pub fn start_on(pipe: &str, sid: &str, engine: Arc<Engine>, on_change: ChangeHook) -> Result<Self> {
        // Protected DACL: this user and SYSTEM only; network logons denied.
        let sddl = format!("D:P(D;;GA;;;NU)(A;;GA;;;{sid})(A;;GA;;;SY)");
        let mut listener = PipeListener::bind(pipe, &sddl, MAX_CONNECTIONS as u32 + 1).map_err(|e| {
            Error::Driver(format!(
                "could not start the automation API on {pipe}: {e}. Is another copy of SerialLoom running?"
            ))
        })?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("automation".into())
                .spawn(move || accept_loop(&mut listener, &stop, engine, on_change))?
        };
        tracing::info!("automation API listening on {pipe}");
        Ok(Self {
            pipe: pipe.to_string(),
            stop,
            thread: Some(thread),
        })
    }

    pub fn pipe_name(&self) -> &str {
        &self.pipe
    }
}

impl Drop for AutomationServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        tracing::info!("automation API stopped");
    }
}

fn accept_loop(
    listener: &mut PipeListener,
    stop: &Arc<AtomicBool>,
    engine: Arc<Engine>,
    on_change: ChangeHook,
) {
    let active = Arc::new(AtomicUsize::new(0));
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        workers.retain(|w| !w.is_finished());
        let conn = match listener.accept(Some(Duration::from_millis(250))) {
            Ok(Some(c)) => c,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!("automation: accept failed: {e}");
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
        };
        if active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            let mut conn = conn;
            let busy = Response::from_result(Err(Error::Invalid("too many connections; try again".into())));
            let _ = conn.write_line(
                &serde_json::to_vec(&busy).unwrap_or_default(),
                Some(WRITE_TIMEOUT),
            );
            let _ = conn.read_line(16, Some(Duration::from_secs(2)));
            continue;
        }
        active.fetch_add(1, Ordering::SeqCst);
        let (engine, on_change, stop, active) = (
            Arc::clone(&engine),
            Arc::clone(&on_change),
            Arc::clone(stop),
            Arc::clone(&active),
        );
        let spawned = std::thread::Builder::new()
            .name("automation-conn".into())
            .spawn(move || {
                serve_connection(conn, &engine, &on_change, &stop);
                active.fetch_sub(1, Ordering::SeqCst);
            });
        match spawned {
            Ok(w) => workers.push(w),
            Err(e) => tracing::warn!("automation: could not start a connection thread: {e}"),
        }
    }
    for w in workers {
        let _ = w.join();
    }
}

fn serve_connection(mut conn: PipeStream, engine: &Engine, on_change: &ChangeHook, stop: &AtomicBool) {
    let mut idle = Duration::ZERO;
    let tick = Duration::from_millis(500);
    while !stop.load(Ordering::SeqCst) {
        let line = match conn.read_line(MAX_REQUEST, Some(tick)) {
            Ok(Some(line)) => line,
            Ok(None) => return,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                idle += tick;
                if idle >= IDLE_TIMEOUT {
                    return;
                }
                continue;
            }
            Err(e) => {
                let resp = Response::from_result(Err(Error::Invalid(format!("invalid request: {e}"))));
                let _ = conn.write_line(
                    &serde_json::to_vec(&resp).unwrap_or_default(),
                    Some(WRITE_TIMEOUT),
                );
                return;
            }
        };
        idle = Duration::ZERO;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let (resp, changed) = handle_line(engine, &line);
        if changed {
            on_change();
        }
        let bytes = serde_json::to_vec(&resp).unwrap_or_default();
        if conn.write_line(&bytes, Some(WRITE_TIMEOUT)).is_err() {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Why a client could not reach SerialLoom.
#[derive(Debug)]
pub enum ConnectError {
    /// SerialLoom is not running, or automation is off in its Settings.
    NotRunning,
    /// The pipe exists but is not served by this user's SerialLoom.
    Untrusted(String),
    Other(std::io::Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::NotRunning => write!(
                f,
                "SerialLoom is not reachable. Start SerialLoom and turn on \
                 Settings > Automation > \"Allow other programs to control SerialLoom\"."
            ),
            ConnectError::Untrusted(why) => write!(f, "refusing to talk to the automation pipe: {why}"),
            ConnectError::Other(e) => write!(f, "could not connect to SerialLoom: {e}"),
        }
    }
}

impl std::error::Error for ConnectError {}

/// A connection to the running app.
pub struct Client {
    conn: PipeStream,
}

impl Client {
    /// Connects to this user's SerialLoom.
    pub fn connect(timeout: Duration) -> std::result::Result<Self, ConnectError> {
        let sid = current_user_sid().map_err(ConnectError::Other)?;
        Self::connect_to(&pipe_name_for(&sid), &sid, timeout)
    }

    /// Connects to `pipe`, which must be served by a process of user `sid`.
    pub fn connect_to(pipe: &str, sid: &str, timeout: Duration) -> std::result::Result<Self, ConnectError> {
        let conn = PipeStream::connect(pipe, timeout).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ConnectError::NotRunning,
            _ => ConnectError::Other(e),
        })?;
        let server = conn.server_process_id().map_err(ConnectError::Other)?;
        match process_user_sid(server) {
            Ok(owner) if owner.eq_ignore_ascii_case(sid) => Ok(Self { conn }),
            Ok(owner) => Err(ConnectError::Untrusted(format!(
                "it is served by process {server} of another user ({owner})"
            ))),
            Err(e) => Err(ConnectError::Untrusted(format!(
                "could not check who runs process {server} ({e}). If SerialLoom runs as administrator, \
                 run this program as administrator too"
            ))),
        }
    }

    /// Sends one request and waits up to `timeout` for its response.
    pub fn call(&mut self, req: &Request, timeout: Duration) -> std::io::Result<Response> {
        let bytes = serde_json::to_vec(req).map_err(std::io::Error::other)?;
        self.call_raw(&bytes, timeout)
    }

    /// Sends one request given as JSON text.
    pub fn call_raw(&mut self, json: &[u8], timeout: Duration) -> std::io::Result<Response> {
        self.conn.write_line(json, Some(WRITE_TIMEOUT))?;
        let line = self
            .conn
            .read_line(16 * 1024 * 1024, Some(timeout))?
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "SerialLoom closed the connection",
                )
            })?;
        serde_json::from_slice(&line).map_err(std::io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::driver::demo::DemoDriver;
    use crate::model::HubView;

    fn parse(s: &str) -> std::result::Result<Request, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }

    #[test]
    fn parses_commands_strictly() {
        assert!(matches!(parse(r#"{"command":"ping"}"#), Ok(Request::Ping)));
        match parse(r#"{"command":"createPair","a":"COM9"}"#).unwrap() {
            Request::CreatePair { a, b, autostart, .. } => {
                assert_eq!((a.as_str(), b.as_str(), autostart), ("COM9", "COM#", false));
            }
            other => panic!("{other:?}"),
        }
        match parse(r#"{"command":"deleteHub","id":"x"}"#).unwrap() {
            Request::DeleteHub {
                remove_virtual_ports, ..
            } => assert!(remove_virtual_ports),
            other => panic!("{other:?}"),
        }
        assert!(parse(r#"{"command":"setSettings","driver":"demo"}"#).is_err());
        assert!(parse(r#"{"command":"installDriver"}"#).is_err());
        assert!(parse(r#"{"command":"stopHub","id":"x","sneaky":true}"#).is_err());
        assert!(parse(r#"{"command":"stopHub"}"#).is_err());
        assert!(parse(r#"{"id":"x"}"#).is_err());
    }

    fn start_server(engine: Arc<Engine>, changes: Arc<AtomicUsize>) -> AutomationServer {
        let sid = current_user_sid().unwrap();
        let pipe = format!(
            r"\\.\pipe\SerialLoom.Test.Automation.{}.{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        );
        AutomationServer::start_on(
            &pipe,
            &sid,
            engine,
            Arc::new(move || {
                changes.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .unwrap()
    }

    fn client(server: &AutomationServer) -> Client {
        Client::connect_to(
            server.pipe_name(),
            &current_user_sid().unwrap(),
            Duration::from_secs(5),
        )
        .unwrap()
    }

    fn ok(c: &mut Client, req: Request) -> Value {
        let r = c.call(&req, Duration::from_secs(10)).unwrap();
        assert!(r.ok, "{req:?} failed: {:?}", r.error);
        r.result
    }

    #[test]
    fn creates_uses_and_deletes_a_pair_end_to_end() {
        let engine = Arc::new(Engine::new(Arc::new(DemoDriver::new()), None));
        let changes = Arc::new(AtomicUsize::new(0));
        let server = start_server(Arc::clone(&engine), Arc::clone(&changes));
        let mut c = client(&server);

        let pong = ok(&mut c, Request::Ping);
        assert_eq!(pong["api"], API_VERSION);

        let hub: HubView = serde_json::from_value(ok(
            &mut c,
            Request::CreatePair {
                a: "COM9".into(),
                b: "COM10".into(),
                name: None,
                autostart: false,
            },
        ))
        .unwrap();
        assert_eq!(hub.spec.name, "COM9 ⇄ COM10");
        assert!(hub.status.running);
        assert_eq!(changes.load(Ordering::SeqCst), 1);

        // The window's engine sees the same pair.
        assert_eq!(engine.hubs().len(), 1);
        let ports = ok(&mut c, Request::ListVirtualPorts);
        assert_eq!(ports.as_array().unwrap().len(), 2);

        // Several requests on one connection, and a second connection.
        let mut c2 = client(&server);
        ok(
            &mut c2,
            Request::StopHub {
                id: hub.spec.id.clone(),
            },
        );
        ok(
            &mut c,
            Request::StartHub {
                id: hub.spec.id.clone(),
            },
        );

        // Errors come back as responses, and change nothing.
        let r = c
            .call(&Request::StopHub { id: "nope".into() }, Duration::from_secs(5))
            .unwrap();
        assert!(!r.ok && r.error.unwrap().contains("not found"));
        let r = c
            .call_raw(br#"{"command":"installDriver"}"#, Duration::from_secs(5))
            .unwrap();
        assert!(!r.ok && r.error.unwrap().contains("invalid request"));
        let before = changes.load(Ordering::SeqCst);

        ok(
            &mut c,
            Request::DeleteHub {
                id: hub.spec.id.clone(),
                remove_virtual_ports: true,
            },
        );
        assert!(engine.hubs().is_empty());
        assert!(engine.virtual_ports().unwrap().is_empty());
        assert_eq!(changes.load(Ordering::SeqCst), before + 1);
        drop(c);
        drop(c2);
        drop(server);
    }

    #[test]
    fn rejects_oversized_requests_and_stops_cleanly() {
        let engine = Arc::new(Engine::new(Arc::new(DemoDriver::new()), None));
        let server = start_server(engine, Arc::new(AtomicUsize::new(0)));
        let mut c = client(&server);
        let huge = vec![b' '; MAX_REQUEST + 10];
        let r = c.call_raw(&huge, Duration::from_secs(5)).unwrap();
        assert!(!r.ok && r.error.unwrap().contains("too long"));
        // An idle client must not keep the server from stopping.
        let _idle = client(&server);
        let t = std::time::Instant::now();
        drop(server);
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_pipe_never_disappears_between_clients() {
        // Clients that connect and leave at once (like `Get-Acl` on the pipe)
        // must never find the name missing: that would make clients fail at
        // random and leave a moment in which another process could take it.
        let engine = Arc::new(Engine::new(Arc::new(DemoDriver::new()), None));
        let server = start_server(engine, Arc::new(AtomicUsize::new(0)));
        let mut missing = 0;
        for _ in 0..300 {
            match PipeStream::connect(server.pipe_name(), Duration::from_secs(5)) {
                Ok(c) => drop(c),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing += 1,
                Err(e) => panic!("{e}"),
            }
        }
        assert_eq!(missing, 0, "the pipe name lapsed {missing} times");
        // And it still serves requests afterwards, once the connection
        // threads of the flood have finished (at most 8 run at a time).
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let r = client(&server)
                .call(&Request::Ping, Duration::from_secs(5))
                .unwrap();
            if r.ok {
                break;
            }
            assert!(
                r.error.as_deref().unwrap_or("").contains("too many connections"),
                "{r:?}"
            );
            assert!(std::time::Instant::now() < deadline, "still busy after 5 s");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn a_second_server_on_the_same_pipe_fails() {
        let engine = Arc::new(Engine::new(Arc::new(DemoDriver::new()), None));
        let server = start_server(Arc::clone(&engine), Arc::new(AtomicUsize::new(0)));
        let sid = current_user_sid().unwrap();
        let again = AutomationServer::start_on(server.pipe_name(), &sid, engine, Arc::new(|| {}));
        assert!(again.is_err());
    }

    #[test]
    fn client_rejects_a_server_of_another_user() {
        let engine = Arc::new(Engine::new(Arc::new(DemoDriver::new()), None));
        let server = start_server(engine, Arc::new(AtomicUsize::new(0)));
        // Pretend we are someone else: the real server's user does not match.
        let err = Client::connect_to(server.pipe_name(), "S-1-5-21-1-2-3-1001", Duration::from_secs(5))
            .err()
            .unwrap();
        assert!(matches!(err, ConnectError::Untrusted(_)), "{err}");
        let missing = Client::connect_to(
            r"\\.\pipe\SerialLoom.Test.None",
            "S-1-5-18",
            Duration::from_millis(100),
        )
        .err()
        .unwrap();
        assert!(matches!(missing, ConnectError::NotRunning));
    }
}
