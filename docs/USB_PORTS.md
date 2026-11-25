# Virtual COM ports via virtual USB (default)

Windows only loads drivers that Microsoft has signed. Until SerialLoom's own driver ([DRIVER.md](DRIVER.md)) gets that signature, SerialLoom creates COM ports without any driver of its own:

```
Any program ──opens COM5──► usbser.sys            (Windows' built-in USB serial driver)
                                │ thinks a USB serial adapter is plugged in
                     usbip-win2 virtual USB host  (drivers signed by Microsoft, bundled)
                                │ USB/IP protocol over 127.0.0.1
                     SerialLoom: software USB serial device ──► hubs, splits, TCP…
```

1. SerialLoom runs a USB/IP server on the loopback interface only (`crates/serialloom-core/src/usbip/`).
2. For each virtual port it registers a software **USB CDC-ACM** device: a composite device with an interface association, VID `0x1209` (pid.codes, the open-source vendor ID), and a fixed serial number per port.
3. SerialLoom asks the virtual host controller to plug the device in, sending `PLUGIN_HARDWARE_ONCE` directly to its driver interface (no `usbip.exe` or Visual C++ runtime needed). Windows loads `usbccgp.sys` and `usbser.sys`, both Microsoft drivers, and assigns a COM number.
4. The program's writes arrive as bulk OUT transfers. Data for the program completes pending bulk IN transfers. `SET_LINE_CODING` and `SET_CONTROL_LINE_STATE` carry the baud rate and DTR/RTS. SerialLoom accepts any baud rate and shows it in the topology, but does not slow data down to it: virtual ports always run at full speed.

## Behaviour

