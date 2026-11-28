use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::model::{ReplayFormat, ReplaySettings};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn validate(settings: &ReplaySettings) -> io::Result<()> {
    if !(1..=60_000).contains(&settings.cycle_ms) {
        return Err(invalid("replay cycle is out of range"));
    }
    if settings.format.is_gps() {
        return crate::gps::Config::parse(settings.gps.as_deref()).map(|_| ());
    }
    if !Path::new(settings.path.trim()).is_absolute() {
        return Err(invalid("choose an absolute replay file path"));
    }
    if !(1..=1_000_000).contains(&settings.bytes_per_second) || !(1..=256).contains(&settings.lines_per_cycle)
    {
        return Err(invalid("replay rate or cycle settings are out of range"));
    }
    if settings.format == ReplayFormat::File && settings.timestamped {
        return Err(invalid("raw file replay uses a byte rate"));
    }
    Ok(())
}

fn deadline(start: Instant, delay: Duration) -> io::Result<Instant> {
    start
        .checked_add(delay)
        .ok_or_else(|| invalid("capture timestamp exceeds the supported duration"))
}

fn wait_until(target: Instant, stopped: &impl Fn() -> bool) -> bool {
    while !stopped() {
        let delay = target.saturating_duration_since(Instant::now());
        if delay.is_zero() {
            return true;
        }
        std::thread::sleep(delay.min(Duration::from_millis(20)));
    }
    false
}

fn decode_line(format: ReplayFormat, line: &str) -> io::Result<Vec<u8>> {
    if format == ReplayFormat::Nmea2000 {
        let hex = line.trim().as_bytes();
        if hex.is_empty() || !hex.len().is_multiple_of(2) {
            return Err(invalid("expected an even number of hexadecimal digits"));
        }
        return hex
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                let text = std::str::from_utf8(pair).map_err(|_| invalid("invalid hex data"))?;
                u8::from_str_radix(text, 16).map_err(|_| invalid("invalid hex data"))
            })
            .collect();
    }
    let sentence = line.trim_end_matches(['\r', '\n']);
    if !sentence.starts_with(['$', '!']) || !sentence.is_ascii() {
        return Err(invalid(
            "expected an ASCII NMEA 0183 sentence starting with $ or !",
        ));
    }
    Ok([sentence.as_bytes(), b"\r\n"].concat())
}

fn open_capture(path: &str) -> io::Result<BufReader<File>> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(invalid("expected a regular replay file"));
    }
    Ok(BufReader::new(File::open(path)?))
}

/// Runs a GPS simulator whose settings can change while it runs. A change
/// takes effect at once: the simulator sends an update with it, then goes
/// on at the (possibly new) interval.
pub(crate) fn run_gps(
    format: ReplayFormat,
    live: &parking_lot::Mutex<crate::gps::LiveSettings>,
    stopped: impl Fn() -> bool,
    mut emit: impl FnMut(&[u8]),
) -> io::Result<()> {
    let (mut version, mut cycle, config) = {
        let settings = live.lock();
        let config = crate::gps::Config::parse(Some(&settings.gps))?;
        (
            settings.version,
            Duration::from_millis(u64::from(settings.cycle_ms)),
            config,
        )
    };
    let mut simulator = crate::gps::Simulator::new(config, Instant::now());
    let mut next = Instant::now();
    loop {
        // Wait for the next update, or for new settings.
        while Instant::now() < next {
            if stopped() {
                return Ok(());
            }
            if live.lock().version != version {
                break;
            }
            std::thread::sleep(
                next.saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(20)),
            );
        }
        if stopped() {
            return Ok(());
        }
        {
            let settings = live.lock();
            if settings.version != version {
                version = settings.version;
                cycle = Duration::from_millis(u64::from(settings.cycle_ms));
                // Settings are checked before they get here; keep the old ones otherwise.
                if let Ok(config) = crate::gps::Config::parse(Some(&settings.gps)) {
                    simulator.update(config);
                }
            }
        }
        let now = Instant::now();
        for frame in simulator.tick(format, now, std::time::SystemTime::now()) {
            if stopped() {
                return Ok(());
            }
            emit(&frame);
        }
        next = now + cycle;
    }
}

