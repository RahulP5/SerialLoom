//! `serialloom-cli`: control a running SerialLoom from scripts and tests.
//!
//! It talks to the app's automation API (see `docs/AUTOMATION.md`), which
//! only programs of the same Windows user can reach, and only after the user
//! turned it on in SerialLoom's Settings.

use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str = "\
Control a running SerialLoom from scripts and tests.

Usage: serialloom-cli <command> [arguments] [options]

Commands:
  status                        Check that SerialLoom can be reached
  ports                         List all COM ports
  list                          List connections
  pair [COMa] [COMb]            Create a virtual null-modem pair and print its ports
                                (default: the next free numbers)
      --name <text>             Name for the connection
      --autostart               Start the pair whenever SerialLoom starts
  start <connection>            Start a connection
  stop <connection>             Stop a connection
  delete <connection>           Delete a connection and the virtual ports it created
      --keep-ports              Keep its virtual ports
  create-port <COMn|COM#>...    Create virtual ports that are not connected yet
  remove-port <COMn>            Remove a virtual port
  renumber <COMold> <COMnew>    Give a virtual port another number
  send <json>                   Send one raw API request (see docs/AUTOMATION.md)

<connection> is a connection's id, its name, or one of its COM ports.

Options:
  --json                        Print JSON instead of text
  --timeout <seconds>           How long to wait for an answer (default 300)
  --no-start                    Do not start SerialLoom if it is not running
  -h, --help                    Show this help
  -V, --version                 Show the version

Exit codes: 0 success, 1 the command failed, 2 bad usage, 3 SerialLoom not reachable.

SerialLoom's Settings > Automation > \"Allow other programs to control SerialLoom\"
must be on. If SerialLoom is not running, every command except 'status' starts it
in the background (notification area only) and waits for it.";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Status,
    Ports,
    List,
    Pair {
        a: String,
        b: String,
        name: Option<String>,
        autostart: bool,
    },
    Start(String),
    Stop(String),
    Delete {
        target: String,
        keep_ports: bool,
    },
    CreatePorts(Vec<String>),
    RemovePort(String),
    Renumber {
        from: String,
        to: String,
    },
    Send(String),
    Help,
    Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cli {
    command: Command,
    json: bool,
    timeout: Duration,
    /// Never start SerialLoom; fail if it is not running.
    no_start: bool,
}

fn parse_args(args: &[String]) -> Result<Cli, String> {
    let mut json = false;
    let mut timeout = Duration::from_secs(300);
    let mut name = None;
    let mut autostart = false;
    let mut keep_ports = false;
    let mut no_start = false;
    let mut words: Vec<String> = Vec::new();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--autostart" => autostart = true,
            "--keep-ports" => keep_ports = true,
            "--no-start" => no_start = true,
            "--name" => name = Some(it.next().ok_or("--name needs a value")?.clone()),
            "--timeout" => {
                let v = it.next().ok_or("--timeout needs a number of seconds")?;
                let secs: u64 = v
                    .parse()
                    .ok()
                    .filter(|s| (1..=86_400).contains(s))
                    .ok_or_else(|| format!("'{v}' is not a number of seconds"))?;
                timeout = Duration::from_secs(secs);
            }
            "-h" | "--help" => words.insert(0, "help".into()),
            "-V" | "--version" => words.insert(0, "version".into()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => words.push(arg.clone()),
        }
    }

    let (cmd, rest) = words.split_first().ok_or("missing command")?;
    let one = |what: &str| -> Result<String, String> {
        match rest {
            [x] => Ok(x.clone()),
            _ => Err(format!("{cmd} needs exactly one {what}")),
        }
    };
    let none = || -> Result<(), String> {
        if rest.is_empty() {
            Ok(())
        } else {
            Err(format!("{cmd} takes no arguments"))
        }
    };
    let command = match cmd.as_str() {
        "help" => Command::Help,
        "version" => Command::Version,
        "status" => none().map(|()| Command::Status)?,
        "ports" => none().map(|()| Command::Ports)?,
        "list" => none().map(|()| Command::List)?,
        "pair" => {
            let (a, b) = match rest {
                [] => ("COM#".to_string(), "COM#".to_string()),
                [a] => (a.clone(), "COM#".to_string()),
                [a, b] => (a.clone(), b.clone()),
                _ => return Err("pair takes at most two port names".into()),
            };
            Command::Pair {
                a: a.to_ascii_uppercase(),
                b: b.to_ascii_uppercase(),
                name,
                autostart,
            }
        }
        "start" => Command::Start(one("connection")?),
        "stop" => Command::Stop(one("connection")?),
        "delete" => Command::Delete {
            target: one("connection")?,
            keep_ports,
        },
        "create-port" if !rest.is_empty() => {
            Command::CreatePorts(rest.iter().map(|p| p.to_ascii_uppercase()).collect())
        }
        "create-port" => return Err("create-port needs at least one port name".into()),
        "remove-port" => Command::RemovePort(one("port name")?.to_ascii_uppercase()),
        "renumber" => match rest {
            [from, to] => Command::Renumber {
                from: from.to_ascii_uppercase(),
                to: to.to_ascii_uppercase(),
            },
            _ => return Err("renumber needs the current and the new port name".into()),
        },
        "send" => Command::Send(one("JSON request")?),
        other => return Err(format!("unknown command '{other}'")),
    };
    Ok(Cli {
        command,
        json,
        timeout,
        no_start,
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = match parse_args(&args) {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("serialloom-cli: {e}\n\nRun 'serialloom-cli --help' for usage.");
            return ExitCode::from(2);
        }
    };
    match cli.command {
        Command::Help => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("serialloom-cli {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => run(&cli),
    }
}

#[cfg(not(windows))]
fn run(_cli: &Cli) -> ExitCode {
    eprintln!("serialloom-cli: SerialLoom runs on Windows only");
    ExitCode::from(3)
}

#[cfg(windows)]
fn run(cli: &Cli) -> ExitCode {
    match app::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(app::Failure::Unreachable(msg)) => {
            eprintln!("serialloom-cli: {msg}");
            ExitCode::from(3)
        }
        Err(app::Failure::Command(msg)) => {
            eprintln!("serialloom-cli: {msg}");
            ExitCode::from(1)
        }
    }
}

