//! Passive USB bulk payload capture using the USBPcap extcap executable.
use std::io::{self, Read};
use std::time::{Duration, SystemTime};

use crate::capture::Direction;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn header(input: &mut impl Read) -> io::Result<()> {
    let mut header = [0; 24];
    input.read_exact(&mut header)?;
    if header[..4] != [0xd4, 0xc3, 0xb2, 0xa1]
        || u32::from_le_bytes(header[20..24].try_into().unwrap()) != 249
    {
        return Err(invalid(
            "Expected a little-endian USBPcap capture (link type 249)",
        ));
    }
    Ok(())
}

fn read_packets(
    input: &mut impl Read,
    device: u16,
    signals: Option<&parking_lot::Mutex<Signals>>,
    mut record: impl FnMut(Direction, &[u8], SystemTime),
) -> io::Result<()> {
    let mut header = [0; 16];
    let mut packet = Vec::new();
    loop {
        input.read_exact(&mut header)?;
        let size = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
        if size > 1_048_576 {
            return Err(invalid("USB capture packet exceeds the supported size"));
        }
        packet.resize(size, 0);
        input.read_exact(&mut packet)?;
        let seconds = u32::from_le_bytes(header[..4].try_into().unwrap());
        let micros = u32::from_le_bytes(header[4..8].try_into().unwrap());
        let timestamp = SystemTime::UNIX_EPOCH
            + Duration::from_secs(u64::from(seconds))
            + Duration::from_micros(u64::from(micros));
        if let Some((direction, bytes)) = payload(&packet, device) {
            if let (Some(signals), Direction::Rx) = (signals, direction) {
                let mut data = Vec::with_capacity(bytes.len());
                let mut signals = signals.lock();
                // FT232R and FT-X use 64-byte bulk packets.
                for chunk in bytes.chunks(64).filter(|chunk| chunk.len() >= 2) {
                    signals.receive(chunk, timestamp);
                    data.extend_from_slice(&chunk[2..]);
                }
                drop(signals);
                if !data.is_empty() {
                    record(direction, &data, timestamp);
                }
            } else {
                record(direction, bytes, timestamp);
            }
        } else if let Some(signals) = signals {
            signals.lock().control(&packet, device, timestamp);
        }
    }
}

fn payload(packet: &[u8], device: u16) -> Option<(Direction, &[u8])> {
    let header = packet.get(..27)?;
    let header_len = u16::from_le_bytes(header[0..2].try_into().ok()?) as usize;
    let address = u16::from_le_bytes(header[19..21].try_into().ok()?);
    let size = u32::from_le_bytes(header[23..27].try_into().ok()?) as usize;
    if header_len < 27 || address != device || header[22] != 3 || size == 0 {
        return None;
    }
    let input = header[21] & 0x80 != 0;
    let completion = header[16] & 1 != 0;
    if input != completion || (input && header[10..14] != [0; 4]) {
        return None;
    }
    Some((
        if input { Direction::Rx } else { Direction::Tx },
        packet.get(header_len..header_len.checked_add(size)?)?,
    ))
}

#[derive(Default, serde::Serialize)]
pub struct Signals {
    levels: [Option<bool>; 6],
    changes: std::collections::VecDeque<(SystemTime, usize, bool)>,
}

impl Signals {
    fn set(&mut self, index: usize, value: bool, timestamp: SystemTime) {
        if self.levels[index] == Some(value) {
            return;
        }
        self.levels[index] = Some(value);
        if self.changes.len() == 50 {
            self.changes.pop_front();
        }
        self.changes.push_back((timestamp, index, value));
    }

    fn receive(&mut self, packet: &[u8], timestamp: SystemTime) {
        for (index, mask) in [0x80, 0x10, 0x20, 0x40].into_iter().enumerate() {
            self.set(index, packet[0] & mask != 0, timestamp);
        }
    }

    fn control(&mut self, packet: &[u8], device: u16, timestamp: SystemTime) {
        let Some(header) = packet.get(..28) else { return };
        let offset = usize::from(u16::from_le_bytes([header[0], header[1]]));
        if offset < 28
            || u16::from_le_bytes([header[19], header[20]]) != device
            || header[22] != 2
            || header[27] != 0
            || header[16] & 1 != 0
        {
            return;
        }
        let Some([0x40, 1, value, mask, _, _, 0, 0]) = packet.get(offset..offset + 8) else {
            return;
        };
        for (index, bit) in [(4, 1), (5, 2)] {
            if mask & bit != 0 {
                self.set(index, value & bit != 0, timestamp);
            }
        }
    }
}