pub fn run(
    settings: &ReplaySettings,
    stopped: impl Fn() -> bool,
    mut emit: impl FnMut(&[u8]),
) -> io::Result<()> {
    validate(settings)?;
    let cycle = Duration::from_millis(u64::from(settings.cycle_ms));
    if settings.format.is_gps() {
        let live = parking_lot::Mutex::new(crate::gps::LiveSettings {
            gps: *settings.gps.clone().unwrap_or_else(|| unreachable!("validated")),
            cycle_ms: settings.cycle_ms,
            version: 0,
        });
        return run_gps(settings.format, &live, stopped, emit);
    }
    let mut file = open_capture(settings.path.trim())?;
    while !stopped() {
        let started = Instant::now();
        let raw = settings.format == ReplayFormat::File
            || (settings.format == ReplayFormat::Nmea2000 && !settings.timestamped);
        let mut sent = 0_u64;
        let mut first_ms = None;
        let mut previous_ms = 0;
        let mut line = String::new();
        let mut bytes = [0; 4096];
        while !stopped() {
            if raw {
                let chunk = (settings.bytes_per_second as usize / 50).clamp(1, bytes.len());
                let n = file.read(&mut bytes[..chunk])?;
                if n == 0 {
                    break;
                }
                let offset = Duration::from_secs_f64(sent as f64 / f64::from(settings.bytes_per_second));
                if !wait_until(started + offset, &stopped) {
                    return Ok(());
                }
                emit(&bytes[..n]);
                sent += n as u64;
                continue;
            }

            line.clear();
            let read = Read::by_ref(&mut file).take(65_537).read_line(&mut line)?;
            if read == 0 {
                break;
            }
            if read > 65_536 {
                return Err(invalid("capture line exceeds 64 KiB"));
            }
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let (offset, data) = if settings.timestamped {
                let (time, data) = line
                    .split_once('\t')
                    .ok_or_else(|| invalid("expected milliseconds<TAB>data"))?;
                let ms = time
                    .parse::<u64>()
                    .map_err(|_| invalid("invalid millisecond timestamp"))?;
                if ms < previous_ms {
                    return Err(invalid("capture timestamps must be in increasing order"));
                }
                previous_ms = ms;
                (Duration::from_millis(ms - *first_ms.get_or_insert(ms)), data)
            } else {
                let cycle_index = sent / u64::from(settings.lines_per_cycle);
                (
                    Duration::from_millis(cycle_index.saturating_mul(u64::from(settings.cycle_ms))),
                    line.as_str(),
                )
            };
            let data = decode_line(settings.format, data)?;
            let deadline = deadline(started, offset)?;
            if !wait_until(deadline, &stopped) {
                return Ok(());
            }
            emit(&data);
            sent += 1;
        }
        if stopped() {
            break;
        }
        if sent == 0 {
            return Err(invalid("replay file contains no data"));
        }
        if !settings.repeat {
            break;
        }
        let next = if raw {
            started + Duration::from_secs_f64(sent as f64 / f64::from(settings.bytes_per_second))
        } else if settings.timestamped {
            Instant::now() + cycle
        } else {
            deadline(
                started,
                Duration::from_millis(
                    sent.div_ceil(u64::from(settings.lines_per_cycle))
                        .saturating_mul(u64::from(settings.cycle_ms)),
                ),
            )?
        };
        if !wait_until(next, &stopped) {
            break;
        }
        // Reopen the file at the next iteration of the loop.
        file = open_capture(settings.path.trim())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::demo::DemoDriver;
    use crate::hub::HubRuntime;
    use crate::model::{EndpointSpec, HubEndpoint, HubKind, HubSpec};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;

    fn capture(format: ReplayFormat, timestamped: bool, bytes: &[u8]) -> ReplaySettings {
        let path = std::env::temp_dir().join(format!("serialloom-replay-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, bytes).unwrap();
        ReplaySettings {
            path: path.to_string_lossy().into_owned(),
            format,
            gps: None,
            repeat: false,
            timestamped,
            bytes_per_second: 100,
            lines_per_cycle: 2,
            cycle_ms: 40,
        }
    }

    #[test]
    fn replay_formats_preserve_data_and_cadence() {
        let cases = [
            (
                ReplayFormat::File,
                false,
                &b"abc"[..],
                vec![b"ab".to_vec(), b"c".to_vec()],
                20,
            ),
            (
                ReplayFormat::Nmea0183,
                false,
                &b"$A\n$B\r\n!C\n"[..],
                vec![b"$A\r\n".to_vec(), b"$B\r\n".to_vec(), b"!C\r\n".to_vec()],
                40,
            ),
            (
                ReplayFormat::Nmea0183,
                true,
                &b"800\t$A\n840\t$B\n"[..],
                vec![b"$A\r\n".to_vec(), b"$B\r\n".to_vec()],
                40,
            ),
            (
                ReplayFormat::Nmea2000,
                true,
                &b"100\t10020110\n125\tFF001003\n"[..],
                vec![vec![0x10, 2, 1, 0x10], vec![0xFF, 0, 0x10, 3]],
                25,
            ),
            (
                ReplayFormat::Nmea2000,
                false,
                &b"\x10\x02\xFF"[..],
                vec![vec![0x10, 2], vec![0xFF]],
                20,
            ),
        ];
        for (format, timestamped, bytes, expected, min_ms) in cases {
            let settings = capture(format, timestamped, bytes);
            let start = Instant::now();
            let mut actual = Vec::new();
            let result = run(&settings, || false, |data| actual.push(data.to_vec()));
            std::fs::remove_file(&settings.path).unwrap();
            result.unwrap();
            assert_eq!(actual, expected);
            assert!(start.elapsed() >= Duration::from_millis(min_ms));
        }
    }

    #[test]
    fn replay_source_routes_through_split_and_stops() {
        let mut settings = capture(ReplayFormat::File, false, b"OK");
        settings.repeat = true;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let bind = listener.local_addr().unwrap();
        drop(listener);
        let hub = HubRuntime::start(
            &HubSpec {
                id: "replay-test".into(),
                name: "Replay".into(),
                kind: HubKind::Split,
                master: HubEndpoint {
                    spec: EndpointSpec::Replay {
                        settings: settings.clone(),
                    },
                    created: false,
                },
                members: vec![HubEndpoint {
                    spec: EndpointSpec::TcpServer {
                        bind: bind.to_string(),
                    },
                    created: false,
                }],
                autostart: false,
            },
            Arc::new(DemoDriver::new()),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut client = loop {
            if let Ok(client) = TcpStream::connect(bind) {
                break client;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        };
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut bytes = [0; 6];
        client.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"OKOKOK");
        drop(hub);
        std::fs::remove_file(settings.path).unwrap();
    }

    /// New simulator settings take effect at once, not at the next update.
    #[test]
    fn gps_settings_change_while_running() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let gps: crate::model::GpsSettings =
            serde_json::from_str(r#"{"latitude":"1","longitude":"2","course":"90","speed":"5"}"#).unwrap();
        let live = parking_lot::Mutex::new(crate::gps::LiveSettings {
            gps,
            cycle_ms: 10_000,
            version: 0,
        });
        let stop = AtomicBool::new(false);
        let sent = parking_lot::Mutex::new(Vec::<String>::new());
        let wait_for = |prefix: &str| {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if sent.lock().iter().any(|s| s.starts_with(prefix)) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        };
        std::thread::scope(|scope| {
            scope.spawn(|| {
                run_gps(
                    ReplayFormat::Gps0183,
                    &live,
                    || stop.load(Ordering::Relaxed),
                    |frame| {
                        sent.lock().push(String::from_utf8_lossy(frame).into_owned());
                    },
                )
                .unwrap();
            });
            assert!(wait_for("$GPRMC"), "first update");
            {
                let mut settings = live.lock();
                settings.gps.sentences = vec![crate::model::NmeaSentence::Zda];
                settings.gps.talker = crate::model::NmeaTalker::Gn;
                settings.version += 1;
            }
            let changed = Instant::now();
            assert!(wait_for("$GNZDA"), "the change is sent at once");
            assert!(changed.elapsed() < Duration::from_secs(1));
            stop.store(true, Ordering::Relaxed);
        });
    }
}
