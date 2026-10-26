# The SerialLoom virtual port driver

> **Status:** this driver needs a Microsoft signature before Windows will load it on normal PCs (see "Signing" below). Until then SerialLoom uses [virtual USB serial ports](USB_PORTS.md) by default, which need no driver of its own.

SerialLoom ships its own driver, so users do not install any third-party software. This page covers how the driver works, how to build and test it, and how to sign it for release.

## How it works

Windows only lets a driver create a COM port that other programs can open. The SerialLoom driver (`drivers/serialloom-port`) is a **UMDF 2** driver written in Rust with Microsoft's [windows-drivers-rs](https://github.com/microsoft/windows-drivers-rs). UMDF drivers run in a user-mode host process (`WUDFHost.exe`), not in the kernel. A bug in the driver therefore crashes that host process, not Windows.

Each virtual port is one device instance with hardware ID `Root\SerialLoomPort` in the Windows **Ports** class. Each device has two sides:

| Side | Opened by | Path | Behaviour |
|---|---|---|---|
| COM side | any program | `\\.\COM20` | A normal serial port: baud rate, timeouts, DTR/RTS, `WaitCommEvent` and the other serial calls all work. |
| Control side | SerialLoom only | device interface `{6F1C3E2A-…}` + `\control` | Reads what the program wrote; writes what the program will read. |

```
 Program (PuTTY, GPS app, PLC tool)                      SerialLoom app
            │  open COM20                                       │  open …\control
            ▼                                                   ▼
   ┌───────────────────────── serialloom_port.dll (UMDF) ─────────────────────────┐
   │  COM side ── writes ──► from_com buffer ── reads ──► control side             │
   │  COM side ◄── reads ─── to_com buffer  ◄── writes ── control side             │
   │  serial IOCTLs, read timeouts, wait masks, modem lines (CTS/DSR/DCD follow    │
   │  whether SerialLoom is attached)                                              │
   └───────────────────────────────────────────────────────────────────────────────┘
```

Splitting, joining and bridging all happen in the SerialLoom app (`crates/serialloom-core`). The driver only turns a COM port into a byte pipe. A "virtual null-modem pair" is two virtual ports bridged by the app.

Code layout:

| Path | Contents |
|---|---|
| `crates/serialloom-proto` | GUIDs, IOCTL codes and the `PortState` struct shared by driver and app |
| `crates/serialloom-port-logic` | Buffers, Win32 `COMMTIMEOUTS` rules, wait events, modem lines. Plain Rust with unit tests. |
| `drivers/serialloom-port/src/lib.rs` | WDF callbacks: file create/cleanup, read, write, device control |
| `drivers/serialloom-port/src/wdf.rs` | Thin wrappers over the WDF calls; all `unsafe` code lives here |
| `drivers/serialloom-port/serialloom_port.inx` | INF template (stamped into `serialloom_port.inf`) |
| `crates/serialloom-core/src/driver/native/` | App side: SetupAPI device management, COM number database, UAC helper, control channel |

### Creating and removing ports

The app creates ports with SetupAPI, the same way Device Manager does:

1. Claim a COM number in the Windows COM port database (`ComDBClaimPort`).
2. Create a root-enumerated device with hardware ID `Root\SerialLoomPort`.
3. Write `PortName` and a friendly name, then install the driver on the device.

These steps need administrator rights. The app relaunches itself through UAC with `--serialloom-admin`, so one prompt covers a batch of ports. The elevated process:

- accepts only `create`, `remove`, `install-driver` and `uninstall-all`;
- re-validates every port name (`COM1` to `COM255`, or `COM#`);
- finds the driver INF next to its own executable, never from an argument.

## Building

Requirements (Windows x64):

