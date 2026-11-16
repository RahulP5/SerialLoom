# Automation and administrator permission

This page covers two features that make SerialLoom usable from scripts and test benches:

- **SerialLoom Helper** removes the administrator prompt from creating, renumbering and removing virtual ports.
- The **automation API** and **`serialloom-cli`** let other programs control the running app, for example to create a virtual null-modem pair before a test and delete it afterwards ([#13](https://github.com/RahulP5/SerialLoom/issues/13)).

## SerialLoom Helper

Creating, renumbering and removing virtual ports changes Windows device settings, which needs administrator rights. SerialLoom Helper is a small Windows service that does these changes for SerialLoom, so Windows does not ask every time.

- **Installed with SerialLoom.** The installer registers it. *Settings > Administrator permission* shows whether it is installed and can install or remove it (each asks for administrator permission once).
- **Runs only when needed.** SerialLoom starts it on demand; it stops itself after 10 idle minutes.
- **Fallback.** Without the helper, SerialLoom asks through UAC as before.

### What it can and cannot do

| | |
|---|---|
| Can | Create SerialLoom ports, give SerialLoom ports another COM number, remove SerialLoom ports |
| Cannot | Install or remove drivers, install or remove itself, touch any device that is not SerialLoom's own (checked by USB ID **and** serial number), take a COM number that belongs to real hardware (even unplugged hardware) |

### How it is protected

- It accepts the same operations, parsed and validated by the same code, as the UAC helper (`serialloom.exe --serialloom-admin`), restricted to the port operations above.
- Its pipe, `\\.\pipe\SerialLoom.Helper`, refuses remote clients and network logons. Local users get read/write access only, so they cannot create a server instance of the pipe to intercept requests.
- It serves only the `serialloom.exe` it was installed from (checked by process image path). Any other copy, such as a development build, is turned down without anything running and falls back to the UAC prompt.
- SerialLoom checks that the pipe belongs to the helper's process (by process ID, from the service manager) before sending anything, and connects at identification level so the helper can never act with the user's rights.
- The service runs from Program Files, which only administrators can change. Registering it from any other folder is refused.
- Only administrators can stop, reconfigure or delete the service. Signed-in users can start it and query its status.

**Policy note:** with the helper installed, any signed-in user of the PC can create, renumber and remove SerialLoom virtual ports through SerialLoom, without an administrator prompt. Nothing else changes. If that is not acceptable on a shared PC, remove the helper in *Settings*, or have an administrator run:

```powershell
& "C:\Program Files\SerialLoom\serialloom.exe" --serialloom-admin "$env:TEMP\serialloom-reply-x.json" uninstall-service
```

## Automation API

Turn it on in *Settings > Automation > Allow other programs to control SerialLoom*. It is off by default.

- Only programs running under **your** Windows account can connect. The pipe's access list names your account (and SYSTEM) only; remote clients are refused. Such programs can already do anything your account can, so the API grants no new rights. Port changes still go through SerialLoom Helper or a UAC prompt, exactly as from the window.
- Settings, driver installation and the helper cannot be changed through the API.
- The window updates right away when a program changes ports or connections.
- SerialLoom does not need to be running: `serialloom-cli` starts it in the background (notification area only, no window) and waits for it. `status` only checks, and `--no-start` turns this off. It does not start SerialLoom from an administrator terminal, so SerialLoom never runs with administrator rights by accident.

### serialloom-cli

Installed next to SerialLoom, usually `C:\Program Files\SerialLoom\serialloom-cli.exe`.

```text
serialloom-cli status                      Check that SerialLoom can be reached
serialloom-cli ports                       List all COM ports
serialloom-cli list                        List connections
serialloom-cli pair [COMa] [COMb]          Create a virtual null-modem pair, print its ports
    --name <text>  --autostart
serialloom-cli start|stop <connection>     Start or stop a connection
serialloom-cli delete <connection>         Delete it and the virtual ports it created
    --keep-ports
serialloom-cli create-port <COMn|COM#>...  Create unconnected virtual ports
serialloom-cli remove-port <COMn>          Remove a virtual port
serialloom-cli renumber <COMold> <COMnew>  Give a virtual port another number
serialloom-cli send <json>                 Send one raw API request

Options: --json, --timeout <seconds> (default 300), --no-start, --help, --version
```

`<connection>` is a connection's id, its name, or one of its COM ports. `COM#` means "the next free number".

Exit codes: `0` success, `1` the command failed, `2` bad usage, `3` SerialLoom not reachable. The CLI prints results on stdout and messages on stderr, so scripts can read the port names alone.

### Checking that it works

From a normal (not administrator) Command Prompt:

```bat
set CLI="C:\Program Files\SerialLoom\serialloom-cli.exe"
%CLI% status
%CLI% pair COM41 COM42
%CLI% list
%CLI% delete COM41
```

1. `status` prints `SerialLoom … is running (automation API 1).`
2. `pair` prints `COM41 COM42`, without an administrator prompt, and the pair appears in the SerialLoom window.
3. `list` shows the pair as `running`; `delete` removes it from the window and from Windows.

To test the data path too, in PowerShell:

```powershell
$a = New-Object System.IO.Ports.SerialPort COM41,115200; $b = New-Object System.IO.Ports.SerialPort COM42,115200
$a.Open(); $b.Open(); $a.Write("hello`n"); Start-Sleep -Milliseconds 500; "COM42 received: " + $b.ReadExisting(); $a.Close(); $b.Close()
```

If a command fails, check its exit code (`echo %ERRORLEVEL%` in Command Prompt, `$LASTEXITCODE` in PowerShell). Code `3` usually means *Settings > Automation* is off, or the command ran in an administrator window while SerialLoom was not running.

### Examples: a test run with its own pair

PowerShell:

```powershell
$cli = "C:\Program Files\SerialLoom\serialloom-cli.exe"
$a, $b = (& $cli pair) -split ' '     # e.g. COM21 COM22
try {
    pytest tests/serial --port-a $a --port-b $b
} finally {
    & $cli delete $a
}
```

Command Prompt / batch file:

```bat
@echo off
set CLI="C:\Program Files\SerialLoom\serialloom-cli.exe"
for /f "tokens=1,2" %%a in ('%CLI% pair') do (set PORT_A=%%a& set PORT_B=%%b)
if not defined PORT_A exit /b 1
rem Run the tests on %PORT_A% and %PORT_B% here.
%CLI% delete %PORT_A%
```

Python (through the CLI):

```python
import subprocess

CLI = r"C:\Program Files\SerialLoom\serialloom-cli.exe"
a, b = subprocess.run([CLI, "pair"], capture_output=True, text=True, check=True).stdout.split()
try:
    ...  # run the tests on ports a and b
finally:
    subprocess.run([CLI, "delete", a], check=True)
```

### Test benches and CI

SerialLoom is a desktop app: it runs in a signed-in Windows session, and the automation API serves that session's user only. Set up a test machine like this:

1. Sign in as the account that runs the tests. For an unattended bench, let Windows sign in automatically ([Sysinternals Autologon](https://learn.microsoft.com/sysinternals/downloads/autologon)).
2. In SerialLoom, turn on *Settings > Automation* and *Settings > Running in the background > Start SerialLoom in the background when I sign in*. After a reboot SerialLoom is ready without a window; `serialloom-cli` would also start it on demand.
3. Run the test runner **as that same user, in that session**, not as a Windows service account. For example, install the GitHub Actions runner or Jenkins agent to run interactively (`run.cmd` / a scheduled task "at log on" for that user) rather than as a service. A runner under another account (e.g. `NETWORK SERVICE`) cannot reach the API, by design: it would let other accounts control your ports.
4. Use `pair` with `COM#` (any free numbers) or fixed numbers. Fixed numbers need SerialLoom Helper (installed by default) to avoid an administrator prompt.

`serialloom-cli` starts SerialLoom outside the build step's job object when Windows allows it, so the ports survive the step that created them.

### Protocol

Any language can use the pipe directly: `\\.\pipe\SerialLoom.Automation.<your SID>`. Your SID is shown by `whoami /user`, and the full pipe name in *Settings > Automation*.

Send one JSON object per line (UTF-8, ending in `\n`) and read one JSON line back. A connection can carry many requests. Limits: 64 KB per request, 8 connections at a time, idle connections close after 2 minutes.

Responses look like `{"ok": true, "result": …}` or `{"ok": false, "error": "…"}`. Results use the same JSON shapes as the app (`src/lib/types.ts`).

| `command` | Fields | Result |
|---|---|---|
| `ping` | | `{"app", "version", "api"}` |
| `listPorts` | | all COM ports |
| `listVirtualPorts` | | SerialLoom's virtual ports |
| `listHubs` | | connections with live status |
| `getHub` | `id` | one connection |
| `createPair` | `a`, `b` (default `COM#`), `name`?, `autostart` (default false) | the new connection, started |
| `createHub` | `request` (split, join, bridge or bus, as in the app) | the new connection |
| `startHub`, `stopHub` | `id` | the connection |
| `updateHub` | `id`, `name`?, `autostart`? | the connection |
| `deleteHub` | `id`, `removeVirtualPorts` (default true) | nothing |
| `addMember` | `id`, `request` | the connection |
| `removeMember` | `id`, `index`, `removeVirtualPort` (default true) | the connection |
| `createVirtualPorts` | `names` | the new ports |
| `renameVirtualPort` | `name`, `newName` | the port |
| `removeVirtualPort` | `name` | nothing |

Unknown commands and unknown fields are rejected. `ping` reports `api: 1`; later versions only add commands and optional fields.

Python example:

```python
import json, subprocess

sid = subprocess.run(["whoami", "/user", "/fo", "csv", "/nh"],
                     capture_output=True, text=True).stdout.strip().split(",")[-1].strip('"')
with open(rf"\\.\pipe\SerialLoom.Automation.{sid}", "r+b", buffering=0) as pipe:
    def call(request):
        pipe.write((json.dumps(request) + "\n").encode())
        line = b""
        while not line.endswith(b"\n"):
            line += pipe.read(1)
        reply = json.loads(line)
        if not reply["ok"]:
            raise RuntimeError(reply["error"])
        return reply.get("result")

    hub = call({"command": "createPair", "a": "COM9", "b": "COM10"})
    # … run tests on COM9 / COM10 …
    call({"command": "deleteHub", "id": hub["spec"]["id"]})
```

A client should check that the pipe's server process runs as the same user before trusting it, as `serialloom-cli` does (`GetNamedPipeServerProcessId`, then the process token).

### Trying it without the app

`cargo run -p serialloom-core --example automation_probe` serves the API with the in-memory demo driver (no real ports), so the CLI and clients can be tried safely. The app must not be running with automation on at the same time.
