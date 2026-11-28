//! Bounded diagnostic capture owned by the routing engine.
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, SecondsFormat, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

const RESERVE: u64 = 20_000_000_000;
const FILE_LIMIT: u64 = 128_000_000;
const QUEUE: usize = 64;
const WINDOW_LINES: usize = 200;
const WINDOW_BYTES: usize = 131_072;

#[derive(Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Rx,
    Tx,
    Both,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileFormat {
    #[default]
    Hex,
    Text,
    Both,
}

#[derive(Deserialize)]
pub struct CaptureRequest {
    pub direction: Direction,
    pub seconds: u64,
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub format: FileFormat,
}

impl CaptureRequest {
    pub(crate) fn validate(&self) -> io::Result<()> {
        if !(1..=600).contains(&self.seconds) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "capture duration must be 1–600 seconds",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CaptureAction {
    Start(CaptureRequest),
    Stop,
    CloseWindow,
    Status,
    Summary,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStatus {
    running: bool,
    pending: bool,
    is_file: bool,
    remaining_seconds: u64,
    dropped: u64,
    reason: Option<&'static str>,
    detail: Option<String>,
    records: Vec<CaptureRecord>,
}

struct Record {
    time: SystemTime,
    elapsed_ms: u64,
    direction: Direction,
    bytes: Vec<u8>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CaptureRecord {
    sequence: u64,
    timestamp: String,
    elapsed_ms: u64,
    direction: Direction,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct Output {
    records: VecDeque<CaptureRecord>,
    bytes: usize,
    sequence: u64,
    end: Option<(&'static str, Option<String>)>,
}

pub(crate) struct Capture {
    direction: Direction,
    start: Instant,
    deadline: Instant,
    stop: Arc<AtomicBool>,
    dropped: AtomicU64,
    is_file: bool,
    output: Arc<Mutex<Output>>,
    queue: SyncSender<Record>,
}

impl CaptureStatus {
    pub fn idle() -> Self {
        status(None, false)
    }
}

pub(crate) fn status(capture: Option<&Capture>, include_records: bool) -> CaptureStatus {
    capture.map_or_else(
        || CaptureStatus {
            running: false,
            pending: false,
            is_file: false,
            remaining_seconds: 0,
            dropped: 0,
            reason: None,
            detail: None,
            records: vec![],
        },
        |capture| capture.status(include_records),
    )
}

pub(crate) fn pending_status(request: Option<&Result<CaptureRequest, String>>) -> CaptureStatus {
    let mut state = status(None, false);
    match request {
        Some(Ok(request)) => {
            state.pending = true;
            state.is_file = request.path.is_some();
            state.remaining_seconds = request.seconds;
        }
        Some(Err(error)) => {
            state.reason = Some("write");
            state.detail = Some(error.clone());
        }
        None => {}
    }
    state
}

impl Capture {
    pub(crate) fn start(request: CaptureRequest) -> io::Result<Self> {
        request.validate()?;
        let CaptureRequest {
            direction,
            seconds,
            path,
            format,
        } = request;
        let mut file = path.map(open_capture).transpose()?;
        let is_file = file.is_some();
        let start = Instant::now();
        let deadline = start + Duration::from_secs(seconds);
        let (queue, receiver) = mpsc::sync_channel::<Record>(QUEUE);
        let output = Arc::new(Mutex::new(Output::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_output = Arc::clone(&output);
        let worker_stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("serialloom-capture".into())
            .spawn(move || {
                let mut file_bytes = 0;
                let end = loop {
                    if worker_stop.load(Relaxed) {
                        break ("stopped", None);
                    }
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break ("expired", None);
                    }
                    let record = match receiver.recv_timeout(remaining.min(Duration::from_millis(100))) {
                        Ok(record) => record,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break ("stopped", None),
                    };
                    if Instant::now() >= deadline {
                        break ("expired", None);
                    }
                    if worker_stop.load(Relaxed) {
                        break ("stopped", None);
                    }
                    if let Some((file, directory)) = &mut file {
                        let line = format_record(&record, format);
                        let size = line.len() as u64;
                        if file_bytes + size > FILE_LIMIT {
                            break ("size", None);
                        }
                        match available(directory) {
                            Ok(free) if free >= RESERVE + size => {}
                            Ok(_) => break ("space", None),
                            Err(error) => break ("space", Some(error.to_string())),
                        }
                        if worker_stop.load(Relaxed) {
                            break ("stopped", None);
                        }
                        if Instant::now() >= deadline {
                            break ("expired", None);
                        }
                        if let Err(error) = file.write_all(line.as_bytes()) {
                            break ("write", Some(error.to_string()));
                        }
                        file_bytes += size;
                    } else {
                        let mut output = worker_output.lock();
                        while output.records.len() >= WINDOW_LINES
                            || output.bytes + record.bytes.len() > WINDOW_BYTES
                        {
                            let Some(oldest) = output.records.pop_front() else {
                                break;
                            };
                            output.bytes -= oldest.bytes.len();
                        }
                        output.bytes += record.bytes.len();
                        output.sequence += 1;
                        let sequence = output.sequence;
                        output.records.push_back(CaptureRecord {
                            sequence,
                            timestamp: timestamp(record.time),
                            elapsed_ms: record.elapsed_ms,
                            direction: record.direction,
                            bytes: record.bytes,
                        });
                    }
                };
                drop(file);
                worker_stop.store(true, Relaxed);
                worker_output.lock().end.get_or_insert(end);
            })?;
        Ok(Self {
            direction,
            start,
            deadline,
            stop,
            dropped: AtomicU64::new(0),
            is_file,
            output,
            queue,
        })
    }

    pub(crate) fn record(&self, direction: Direction, bytes: &[u8]) {
        self.record_at(direction, bytes, SystemTime::now());
    }

    pub(crate) fn record_at(&self, direction: Direction, bytes: &[u8], time: SystemTime) {
        if (self.direction != Direction::Both && self.direction != direction)
            || self.stop.load(Relaxed)
            || Instant::now() >= self.deadline
        {
            return;
        }
        let record = Record {
            time,
            elapsed_ms: self.start.elapsed().as_millis() as u64,
            direction,
            bytes: bytes.to_vec(),
        };
        if let Err(TrySendError::Full(_)) = self.queue.try_send(record) {
            self.dropped.fetch_add(1, Relaxed);
        }
    }

    pub(crate) fn stop(&self) {
        self.stop.store(true, Relaxed);
    }

    pub(crate) fn running(&self) -> bool {
        self.output.lock().end.is_none()
    }

    pub(crate) fn is_file(&self) -> bool {
        self.is_file
    }

    #[cfg(windows)]
    pub(crate) fn fail(&self, message: String) {
        self.output.lock().end = Some(("write", Some(message)));
        self.stop();
    }

    pub(crate) fn status(&self, include_records: bool) -> CaptureStatus {
        let output = self.output.lock();
        let (reason, detail) = output
            .end
            .clone()
            .map_or((None, None), |(reason, detail)| (Some(reason), detail));
        CaptureStatus {
            running: reason.is_none(),
            pending: false,
            is_file: self.is_file,
            remaining_seconds: if reason.is_none() {
                self.deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .div_ceil(1000) as u64
            } else {
                0
            },
            dropped: self.dropped.load(Relaxed),
            reason,
            detail,
            records: if include_records {
                output.records.iter().cloned().collect()
            } else {
                vec![]
            },
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn timestamp(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn format_record(record: &Record, format: FileFormat) -> String {
    use std::fmt::Write as _;
    let direction = if record.direction == Direction::Rx {
        "RX"
    } else {
        "TX"
    };
    let time = timestamp(record.time);
    let prefix = format!("{time}\t{direction}\t{}", record.bytes.len());
    let mut line = prefix.clone();
    if matches!(format, FileFormat::Text | FileFormat::Both) {
        line.push('\t');
        // Escape control and binary bytes to preserve one physical line per record.
        for &byte in &record.bytes {
            match byte {
                b'\r' => line.push_str(r"\r"),
                b'\n' => line.push_str(r"\n"),
                b'\t' => line.push_str(r"\t"),
                b'\\' => line.push_str(r"\\"),
                32..=126 => line.push(char::from(byte)),
                _ => {
                    let _ = write!(line, r"\x{byte:02X}");
                }
            }
        }
    }
    if matches!(format, FileFormat::Hex | FileFormat::Both) {
        if matches!(format, FileFormat::Both) {
            line.push('\n');
            line.push_str(&prefix);
        }
        for byte in &record.bytes {
            line.push(' ');
            let _ = write!(line, "0x{byte:02X}");
        }
    }
    line.push('\n');
    line
}

fn open_capture(path: PathBuf) -> io::Result<(File, PathBuf)> {
    let name = path.file_name().filter(|_| path.is_absolute()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "choose an absolute capture file path",
        )
    })?;
    let directory = path
        .parent()
        .expect("absolute file path has a parent")
        .canonicalize()?;
    if available(&directory)? < RESERVE {
        return Err(io::Error::other("Capture requires 20 GB of free disk space"));
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join(name))?;
    Ok((file, directory))
}

#[cfg(windows)]
fn available(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0;
    // SAFETY: the path is NUL-terminated and `free` points to a writable u64;
    // the other outputs are optional and passed as null.
    if unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(free)
}

#[cfg(unix)]
fn available(path: &Path) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: the path is a valid C string and `stat` has room for the result.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statvfs succeeded, so it initialized `stat`.
    let stat = unsafe { stat.assume_init() };
    Ok((stat.f_bavail as u128 * stat.f_frsize as u128).min(u64::MAX as u128) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_capture_filters_directions_and_expires_while_idle() {
        assert!(
            Capture::start(CaptureRequest {
                direction: Direction::Both,
                seconds: 601,
                path: None,
                format: FileFormat::Hex
            })
            .is_err()
        );
        let capture = Capture::start(CaptureRequest {
            direction: Direction::Tx,
            seconds: 1,
            path: None,
            format: FileFormat::Hex,
        })
        .unwrap();
        capture.record(Direction::Rx, b"ignored");
        capture.record(Direction::Tx, &[0x24, 0xFF]);
        let deadline = Instant::now() + Duration::from_secs(3);
        while capture.running() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(25));
        }
        let status = capture.status(true);
        assert_eq!(status.reason, Some("expired"));
        assert_eq!(status.records.len(), 1);
        assert_eq!(status.records[0].bytes, [0x24, 0xFF]);
        assert!(capture.status(false).records.is_empty());
    }

    #[test]
    fn file_record_is_utc_milliseconds_direction_and_exact_raw_bytes() {
        let record = Record {
            time: SystemTime::UNIX_EPOCH + Duration::from_millis(127),
            elapsed_ms: 0,
            direction: Direction::Rx,
            bytes: vec![0, 0xFF, b'\n'],
        };
        assert_eq!(
            format_record(&record, FileFormat::Hex),
            "1970-01-01T00:00:00.127Z\tRX\t3 0x00 0xFF 0x0A\n"
        );
        assert_eq!(
            format_record(&record, FileFormat::Text),
            "1970-01-01T00:00:00.127Z\tRX\t3\t\\x00\\xFF\\n\n"
        );
        assert_eq!(
            format_record(&record, FileFormat::Both),
            "1970-01-01T00:00:00.127Z\tRX\t3\t\\x00\\xFF\\n\n1970-01-01T00:00:00.127Z\tRX\t3 0x00 0xFF 0x0A\n"
        );
    }
}