- Rust stable (MSVC toolchain)
- [Windows Driver Kit](https://learn.microsoft.com/windows-hardware/drivers/download-the-wdk) 10.0.26100 or newer: `winget install Microsoft.WindowsWDK.10.0.26100`
- **LLVM 17** for binding generation: `winget install LLVM.LLVM --version 17.0.6`. Newer LLVM releases currently produce broken bindings with windows-drivers-rs.

```powershell
# Unsigned package (what CI builds)
.\drivers\serialloom-port\package.ps1 -NoSign

# Test-signed package: creates a "SerialLoom Test Signing" certificate in your
# user certificate store on first run and exports SerialLoomTestCert.cer
.\drivers\serialloom-port\package.ps1
```

Output goes to `drivers\serialloom-port\target\release\package\`: `serialloom_port.dll`, `.inf`, `.cat`, and `SerialLoomTestCert.cer` when signed. The script also runs `infverif`, Microsoft's INF checker.

Logic tests run with the normal workspace tests: `cargo test -p serialloom-port-logic`.

## Signing: free, per PC

Windows installs a driver package only when its catalog (`.cat`) is signed by a publisher the PC trusts. SerialLoom does not need a paid certificate. The installer signs the driver **on each PC**, the same approach libwdi/Zadig use:

1. Copy the bundled package to a fresh folder under `%ProgramData%\SerialLoom`, where only administrators can change files.
2. Check the catalog's SHA-256 against the value recorded when the app was built (`crates/serialloom-core/build.rs`). Any other catalog is refused, so nothing else can get signed.
3. Create a new certificate (`CN=SerialLoom Local Driver Signing (<PC name>)`), trust it in *Trusted Root* and *Trusted Publishers*, and sign the catalog.
4. **Delete the private key** right away. Nobody, including the SerialLoom authors, can sign anything else this PC would trust.
5. Install the signed package. Uninstalling SerialLoom removes the ports, the driver and these certificates.

The code is in `crates/serialloom-core/src/driver/native/localsign.rs`. A catalog that already has a trusted signature, such as a Microsoft-signed one, is installed unchanged.

**Status (tested on Windows 11 25H2):** signing only the catalog is **not** enough. The driver package installs, but Code Integrity inside `WUDFHost.exe` refuses to load the DLL (Code Integrity event 3004, "file hash could not be found"; device problem code 31). The installer therefore also embeds the per-PC signature in `serialloom_port.dll`. If that is still refused, Windows requires a Microsoft signature for user-mode driver DLLs, and releases need [Microsoft signing](#microsoft-signing-optional); development machines can use test-signing mode.

## Testing on another PC or a VM

No test mode and no setup steps. Test with a normal Windows configuration, Secure Boot on:

1. Build: `.\drivers\serialloom-port\package.ps1 -NoSign`, then `npm run app:build`.
2. Copy `target\release\bundle\nsis\SerialLoom_0.1.0_x64-setup.exe` to the test PC and run it. SmartScreen may warn because the installer is not signed: choose *More info* > *Run anyway*.
3. Open SerialLoom. Settings should show the driver as **ready**.
4. **Smoke test**:
   - Toolbar > Virtual pair > create `COM20 ⇄ COM21`. Under Virtual ports, both should be **ready**.
   - Open two terminals (PuTTY, Tera Term or `python -m serial.tools.miniterm COM20`) on COM20 and COM21. Text typed in one appears in the other.
   - Device Manager > Ports (COM & LPT) lists "SerialLoom Virtual Port (COM20)".

### Troubleshooting

| Symptom | Meaning |
|---|---|
| Port state "problem", code 52 or 31, with Code Integrity event 3004 for `serialloom_port.dll` | Windows did not accept the per-PC signature for loading the driver. Microsoft signing is needed (below). |
| "could not sign the driver on this PC" | Signing failed; the message includes PowerShell's error. Please report it. |
| Code 28 | Driver package not installed: Settings > Install driver. |
| Code 10 / 31 | The driver failed to start. Capture debug output (below). |
| "already attached to another SerialLoom hub" | Only one hub can use a virtual port at a time. |

Driver log messages go to the debugger output. Watch them with Sysinternals DebugView (Capture Global Win32), or attach WinDbg to the `WUDFHost.exe` process that hosts `serialloom_port.dll`.

### Developer test-signing (optional)

`.\drivers\serialloom-port\package.ps1` without `-NoSign` signs with a test certificate on your machine. Use it with `scripts\vm-prepare-test-driver.ps1` (test-signing mode, Secure Boot off) only when debugging signing itself.

## Microsoft signing (optional)

Only needed if per-PC signing turns out not to be enough (code 52), or to avoid adding a certificate to users' PCs:

1. Buy an **EV code-signing certificate** and register a [Windows Hardware Dev Center](https://partner.microsoft.com/dashboard/hardware) account.
2. Build the unsigned package: `.\drivers\serialloom-port\package.ps1 -NoSign`
3. Sign `serialloom_port.dll` with the EV certificate, put the package files in a CAB (`makecab`), and sign the CAB.
4. Submit the CAB for **attestation signing** in Partner Center. Microsoft returns a Microsoft-signed package.
5. Zip it and set the `SIGNED_DRIVER_URL` repository secret; the release workflow then ships it. SerialLoom detects the trusted signature and skips per-PC signing.