#[cfg(windows)]
mod app {
    use serde_json::{Value, json};
    use std::time::Instant;

    use serialloom_core::automation::{Client, ConnectError, Request};
    use serialloom_core::model::{HubView, PortInfo};

    use super::{Cli, Command, Duration};

    pub enum Failure {
        Unreachable(String),
        Command(String),
    }

    struct Session {
        client: Client,
        timeout: Duration,
    }

    impl Session {
        fn call(&mut self, req: &Request) -> Result<Value, Failure> {
            let resp = self
                .client
                .call(req, self.timeout)
                .map_err(|e| Failure::Unreachable(format!("lost the connection to SerialLoom: {e}")))?;
            if resp.ok {
                Ok(resp.result)
            } else {
                Err(Failure::Command(
                    resp.error.unwrap_or_else(|| "the command failed".into()),
                ))
            }
        }

        fn hubs(&mut self) -> Result<Vec<HubView>, Failure> {
            parse(self.call(&Request::ListHubs)?)
        }

        /// A connection by id, name or one of its ports.
        fn resolve(&mut self, target: &str) -> Result<HubView, Failure> {
            let hubs = self.hubs()?;
            if let Some(h) = hubs.iter().find(|h| h.spec.id == target) {
                return Ok(h.clone());
            }
            let matches: Vec<&HubView> = hubs
                .iter()
                .filter(|h| {
                    h.spec.name.eq_ignore_ascii_case(target)
                        || h.spec
                            .endpoints()
                            .any(|e| e.spec.port_name().is_some_and(|p| p.eq_ignore_ascii_case(target)))
                })
                .collect();
            match matches.as_slice() {
                [h] => Ok((*h).clone()),
                [] => Err(Failure::Command(format!("no connection matches '{target}'"))),
                _ => Err(Failure::Command(format!(
                    "'{target}' matches {} connections; use an id from 'serialloom-cli list'",
                    matches.len()
                ))),
            }
        }
    }

    fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, Failure> {
        serde_json::from_value(v).map_err(|e| {
            Failure::Command(format!(
                "unexpected answer from SerialLoom (is it a newer version?): {e}"
            ))
        })
    }

    fn print_json(v: &Value) {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    }

    fn ports_of(h: &HubView) -> Vec<String> {
        h.spec.endpoints().map(|e| e.spec.label()).collect()
    }

    fn print_hub_line(h: &HubView) {
        println!(
            "{:<12}  {:<7}  {:<6}  {}  ({})",
            h.spec.id,
            if h.status.running { "running" } else { "stopped" },
            format!("{:?}", h.spec.kind).to_lowercase(),
            ports_of(h).join(" ⇄ "),
            h.spec.name
        );
    }

    /// How long to wait for SerialLoom to start and open its API.
    const START_WAIT: Duration = Duration::from_secs(45);

