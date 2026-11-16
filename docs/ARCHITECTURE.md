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
│   ├─ hubs: HubSpec + HubRuntime (one reader thread per endpoint)                       │
│   ├─ topology.json persistence, autostart                                              │
│   └─ PortDriver ── NativeDriver (SetupAPI, ComDB, UAC helper, control channel)         │
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

## Hubs

Every connection is a **hub**: a list of endpoints plus a routing rule. Endpoint 0 is the master.

| Kind | Routing | Typical use |
|---|---|---|
| Split | master → all members; member → master | one GPS, three navigation apps |
| Join | same as split; the master is the combined port | several sensors, one logger |
| Bridge | exactly two endpoints, both ways | COM ↔ TCP, or a virtual null-modem pair |
| Bus | everyone → everyone else | RS-485-style multidrop |

Endpoint types: `serial` (real port), `virtual` (control side of a SerialLoom port), `tcpServer` and `tcpClient`.

A running hub starts one thread per endpoint. Each thread owns its connection: it opens the port, reconnects after errors, reads, and writes straight to the target endpoints. There is no central router thread, so a slow target only slows the source that feeds it. Counters and a bounded traffic log (256 KB) feed the UI.

## Elevation

Creating or removing virtual ports and installing the driver need administrator rights. The engine never runs elevated itself. For port operations it first asks **SerialLoom Helper**, a small on-demand Windows service (`serialloom.exe --serialloom-service`) registered by the installer, over the pipe `\\.\pipe\SerialLoom.Helper`. When the helper is not installed, and always for driver installation, it relaunches `serialloom.exe --serialloom-admin …` through UAC for that one operation. Both paths parse and validate requests with the same code. See [AUTOMATION.md](AUTOMATION.md#serialloom-helper) for the helper's limits and [DRIVER.md](DRIVER.md#creating-and-removing-ports) for how the elevated step is kept safe.

## Automation

When turned on in *Settings*, the app serves a local API on `\\.\pipe\SerialLoom.Automation.<user SID>` (one JSON request per line), restricted to the same Windows user. `serialloom-cli` is a thin client for it. Commands run on the same `Engine` as the UI, and the UI refreshes on a `topology-changed` event. See [AUTOMATION.md](AUTOMATION.md).

## Frontend

- **State:** one zustand store (`src/store.ts`). Live status events update throughput rates without refetching everything.
- **Canvas:** React Flow with custom nodes (`EndpointNode`, `HubNode`) and an animated `TrafficEdge`. Node positions are saved per browser profile.
- **Browser preview:** when not running inside Tauri, `src/lib/mock.ts` stands in for the backend with simulated traffic.
