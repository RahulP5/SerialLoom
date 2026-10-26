# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Virtual COM ports as software USB serial devices (USB/IP server + CDC-ACM device in Rust). Windows creates the ports with its own `usbser.sys` through the bundled, Microsoft-signed usbip-win2 virtual USB controller, so they work on any PC without test mode.
- Diagnostics report (Virtual ports > Diagnostics) and per-port driver status.
- Choose COM numbers for virtual USB ports (e.g. COM20) at creation or later with "Change number".
- Deleting a virtual port removes it from Windows completely and frees its COM number.
- COM numbers that belong to real hardware (even when unplugged) are never given to virtual ports.
- Dialogs and the main window fit the screen area above the taskbar.

- SerialLoom's own virtual serial port driver (UMDF 2, Rust), with full serial API behaviour: baud rate, line control, timeouts, DTR/RTS, wait masks and modem lines.
- Virtual COM ports, created and removed from the app with a single administrator prompt per batch.
- Hubs: split, join, bridge and bus, between real serial ports, virtual ports and TCP (server or client).
- Virtual null-modem pairs, built as a bridge between two virtual ports.
- Live topology canvas with drag-and-drop, animated traffic, live throughput, and a traffic monitor (ASCII/hex).
- Persistent topology with autostart.
- Demo driver and browser preview with simulated data.
- Free per-PC driver signing: the installer signs a verified copy of the driver with a one-time certificate and deletes its private key.
- Windows installer that installs the driver, and an uninstaller that removes ports, driver and certificates.