- **Lifetime:** ports exist while SerialLoom runs. Closing the window does not stop it: SerialLoom keeps running in the notification area. When it exits (*Quit SerialLoom* in the notification-area menu), the virtual devices are unplugged. On the next start they are plugged in again with the same serial numbers, so Windows reuses the same COM numbers.
- **COM numbers:** Windows first assigns the next free number, as for any USB adapter. When you ask for a specific name (e.g. COM20), SerialLoom then renames the port the way Device Manager's *Port Settings > Advanced* does: it updates the port's `PortName`, claims the number in the COM port database and restarts the port. This needs administrator rights: SerialLoom Helper does it without a prompt, or Windows asks once per batch when the helper is not installed ([details](AUTOMATION.md#serialloom-helper)). Windows remembers the number for that device, so it stays the same on every start. *Virtual ports > Change number* does the same for existing ports, and connections using the port follow the new name.
- **Device Manager:** ports appear under *Ports (COM & LPT)* as "USB Serial Device (COMn)". The controller appears as "USBip 3.X Emulated Host Controller".
- **No data lost to full buffers:** SerialLoom keeps up to 256 KiB per direction. When a program writes faster than the other side takes the data, its write waits, as with a real USB adapter that is busy. When a program stops reading, data for it waits too; once it has not read for 2 seconds, further data for it is dropped and shown as dropped bytes, so the sender and the other members of a split are not held up. Data from a real serial port never waits, because a device cannot be held back: a virtual port that cannot take it at that moment loses it (shown as dropped bytes), and the other members still get everything.

- **Real hardware first:** a virtual port never takes a COM number that belongs to real hardware, even when that device is unplugged and only its reservation remains. Leftovers of deleted SerialLoom ports do not block a number.
- **Deleting a port removes it from Windows:** SerialLoom unplugs the virtual device, then removes its Windows entries (the COM port, the USB function and the USB device) and releases its COM number. This needs administrator rights, handled the same way. Only devices with SerialLoom's USB ID *and* serial-number pattern (`SL` + 10 hex digits) are ever removed. Uninstalling SerialLoom removes all of them.

## Limitations

Programs talk to Windows' own `usbser.sys`, not to SerialLoom, so a few things differ from a classic serial port:

- **Interval timeouts** ([#16](https://github.com/RahulP5/SerialLoom/issues/16)): when a program sets `ReadIntervalTimeout`, a read can return 0 bytes *before the first byte arrives*, after a few milliseconds up to one system timer tick (about 16 ms). By the Win32 rules, the interval timer should only start after the first byte. A program whose peer takes longer to answer sees an empty read. This happens in `usbser.sys`'s timeout handling: SerialLoom never completes a read without data. pyserial's `inter_byte_timeout` sets this timeout, and some Modbus libraries (e.g. `modbus_tk`) set it by default. **Workaround:** use a total timeout only (`ReadTotalTimeoutConstant`; in pyserial, `inter_byte_timeout=None`).
- **CTS:** a USB serial device has no way to report CTS (the CDC `SERIAL_STATE` notification has no CTS bit), so CTS always reads as on. RTS from the other port of a pair does not reach it, and RTS/CTS flow control never holds a write back. RS-485 direction control through RTS cannot be emulated.
- **DSR flow control:** `usbser.sys` does not hold writes while DSR is off, even with DSR flow control (`fOutxDsrFlow`) turned on. Programs can still read DSR and wait for `EV_DSR`.
- **Break:** a program can send a break, but the other port does not see one: `usbser.sys` reports no break event or error for it.
- **RI (ring indicator)** is always off.

## How long will this keep working?

- **License:** usbip-win2 is BSD-2-Clause. The right to use and redistribute the bundled version does not expire, and later license changes by the author cannot take it back.
- **Signature:** the drivers are signed by *Microsoft Windows Hardware Compatibility Publisher* and **timestamped** by the Microsoft Time-Stamp Service. The signing certificate itself expires (for 0.9.8.1: 2026-11-11), but a timestamped signature stays valid after that date, so Windows keeps loading the drivers.
- **What could end it:** Microsoft revoking or blocklisting these drivers (only happens for security problems), or a future Windows release changing the USB device emulation (UDE) framework the controller is built on. Either would affect every usbip-win2 user, and a fixed usbip-win2 release would follow. Updating means changing the pinned version and hash in `scripts/fetch-usbip-win2.ps1`.
- **Long-term exit:** SerialLoom's own driver (`drivers/serialloom-port`) remains in the repository. Once it has a Microsoft signature (see [DRIVER.md](DRIVER.md)), SerialLoom no longer depends on usbip-win2.

## Bundled third-party files

SerialLoom ships these files unmodified from [usbip-win2](https://github.com/vadimgrn/usbip-win2) (BSD-2-Clause, see [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)):

| File | Purpose |
|---|---|
| `driver\usbip2_ude.{inf,sys,cat}` | Virtual USB host controller (signed by Microsoft) |
| `driver\usbip2_filter.{inf,sys,cat}` | Companion filter driver (signed by Microsoft) |

`scripts/fetch-usbip-win2.ps1` collects these files from an installed usbip-win2. Windows keeps each installed driver package unmodified in its driver store, so the copies are exactly the files Microsoft signed. With `-InstallTemporarily`, the script downloads the official release (SHA-256 pinned), installs it silently, copies the files and uninstalls it again; CI does this on its throwaway machines. The script **refuses** driver files that are not signed by "Microsoft Windows Hardware Compatibility Publisher". The files land in `vendor/usbip-win2/`, which is git-ignored, and the build bundles them from there. (The installer itself cannot be unpacked directly: innoextract 1.9 does not read its newer Inno Setup format.)

The SerialLoom installer installs the controller itself (`serialloom.exe --serialloom-admin … install-usb`), so users never run a separate setup. If usbip-win2 is already installed, SerialLoom reuses it. Uninstalling SerialLoom removes the controller only if SerialLoom installed it.

## Before a public release

- Request a dedicated product ID for SerialLoom from [pid.codes](https://pid.codes) and replace the test PID `0x0001` in `usbip/cdc.rs`.