fn extcap_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let start = format!("{{{key}=");
    let (_, rest) = line.split_once(&start)?;
    rest.split_once('}').map(|(value, _)| value)
}

fn matching_addresses(config: &str, port: &str) -> std::collections::BTreeSet<u16> {
    let suffix = format!("({port})");
    config
        .lines()
        .filter_map(|line| {
            if !line.starts_with("value ") {
                return None;
            }
            let display = extcap_field(line, "display")?.trim_end();
            if !display.eq_ignore_ascii_case(port)
                && !display
                    .to_ascii_uppercase()
                    .ends_with(&suffix.to_ascii_uppercase())
            {
                return None;
            }
            // Child nodes use values such as 7_2, where 7 is the USB address.
            let value = extcap_field(line, "value")?;
            let address = value.split('_').next()?.parse().ok()?;
            (1..=127).contains(&address).then_some(address)
        })
        .collect()
}

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::capture::{Capture, CaptureRequest};
    use std::io::BufReader;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::{Arc, mpsc};

    fn extcap(executable: &std::path::Path, args: &[&str]) -> io::Result<String> {
        let output = Command::new(executable)
            .args(args)
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .output()?;
        if !output.status.success() {
            return Err(invalid("USBPcap could not enumerate USB devices"));
        }
        String::from_utf8(output.stdout).map_err(|error| invalid(&error.to_string()))
    }

    pub struct UsbProbe(Arc<Capture>, Option<Arc<parking_lot::Mutex<Signals>>>);

    impl UsbProbe {
        pub fn start(port: &str, request: CaptureRequest) -> io::Result<Self> {
            request.validate()?;
            // Prevent USBPcap from spawning its own elevated worker outside our lifecycle.
            // SAFETY: IsUserAnAdmin takes no arguments and only reads the process token.
            if unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() } == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Run SerialLoom as administrator to use USBPcap",
                ));
            }
            let executable = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
                .into_iter()
                .filter_map(std::env::var_os)
                .map(std::path::PathBuf::from)
                .flat_map(|dir| {
                    [
                        dir.join("USBPcap/USBPcapCMD.exe"),
                        dir.join("Wireshark/extcap/USBPcapCMD.exe"),
                    ]
                })
                .find(|path| path.is_file())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "USBPcapCMD.exe is missing. Install USBPcap.",
                    )
                })?;

            let mut candidates = std::collections::BTreeSet::new();
            let interfaces = extcap(&executable, &["--extcap-interfaces"])?;
            for line in interfaces.lines().filter(|line| line.starts_with("interface ")) {
                let Some(interface) = extcap_field(line, "value") else {
                    continue;
                };
                let Some(number) = interface.strip_prefix(r"\\.\USBPcap") else {
                    continue;
                };
                if number.parse::<u16>().is_err() {
                    continue;
                }
                let config = extcap(&executable, &["--extcap-interface", interface, "--extcap-config"])?;
                for address in matching_addresses(&config, port) {
                    candidates.insert((interface.to_owned(), address));
                }
            }
            if candidates.len() != 1 {
                return Err(invalid(&format!(
                    "USBPcap could not uniquely identify {port}. Check that the USB device is connected and USBPcap is installed."
                )));
            }
            let (interface, device) = candidates.into_iter().next().unwrap();

            let ftdi = serialport::available_ports()
                .unwrap_or_default()
                .into_iter()
                .any(|entry| {
                    entry.port_name.eq_ignore_ascii_case(port)
                        && matches!(
                            entry.port_type,
                            serialport::SerialPortType::UsbPort(info)
                                if info.vid == 0x0403 && matches!(info.pid, 0x6001 | 0x6015)
                        )
                });
            let signals = ftdi.then(|| Arc::new(parking_lot::Mutex::new(Signals::default())));
            let reader_signals = signals.clone();

            let mut child = Command::new(executable)
                .args([
                    "-d",
                    &interface,
                    "--devices",
                    &device.to_string(),
                    "-s",
                    "131072",
                    "-o",
                    "-",
                ])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?;
            let mut input = BufReader::new(child.stdout.take().expect("piped stdout"));
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            let (capture_tx, capture_rx) = mpsc::sync_channel::<Arc<Capture>>(1);
            let reader = std::thread::spawn(move || {
                let result = header(&mut input);
                let ready = result.is_ok();
                if ready_tx.send(result).is_ok()
                    && ready
                    && let Ok(capture) = capture_rx.recv()
                {
                    let result = read_packets(
                        &mut input,
                        device,
                        reader_signals.as_deref(),
                        |direction, bytes, timestamp| capture.record_at(direction, bytes, timestamp),
                    );
                    if capture.running() {
                        capture.fail(result.unwrap_err().to_string());
                    }
                }
            });

            let start = ready_rx.recv_timeout(Duration::from_secs(5))
                .map_err(|_| invalid("USBPcap did not start. Check the adapter and run SerialLoom as administrator."))
                .and_then(|result| result.map_err(|error| invalid(&format!(
                    "USBPcap could not start. Check the interface, USB address and administrator permissions: {error}"
                ))))
                .and_then(|()| Capture::start(request).map(Arc::new));
            let capture = match start {
                Ok(capture) => capture,
                Err(error) => {
                    drop(capture_tx);
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err(error);
                }
            };
            let _ = capture_tx.send(Arc::clone(&capture));
            let worker = Arc::clone(&capture);
            std::thread::spawn(move || {
                while worker.running() {
                    std::thread::sleep(Duration::from_millis(100));
                }
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
            });
            Ok(Self(capture, signals))
        }

        pub fn stop(&self) {
            self.0.stop();
        }

        pub fn running(&self) -> bool {
            self.0.running()
        }

        pub fn status(&self) -> serde_json::Value {
            let mut status = serde_json::to_value(self.0.status(true)).expect("serializable capture");
            if let Some(signals) = &self.1 {
                status["signals"] = serde_json::to_value(&*signals.lock()).expect("serializable signals");
            }
            status
        }
    }

    impl Drop for UsbProbe {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(windows)]
