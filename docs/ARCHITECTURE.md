# Architecture

SerialLoom has three layers: a driver that creates COM ports, an engine that moves bytes between endpoints, and a UI that edits the topology.

```
┌──────────────────────────── SerialLoom desktop app (Tauri) ────────────────────────────┐
│  React UI (src/)                                                                       │
│   topology canvas · port palette · wizards · inspector · traffic monitor               │
│        │ invoke() commands                      ▲ "hub-status" events (every 500 ms)   │
│        ▼                                        │                                      │
│  Tauri commands (src-tauri/src/commands.rs) ── blocking work on worker threads         │
│        │                                                                               │
│  serialloom-core::Engine                                                               │
│   ├─ hubs: HubSpec + HubRuntime (a reader and a writer thread per endpoint)            │
│   ├─ topology.json persistence, autostart                                              │
│   └─ PortDriver ── UsbDriver    (USB/IP server + usbip-win2; the default)              │
│                 ├─ NativeDriver (SetupAPI, ComDB, UAC helper, control channel)         │
│                 └─ DemoDriver   (in-memory, for previews and tests)                    │
│  automation API (\\.\pipe\SerialLoom.Automation.<SID>) ◄── serialloom-cli, scripts     │
└────────────────────────────────────────────────────────────────────────────────────────┘
                 │ control side (\\?\ROOT#PORTS#…\control)
                 ▼
     serialloom_port.dll (UMDF 2 driver, one device per virtual port)
                 ▲ COM side (\\.\COM20)
                 │
        any Windows program
```

The lower half shows SerialLoom's own UMDF driver (`NativeDriver`). By default, ports come from `UsbDriver` instead: software USB serial devices that Windows' own `usbser.sys` serves through the bundled usbip-win2 controller. See [USB_PORTS.md](USB_PORTS.md).

## Hubs

Every connection is a **hub**: a list of endpoints plus a routing rule. Endpoint 0 is the master.

| Kind | Routing | Typical use |
|---|---|---|
| Split | master → all members; member → master | one GPS, three navigation apps |
| Join | same as split; the master is the combined port | several sensors, one logger |
| Bridge | exactly two endpoints, both ways | COM ↔ TCP, or a virtual null-modem pair |
| Bus | everyone → everyone else | RS-485-style multidrop |

Endpoint types: `serial` (real port), `virtual` (control side of a SerialLoom port), `tcpServer`, `tcpClient`, `udp`, and `replay` (a file played back, or the NMEA 0183 or NMEA 2000 GPS simulator).

A running hub starts two threads per endpoint. The reader owns the connection: it opens the port, reconnects after errors and reads. Each read goes into a queue for every target endpoint, and that endpoint's writer thread writes it out. There is no central router thread, and a target that reads slowly only fills its own queue (64 KB), so the other targets keep getting data.

When a target's queue is full, a source that can be held back (a virtual port, TCP or file replay) waits for space, up to 5 seconds, so the sending program slows down instead of losing data. A real serial port or UDP source never waits: data for a full queue is dropped and counted, and the slow endpoint shows an amber warning until it takes data again. Counters and a bounded traffic log (256 KB) feed the UI.

## GPS simulator and replay

A `replay` endpoint only sends. For a file, `replay.rs` plays raw bytes at a set rate, or NMEA 0183 sentences and Actisense NMEA 2000 records with their recorded timing. For the GPS simulator, `gps.rs` works out the position at each update (dead reckoning along the course, with optional noise) and builds the chosen NMEA 0183 sentences or NMEA 2000 messages. A running simulator takes new settings at once (`updateSimulator`, from the inspector or the automation API) and carries on from its current position.

## Traffic capture

Right-clicking a link opens a trace window or records the link's traffic to a file (`capture.rs`). A capture can be armed before the connection starts, so it sees the first bytes. It stops after 10 minutes, caps each file at 128 MB and keeps 20 GB free on the disk.

## Elevation

Creating or removing virtual ports and installing the driver need administrator rights. The engine never runs elevated itself. For port operations it first asks **SerialLoom Helper**, a small on-demand Windows service (`serialloom.exe --serialloom-service`) registered by the installer, over the pipe `\\.\pipe\SerialLoom.Helper`. When the helper is not installed, and always for driver installation, it relaunches `serialloom.exe --serialloom-admin …` through UAC for that one operation. Both paths parse and validate requests with the same code. See [AUTOMATION.md](AUTOMATION.md#serialloom-helper) for the helper's limits and [DRIVER.md](DRIVER.md#creating-and-removing-ports) for how the elevated step is kept safe.

## Automation

When turned on in *Settings*, the app serves a local API on `\\.\pipe\SerialLoom.Automation.<user SID>` (one JSON request per line), restricted to the same Windows user. `serialloom-cli` is a thin client for it. Commands run on the same `Engine` as the UI, and the UI refreshes on a `topology-changed` event. See [AUTOMATION.md](AUTOMATION.md).

## Frontend

- **State:** one zustand store (`src/store.ts`). Live status events update throughput rates without refetching everything.
- **Canvas:** React Flow with custom nodes (`EndpointNode`, `HubNode`) and an animated `TrafficEdge`. Node positions are saved per browser profile.
- **Browser preview:** when not running inside Tauri, `src/lib/mock.ts` stands in for the backend with simulated traffic.