    /// Connects, starting SerialLoom in the background first if it is not
    /// running (unless `--no-start`, or for `status`, which only checks).
    fn connect(cli: &Cli) -> Result<Client, Failure> {
        match Client::connect(Duration::from_secs(10)) {
            Ok(c) => return Ok(c),
            Err(ConnectError::NotRunning) if !cli.no_start && cli.command != Command::Status => {}
            Err(e) => return Err(Failure::Unreachable(e.to_string())),
        }
        start_app()?;
        let deadline = Instant::now() + START_WAIT;
        loop {
            match Client::connect(Duration::from_secs(2)) {
                Ok(c) => return Ok(c),
                Err(ConnectError::NotRunning) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(ConnectError::NotRunning) => {
                    return Err(Failure::Unreachable(
                        "SerialLoom is running, but other programs may not control it. Turn on \
                         Settings > Automation > \"Allow other programs to control SerialLoom\"."
                            .into(),
                    ));
                }
                Err(e) => return Err(Failure::Unreachable(e.to_string())),
            }
        }
    }

    /// Starts `serialloom.exe --background` from this program's folder. If
    /// SerialLoom is already running (with automation off), the running copy
    /// ignores this and no window opens.
    fn start_app() -> Result<(), Failure> {
        use std::os::windows::process::CommandExt;
        use std::process::{Command as Process, Stdio};

        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

        if serialloom_core::driver::native::elevate::is_elevated() {
            // It would inherit administrator rights, and so would its window.
            return Err(Failure::Unreachable(
                "SerialLoom is not running. serialloom-cli does not start it from an administrator \
                 terminal; start SerialLoom normally, or run this command without administrator rights."
                    .into(),
            ));
        }
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("serialloom.exe")))
            .filter(|p| p.is_file())
            .ok_or_else(|| {
                Failure::Unreachable(
                    "SerialLoom is not running, and serialloom.exe is not next to serialloom-cli.exe to start it."
                        .into(),
                )
            })?;
        eprintln!("Starting SerialLoom in the background...");
        // SerialLoom keeps running after we exit. It must not inherit our
        // stdout/stderr: a script reading our output (`$x = serialloom-cli
        // pair`) would otherwise wait for SerialLoom to exit, i.e. forever.
        stop_inheriting_std_handles();
        let spawn = |flags: u32| {
            Process::new(&exe)
                .arg("--background")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(flags)
                .spawn()
        };
        let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        // Leave a CI job object if allowed, so SerialLoom (and its ports)
        // outlive the build step that started it.
        spawn(base | CREATE_BREAKAWAY_FROM_JOB)
            .or_else(|_| spawn(base))
            .map(drop)
            .map_err(|e| Failure::Unreachable(format!("could not start SerialLoom: {e}")))
    }

    fn stop_inheriting_std_handles() {
        use windows_sys::Win32::Foundation::{
            HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
        };
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };
        for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: plain queries and flag changes on our own std handles.
            unsafe {
                let h = GetStdHandle(id);
                if !h.is_null() && h != INVALID_HANDLE_VALUE {
                    SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    }

    pub fn run(cli: &Cli) -> Result<(), Failure> {
        let client = connect(cli)?;
        let mut s = Session {
            client,
            timeout: cli.timeout,
        };
        match &cli.command {
            Command::Status => {
                let v = s.call(&Request::Ping)?;
                if cli.json {
                    print_json(&v);
                } else {
                    println!(
                        "SerialLoom {} is running (automation API {}).",
                        v["version"].as_str().unwrap_or("?"),
                        v["api"]
                    );
                }
            }
            Command::Ports => {
                let v = s.call(&Request::ListPorts)?;
                if cli.json {
                    print_json(&v);
                } else {
                    let ports: Vec<PortInfo> = parse(v)?;
                    for p in ports {
                        println!(
                            "{:<7}  {:<9}  {}{}",
                            p.name,
                            format!("{:?}", p.kind).to_lowercase(),
                            p.description.unwrap_or_default(),
                            p.in_use_by
                                .map(|h| format!("  [in use by {h}]"))
                                .unwrap_or_default()
                        );
                    }
                }
            }
            Command::List => {
                let v = s.call(&Request::ListHubs)?;
                if cli.json {
                    print_json(&v);
                } else {
                    let hubs: Vec<HubView> = parse(v)?;
                    if hubs.is_empty() {
                        println!("No connections.");
                    }
                    hubs.iter().for_each(print_hub_line);
                }
            }
            Command::Pair {
                a,
                b,
                name,
                autostart,
            } => {
                let v = s.call(&Request::CreatePair {
                    a: a.clone(),
                    b: b.clone(),
                    name: name.clone(),
                    autostart: *autostart,
                })?;
                if cli.json {
                    print_json(&v);
                } else {
                    let hub: HubView = parse(v)?;
                    // Plain "COM9 COM10" so scripts can split it.
                    println!("{}", ports_of(&hub).join(" "));
                    eprintln!(
                        "Created {} (connection {}).",
                        ports_of(&hub).join(" ⇄ "),
                        hub.spec.id
                    );
                }
            }
            Command::Start(target) | Command::Stop(target) => {
                let id = s.resolve(target)?.spec.id;
                let req = if matches!(cli.command, Command::Start(_)) {
                    Request::StartHub { id }
                } else {
                    Request::StopHub { id }
                };
                let v = s.call(&req)?;
                if cli.json {
                    print_json(&v);
                } else {
                    print_hub_line(&parse(v)?);
                }
            }
            Command::Delete { target, keep_ports } => {
                let hub = s.resolve(target)?;
                s.call(&Request::DeleteHub {
                    id: hub.spec.id.clone(),
                    remove_virtual_ports: !keep_ports,
                })?;
                if cli.json {
                    print_json(&json!({ "deleted": hub.spec.id }));
                } else {
                    println!("Deleted {} ({}).", hub.spec.name, hub.spec.id);
                }
            }
            Command::CreatePorts(names) => {
                let v = s.call(&Request::CreateVirtualPorts { names: names.clone() })?;
                if cli.json {
                    print_json(&v);
                } else {
                    let names: Vec<String> = v
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| p["name"].as_str().map(String::from))
                        .collect();
                    println!("{}", names.join(" "));
                }
            }
            Command::RemovePort(name) => {
                s.call(&Request::RemoveVirtualPort { name: name.clone() })?;
                if cli.json {
                    print_json(&json!({ "removed": name }));
                } else {
                    println!("Removed {name}.");
                }
            }
            Command::Renumber { from, to } => {
                let v = s.call(&Request::RenameVirtualPort {
                    name: from.clone(),
                    new_name: to.clone(),
                })?;
                if cli.json {
                    print_json(&v);
                } else {
                    println!("{from} is now {}.", v["name"].as_str().unwrap_or(to));
                }
            }
            Command::Send(raw) => {
                serde_json::from_str::<Value>(raw)
                    .map_err(|e| Failure::Command(format!("not valid JSON: {e}")))?;
                let resp = s
                    .client
                    .call_raw(raw.as_bytes(), s.timeout)
                    .map_err(|e| Failure::Unreachable(format!("lost the connection to SerialLoom: {e}")))?;
                print_json(&serde_json::to_value(&resp).unwrap_or_default());
                if !resp.ok {
                    return Err(Failure::Command(resp.error.unwrap_or_default()));
                }
            }
            Command::Help | Command::Version => unreachable!("handled in main"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Cli, String> {
        parse_args(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses_pair() {
        let cli = p(&[
            "pair",
            "com9",
            "COM10",
            "--name",
            "test rig",
            "--autostart",
            "--json",
        ])
        .unwrap();
        assert_eq!(
            cli.command,
            Command::Pair {
                a: "COM9".into(),
                b: "COM10".into(),
                name: Some("test rig".into()),
                autostart: true
            }
        );
        assert!(cli.json);
        assert_eq!(
            p(&["pair"]).unwrap().command,
            Command::Pair {
                a: "COM#".into(),
                b: "COM#".into(),
                name: None,
                autostart: false
            }
        );
        assert!(p(&["pair", "COM1", "COM2", "COM3"]).is_err());
    }

    #[test]
    fn parses_other_commands() {
        assert_eq!(p(&["status"]).unwrap().command, Command::Status);
        assert_eq!(
            p(&["delete", "COM9", "--keep-ports"]).unwrap().command,
            Command::Delete {
                target: "COM9".into(),
                keep_ports: true
            }
        );
        assert_eq!(
            p(&["renumber", "com5", "com20"]).unwrap().command,
            Command::Renumber {
                from: "COM5".into(),
                to: "COM20".into()
            }
        );
        assert_eq!(
            p(&["--timeout", "5", "list"]).unwrap().timeout,
            Duration::from_secs(5)
        );
        assert_eq!(p(&["stop", "--help"]).unwrap().command, Command::Help);
        assert!(p(&["list", "--no-start"]).unwrap().no_start);
        assert!(!p(&["list"]).unwrap().no_start);
    }

    #[test]
    fn rejects_bad_usage() {
        assert!(p(&[]).is_err());
        assert!(p(&["frobnicate"]).is_err());
        assert!(p(&["status", "extra"]).is_err());
        assert!(p(&["start"]).is_err());
        assert!(p(&["list", "--nope"]).is_err());
        assert!(p(&["list", "--timeout", "0"]).is_err());
        assert!(p(&["list", "--timeout"]).is_err());
        assert!(p(&["create-port"]).is_err());
        assert!(p(&["renumber", "COM5"]).is_err());
    }
}