pub use windows::UsbProbe;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_signal_changes_and_leaves_unseen_commands_unknown() {
        let mut signals = Signals::default();
        let time = SystemTime::UNIX_EPOCH;
        signals.receive(&[0x01, 0x60], time);
        signals.receive(&[0x01, 0x60], time);
        signals.receive(&[0x91, 0x60], time);
        assert_eq!(signals.changes.len(), 6);
        assert_eq!(
            signals.levels,
            [Some(true), Some(true), Some(false), Some(false), None, None]
        );

        let mut packet = [0; 36];
        packet[0] = 28;
        packet[19] = 7;
        packet[22] = 2;
        packet[28..].copy_from_slice(&[0x40, 1, 1, 1, 0, 0, 0, 0]);
        signals.control(&packet, 8, time);
        assert_eq!(signals.levels[4], None);
        signals.control(&packet, 7, time);
        assert_eq!(signals.levels[4..], [Some(true), None]);
    }

    #[test]
    fn identifies_exact_com_port_on_parent_or_child() {
        let config = concat!(
            "value {arg=5}{value=4}{display=[4] USB Serial Device (COM70)}{enabled=true}\n",
            "value {arg=5}{value=7_2}{display=USB Serial Port (COM7)}{enabled=false}{parent=7}\n",
            "value {arg=5}{value=8}{display=[8] USB Serial Device (COM8)}{enabled=true}\n",
        );
        assert_eq!(matching_addresses(config, "com7"), [7].into());
        assert_eq!(matching_addresses(config, "COM8"), [8].into());
        assert!(matching_addresses(config, "COM9").is_empty());
    }

    #[test]
    fn captures_only_matching_bulk_payloads_once() {
        let mut p = [0; 29];
        p[0] = 27;
        p[19] = 7;
        p[22] = 3;
        p[23] = 2;
        p[27..].copy_from_slice(b"AB");
        assert_eq!(payload(&p, 7).unwrap().1, b"AB");
        assert!(matches!(payload(&p, 7).unwrap().0, Direction::Tx));
        assert!(payload(&p, 8).is_none());
        p[16] = 1;
        assert!(payload(&p, 7).is_none());
        p[21] = 0x81;
        assert!(matches!(payload(&p, 7).unwrap().0, Direction::Rx));
        assert!(payload(&p[..28], 7).is_none());
    }

    #[test]
    fn parses_pcap_header_and_bulk_records() {
        let mut stream = vec![0; 24 + 16 + 28];
        stream[..4].copy_from_slice(&[0xd4, 0xc3, 0xb2, 0xa1]);
        stream[20] = 249;
        stream[32] = 28;
        stream[40] = 27;
        stream[59] = 7;
        stream[62] = 3;
        stream[63] = 1;
        stream[67] = b'A';
        let mut input = stream.as_slice();
        header(&mut input).unwrap();
        let mut result = Vec::new();
        let _ = read_packets(&mut input, 7, None, |_, bytes, _| result.extend_from_slice(bytes));
        assert_eq!(result, b"A");
    }
}
