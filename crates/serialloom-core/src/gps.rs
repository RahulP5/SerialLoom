//! Simulated GPS receiver: NMEA 0183 sentences, or NMEA 2000 frames in
//! Actisense format.
//!
//! The simulator keeps its own position and, at each update, moves it by
//! the course and speed in force. Its settings can change while it runs: a
//! new course or speed carries on from the current position, a new
//! position moves there.
use std::io;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::model::{GpsFix, GpsMotion, GpsSettings, NmeaSentence, NmeaTalker, ReplayFormat};

const EARTH_RADIUS_M: f64 = 6_371_008.8;
const KNOT_MPS: f64 = 1852.0 / 3600.0;
/// PRNs of the simulated constellation, in the order satellites come into view.
const PRNS: [u8; 16] = [2, 5, 7, 9, 12, 13, 15, 17, 19, 20, 24, 25, 28, 29, 30, 32];

/// A running simulator's settings, which can change while it runs.
pub(crate) struct LiveSettings {
    pub gps: GpsSettings,
    pub cycle_ms: u32,
    /// Changes with every update, so the simulator notices it.
    pub version: u64,
}

/// Checked settings.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Config {
    latitude: f64,
    longitude: f64,
    course: f64,
    /// Knots.
    speed: f64,
    moving: bool,
    altitude: f64,
    fix: GpsFix,
    satellites: u8,
    hdop: f64,
    vdop: f64,
    pdop: f64,
    variation: Option<f64>,
    /// Metres.
    noise: f64,
    talker: &'static str,
    sentences: Vec<NmeaSentence>,
}

impl Config {
    pub(crate) fn parse(settings: Option<&GpsSettings>) -> io::Result<Self> {
        fn value(text: &str, name: &str, min: f64, max: f64) -> io::Result<f64> {
            text.trim()
                .parse::<f64>()
                .ok()
                .filter(|value| (min..=max).contains(value))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{name} must be between {min} and {max}"),
                    )
                })
        }

        let s = settings
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing simulator settings"))?;
        let mut sentences = s.sentences.clone();
        sentences.sort();
        sentences.dedup();
        Ok(Self {
            latitude: value(&s.latitude, "Latitude", -90.0, 90.0)?,
            longitude: value(&s.longitude, "Longitude", -180.0, 180.0)?,
            course: value(&s.course, "True course", 0.0, 360.0)? % 360.0,
            speed: value(&s.speed, "Speed", 0.0, 1000.0)?,
            moving: s.motion == GpsMotion::Moving,
            altitude: value(&s.altitude, "Altitude", -1000.0, 100_000.0)?,
            fix: s.fix,
            satellites: value(&s.satellites, "Satellites", 0.0, 12.0)?.round() as u8,
            hdop: value(&s.hdop, "HDOP", 0.5, 99.9)?,
            vdop: value(&s.vdop, "VDOP", 0.5, 99.9)?,
            pdop: value(&s.pdop, "PDOP", 0.5, 99.9)?,
            variation: match s.variation.trim() {
                "" => None,
                text => Some(value(text, "Magnetic variation", -180.0, 180.0)?),
            },
            noise: value(&s.noise, "Position noise", 0.0, 10_000.0)?,
            talker: match s.talker {
                NmeaTalker::Gp => "GP",
                NmeaTalker::Gn => "GN",
            },
            sentences,
        })
    }

    fn valid(&self) -> bool {
        self.fix != GpsFix::None
    }

    fn reported_speed(&self) -> f64 {
        if self.moving { self.speed } else { 0.0 }
    }
}

struct Satellite {
    prn: u8,
    elevation: u8,
    azimuth: u16,
    snr: u8,
}

pub(crate) struct Simulator {
    config: Config,
    /// True position, before noise.
    latitude: f64,
    longitude: f64,
    last: Option<Instant>,
    started: Instant,
    sid: u8,
    rng: u64,
}

impl Simulator {
    pub(crate) fn new(config: Config, now: Instant) -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        Self {
            latitude: config.latitude,
            longitude: config.longitude,
            config,
            last: None,
            started: now,
            sid: 0,
            rng: seed | 1,
        }
    }

    /// Takes on new settings. A new position moves there; otherwise the
    /// simulator carries on from where it is, with the new course and speed.
    pub(crate) fn update(&mut self, config: Config) {
        if (config.latitude, config.longitude) != (self.config.latitude, self.config.longitude) {
            self.latitude = config.latitude;
            self.longitude = config.longitude;
        }
        self.config = config;
    }

    /// Moves on to `now` and returns this update's sentences or frames.
    pub(crate) fn tick(&mut self, format: ReplayFormat, now: Instant, utc: SystemTime) -> Vec<Vec<u8>> {
        let (latitude, longitude) = self.advance(now);
        let sid = self.sid;
        self.sid = (self.sid + 1) % 253;
        if format == ReplayFormat::Gps2000 {
            self.nmea2000(latitude, longitude, now, utc, sid)
        } else {
            self.nmea0183(latitude, longitude, now, utc)
        }
    }

    /// Dead reckoning since the last update; returns the reported position.
    fn advance(&mut self, now: Instant) -> (f64, f64) {
        if let Some(last) = self.last
            && self.config.moving
        {
            let metres = now.saturating_duration_since(last).as_secs_f64() * self.config.speed * KNOT_MPS;
            (self.latitude, self.longitude) =
                destination(self.latitude, self.longitude, self.config.course, metres);
        }
        self.last = Some(now);
        if self.config.noise > 0.0 {
            let radius = self.config.noise * self.random().sqrt();
            let bearing = 360.0 * self.random();
            destination(self.latitude, self.longitude, bearing, radius)
        } else {
            (self.latitude, self.longitude)
        }
    }

    /// Uniform in [0, 1) (xorshift64*).
    fn random(&mut self) -> f64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Satellites in view, slowly moving across the sky; the first
    /// `satellites` of them are used in the fix.
    fn sky(&self, now: Instant) -> Vec<Satellite> {
        let t = now.saturating_duration_since(self.started).as_secs_f64();
        let used = if self.config.valid() {
            self.config.satellites
        } else {
            0
        };
        let in_view = (usize::from(self.config.satellites) + 3).clamp(4, PRNS.len());
        PRNS.iter()
            .take(in_view)
            .enumerate()
            .map(|(i, &prn)| {
                let phase = f64::from(prn) * 0.7;
                Satellite {
                    prn,
                    elevation: (45.0 + 38.0 * (t / 900.0 + phase).sin()).clamp(5.0, 88.0) as u8,
                    azimuth: ((f64::from(prn) * 47.0 + t / 60.0) % 360.0) as u16,
                    snr: if i < usize::from(used) {
                        30 + prn * 7 % 18
                    } else {
                        10 + prn * 3 % 15
                    },
                }
            })
            .collect()
    }

    fn nmea0183(&self, latitude: f64, longitude: f64, now: Instant, utc: SystemTime) -> Vec<Vec<u8>> {
        let c = &self.config;
        let t = c.talker;
        let unix = utc.duration_since(UNIX_EPOCH).unwrap_or_default();
        let seconds = unix.as_secs() % 86_400;
        let (year, month, day) = civil_date((unix.as_secs() / 86_400) as i64);
        let time = format!(
            "{:02}{:02}{:02}.{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60,
            unix.subsec_millis() / 10
        );
        let date = format!("{day:02}{month:02}{:02}", year % 100);
        let valid = c.valid();
        let (lat, lon) = if valid {
            (
                nmea_coord(latitude, 2, 'N', 'S'),
                nmea_coord(longitude, 3, 'E', 'W'),
            )
        } else {
            (",".to_string(), ",".to_string())
        };
        let speed = c.reported_speed();
        let (sog, cog, kmh) = if valid {
            (
                format!("{speed:.2}"),
                format!("{:.2}", c.course),
                format!("{:.2}", speed * 1.852),
            )
        } else {
            Default::default()
        };
        let magnetic = match c.variation {
            Some(v) if valid => format!("{:.2}", (c.course - v).rem_euclid(360.0)),
            _ => String::new(),
        };
        let variation = c.variation.map_or_else(
            || ",".to_string(),
            |v| format!("{:.1},{}", v.abs(), if v < 0.0 { 'W' } else { 'E' }),
        );
        let status = if valid { "A" } else { "V" };
        let mode = match c.fix {
            GpsFix::None => "N",
            GpsFix::Gps => "A",
            GpsFix::Dgps => "D",
            GpsFix::Rtk => "R",
            GpsFix::Float => "F",
            GpsFix::Estimated => "E",
        };
        let quality = match c.fix {
            GpsFix::None => 0,
            GpsFix::Gps => 1,
            GpsFix::Dgps => 2,
            GpsFix::Rtk => 4,
            GpsFix::Float => 5,
            GpsFix::Estimated => 6,
        };
        let used = if valid { usize::from(c.satellites) } else { 0 };
        let sky = self.sky(now);

        let mut out = Vec::new();
        for kind in &c.sentences {
            match kind {
                NmeaSentence::Rmc => out.push(sentence(&format!(
                    "{t}RMC,{time},{status},{lat},{lon},{sog},{cog},{date},{variation},{mode}"
                ))),
                NmeaSentence::Gga if valid => out.push(sentence(&format!(
                    "{t}GGA,{time},{lat},{lon},{quality},{used:02},{:.1},{:.1},M,0.0,M,,",
                    c.hdop, c.altitude
                ))),
                NmeaSentence::Gga => out.push(sentence(&format!("{t}GGA,{time},,,,,0,00,,,M,,M,,"))),
                NmeaSentence::Gll => {
                    out.push(sentence(&format!("{t}GLL,{lat},{lon},{time},{status},{mode}")));
                }
                NmeaSentence::Gsa => {
                    let prns: Vec<String> = (0..12)
                        .map(|i| {
                            sky.get(i)
                                .filter(|_| i < used)
                                .map_or(String::new(), |s| format!("{:02}", s.prn))
                        })
                        .collect();
                    let dops = if valid {
                        format!("{:.1},{:.1},{:.1}", c.pdop, c.hdop, c.vdop)
                    } else {
                        ",,".to_string()
                    };
                    let kind = if valid { 3 } else { 1 };
                    out.push(sentence(&format!("{t}GSA,A,{kind},{},{dops}", prns.join(","))));
                }
                // Satellites in view are listed per system; these are GPS satellites.
                NmeaSentence::Gsv => {
                    let messages = sky.len().div_ceil(4);
                    for (n, group) in sky.chunks(4).enumerate() {
                        let blocks: String = group
                            .iter()
                            .map(|s| {
                                format!(",{:02},{:02},{:03},{:02}", s.prn, s.elevation, s.azimuth, s.snr)
                            })
                            .collect();
                        out.push(sentence(&format!(
                            "GPGSV,{messages},{},{:02}{blocks}",
                            n + 1,
                            sky.len()
                        )));
                    }
                }
                NmeaSentence::Vtg => out.push(sentence(&format!(
                    "{t}VTG,{cog},T,{magnetic},M,{sog},N,{kmh},K,{mode}"
                ))),
                NmeaSentence::Zda => out.push(sentence(&format!(
                    "{t}ZDA,{time},{day:02},{month:02},{year:04},00,00"
                ))),
                NmeaSentence::Hdt => out.push(sentence(&format!("{t}HDT,{:.2},T", c.course))),
            }
        }
        out
    }

    fn nmea2000(
        &self,
        latitude: f64,
        longitude: f64,
        now: Instant,
        utc: SystemTime,
        sid: u8,
    ) -> Vec<Vec<u8>> {
        let c = &self.config;
        let valid = c.valid();
        let millis = now.saturating_duration_since(self.started).as_millis() as u32;
        let unix = utc.duration_since(UNIX_EPOCH).unwrap_or_default();
        let days = unix.as_secs() / 86_400;
        let seconds = unix.as_secs() % 86_400;

        // 126992 System Time: SID, source GPS, days since 1970, time of day in 0.1 ms.
        let mut time = [sid, 0xF0, 0, 0, 0, 0, 0, 0];
        time[2..4].copy_from_slice(&u16::try_from(days).unwrap_or(0xFFFF).to_le_bytes());
        let ticks = seconds as u32 * 10_000 + unix.subsec_micros() / 100;
        time[4..].copy_from_slice(&ticks.to_le_bytes());

        // 129025 Position, Rapid Update: latitude, longitude in 1e-7 degrees.
        let mut position = [0xFF, 0xFF, 0xFF, 0x7F, 0xFF, 0xFF, 0xFF, 0x7F];
        if valid {
            position[..4].copy_from_slice(&((latitude * 1e7).round() as i32).to_le_bytes());
            position[4..].copy_from_slice(&((longitude * 1e7).round() as i32).to_le_bytes());
        }

        // 129026 COG & SOG, Rapid Update: true course in 1e-4 rad, speed in 0.01 m/s.
        let mut motion = [sid, 0xFC, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        if valid {
            motion[2..4].copy_from_slice(&((c.course.to_radians() * 1e4).round() as u16).to_le_bytes());
            motion[4..6]
                .copy_from_slice(&((c.reported_speed() * KNOT_MPS * 100.0).round() as u16).to_le_bytes());
        }

        // 129539 GNSS DOPs: desired mode auto, actual mode 3D (or error), HDOP, VDOP in 0.01, TDOP unknown.
        let actual = if valid { 2 } else { 6 };
        let mut dops = [
            sid,
            3 | (actual << 3) | (0b11 << 6),
            0xFF,
            0x7F,
            0xFF,
            0x7F,
            0xFF,
            0x7F,
        ];
        if valid {
            dops[2..4].copy_from_slice(&((c.hdop * 100.0).round() as i16).to_le_bytes());
            dops[4..6].copy_from_slice(&((c.vdop * 100.0).round() as i16).to_le_bytes());
        }

        // 127250 Vessel Heading: true heading in 1e-4 rad, deviation unknown, variation if known.
        let mut heading = [sid, 0, 0, 0xFF, 0x7F, 0xFF, 0x7F, 0xFC];
        heading[1..3].copy_from_slice(&((c.course.to_radians() * 1e4).round() as u16).to_le_bytes());
        if let Some(v) = c.variation {
            heading[5..7].copy_from_slice(&((v.to_radians() * 1e4).round() as i16).to_le_bytes());
        }

        vec![
            actisense(3, 126992, millis, time),
            actisense(2, 129025, millis, position),
            actisense(2, 129026, millis, motion),
            actisense(6, 129539, millis, dops),
            actisense(2, 127250, millis, heading),
        ]
    }
}

/// The point `metres` away from a start along a great circle, at a bearing.
fn destination(latitude: f64, longitude: f64, bearing: f64, metres: f64) -> (f64, f64) {
    let angular = metres / EARTH_RADIUS_M;
    if angular == 0.0 {
        return (latitude, longitude);
    }
    let lat = latitude.to_radians();
    let lon = longitude.to_radians();
    let heading = bearing.to_radians();
    let (sd, cd) = angular.sin_cos();
    let next_lat = (lat.sin() * cd + lat.cos() * sd * heading.cos())
        .clamp(-1.0, 1.0)
        .asin();
    let delta_lon = (heading.sin() * sd * lat.cos()).atan2(cd - lat.sin() * next_lat.sin());
    (
        next_lat.to_degrees(),
        ((lon + delta_lon).to_degrees() + 180.0).rem_euclid(360.0) - 180.0,
    )
}

fn sentence(body: &str) -> Vec<u8> {
    let checksum = body.bytes().fold(0, |acc, byte| acc ^ byte);
    format!("${body}*{checksum:02X}\r\n").into_bytes()
}

fn nmea_coord(value: f64, degree_digits: usize, positive: char, negative: char) -> String {
    let hemisphere = if value < 0.0 { negative } else { positive };
    let minutes = (value.abs() * 600_000.0).round() as u64;
    let degrees = minutes / 600_000;
    let whole_minutes = minutes / 10_000 % 60;
    let fraction = minutes % 10_000;
    format!("{degrees:0degree_digits$}{whole_minutes:02}.{fraction:04},{hemisphere}")
}

fn actisense(priority: u8, pgn: u32, millis: u32, data: [u8; 8]) -> Vec<u8> {
    let mut message = vec![0x93, 19, priority];
    message.extend_from_slice(&pgn.to_le_bytes()[..3]);
    message.extend_from_slice(&[255, 0]); // Broadcast, simulated source address 0.
    message.extend_from_slice(&millis.to_le_bytes());
    message.push(data.len() as u8);
    message.extend_from_slice(&data);
    message.push(message.iter().fold(0_u8, |sum, byte| sum.wrapping_sub(*byte)));

    let mut frame = vec![0x10, 0x02];
    for byte in message {
        if byte == 0x10 {
            frame.push(0x10);
        }
        frame.push(byte);
    }
    frame.extend_from_slice(&[0x10, 0x03]);
    frame
}

// Convert Unix days using 400-year Gregorian eras, with March as month zero.
fn civil_date(unix_days: i64) -> (i64, i64, i64) {
    let shifted = unix_days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = (march_month + 2) % 12 + 1;
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// The settings a 0.5.0 connection saved before the extra fields existed.
    fn paris() -> GpsSettings {
        serde_json::from_str(r#"{"latitude":"48.8584","longitude":"2.2945","course":"90","speed":"5"}"#)
            .unwrap()
    }

    fn all_sentences() -> GpsSettings {
        GpsSettings {
            sentences: vec![
                NmeaSentence::Hdt,
                NmeaSentence::Zda,
                NmeaSentence::Vtg,
                NmeaSentence::Gsv,
                NmeaSentence::Gsa,
                NmeaSentence::Gll,
                NmeaSentence::Gga,
                NmeaSentence::Rmc,
            ],
            variation: "-2.5".into(),
            ..paris()
        }
    }

    fn simulator(settings: &GpsSettings, now: Instant) -> Simulator {
        Simulator::new(Config::parse(Some(settings)).unwrap(), now)
    }

    fn text(frames: &[Vec<u8>]) -> Vec<String> {
        frames
            .iter()
            .map(|f| String::from_utf8(f.clone()).unwrap())
            .collect()
    }

    /// `$GPRMC,...` -> ("GPRMC", fields after the name), checking the checksum.
    fn fields(sentence: &str) -> (String, Vec<String>) {
        let (body, checksum) = sentence.strip_prefix('$').unwrap().split_once('*').unwrap();
        assert_eq!(
            u8::from_str_radix(checksum.strip_suffix("\r\n").unwrap(), 16).unwrap(),
            body.bytes().fold(0, |sum, byte| sum ^ byte),
            "checksum of {sentence}"
        );
        let mut parts = body.split(',').map(str::to_string);
        (parts.next().unwrap(), parts.collect())
    }

    const NEW_YEAR_2026: u64 = 1_767_225_600;

    #[test]
    fn older_settings_load_and_send_rmc_gga_vtg_as_before() {
        let now = Instant::now();
        let utc = UNIX_EPOCH + Duration::from_secs(NEW_YEAR_2026);
        let mut sim = simulator(&paris(), now);
        let first = text(&sim.tick(ReplayFormat::Gps0183, now, utc));
        assert_eq!(first.len(), 3);
        assert!(first[0].starts_with("$GPRMC,000000.00,A,4851.5040,N,00217.6700,E,5.00,90.00,010126,,,A*"));
        assert!(first[1].starts_with("$GPGGA,000000.00,4851.5040,N,00217.6700,E,1,08,1.0,35.0,M,0.0,M,,*"));
        assert!(first[2].starts_with("$GPVTG,90.00,T,,M,5.00,N,9.26,K,A*"));
        let later = text(&sim.tick(
            ReplayFormat::Gps0183,
            now + Duration::from_secs(10),
            utc + Duration::from_secs(10),
        ));
        // 5 kn east for 10 s is 25.7 m: 0.0211' of longitude at this latitude.
        assert!(
            later[0].starts_with("$GPRMC,000010.00,A,4851.5040,N,00217.6911,E,"),
            "{}",
            later[0]
        );
    }

    #[test]
    fn every_sentence_has_its_fields_and_a_valid_checksum() {
        let now = Instant::now();
        let utc = UNIX_EPOCH + Duration::from_secs(NEW_YEAR_2026 + 3723);
        let frames = text(&simulator(&all_sentences(), now).tick(ReplayFormat::Gps0183, now, utc));
        let parsed: Vec<_> = frames.iter().map(|s| fields(s)).collect();
        let names: Vec<&str> = parsed.iter().map(|(n, _)| n.as_str()).collect();
        // Fixed order, whatever order they were chosen in; GSV in three parts for 11 satellites.
        assert_eq!(
            names,
            [
                "GPRMC", "GPGGA", "GPGLL", "GPGSA", "GPGSV", "GPGSV", "GPGSV", "GPVTG", "GPZDA", "GPHDT"
            ]
        );
        let field_counts: Vec<usize> = parsed.iter().map(|(_, f)| f.len()).collect();
        assert_eq!(field_counts, [12, 14, 7, 17, 19, 19, 15, 9, 6, 2]);

        let (_, rmc) = &parsed[0];
        assert_eq!((rmc[9].as_str(), rmc[10].as_str()), ("2.5", "W"));
        let (_, gsa) = &parsed[3];
        assert_eq!((gsa[0].as_str(), gsa[1].as_str()), ("A", "3"));
        assert_eq!(gsa[2..14].iter().filter(|p| !p.is_empty()).count(), 8);
        assert_eq!(
            (gsa[14].as_str(), gsa[15].as_str(), gsa[16].as_str()),
            ("1.8", "1.0", "1.5")
        );
        let (_, gsv) = &parsed[4];
        assert_eq!((gsv[0].as_str(), gsv[2].as_str()), ("3", "11"));
        let (_, vtg) = &parsed[7];
        assert_eq!(vtg[2], "92.50", "magnetic course = true course - variation");
        let (_, zda) = &parsed[8];
        assert_eq!(&zda[..4], ["010203.00", "01", "01", "2026"]);
    }

    #[test]
    fn a_new_course_carries_on_from_the_current_position() {
        let t0 = Instant::now();
        let mut settings = paris();
        settings.speed = "600".into();
        let mut sim = simulator(&settings, t0);
        sim.tick(ReplayFormat::Gps0183, t0, UNIX_EPOCH);
        sim.tick(ReplayFormat::Gps0183, t0 + Duration::from_secs(60), UNIX_EPOCH);
        let (lat, lon) = (sim.latitude, sim.longitude);
        assert!(
            lon > 2.2945 + 0.1 && (lat - 48.8584).abs() < 0.01,
            "moved east: {lat}, {lon}"
        );

        settings.course = "0".into();
        sim.update(Config::parse(Some(&settings)).unwrap());
        assert_eq!(
            (sim.latitude, sim.longitude),
            (lat, lon),
            "no jump on a new course"
        );
        sim.tick(ReplayFormat::Gps0183, t0 + Duration::from_secs(120), UNIX_EPOCH);
        assert!(
            sim.latitude > lat + 0.1 && (sim.longitude - lon).abs() < 1e-9,
            "now north"
        );

        settings.latitude = "10".into();
        settings.longitude = "20".into();
        sim.update(Config::parse(Some(&settings)).unwrap());
        assert_eq!(
            (sim.latitude, sim.longitude),
            (10.0, 20.0),
            "a new position moves there"
        );
    }

    #[test]
    fn fixed_motion_stays_put_and_reports_no_speed() {
        let t0 = Instant::now();
        let settings = GpsSettings {
            motion: GpsMotion::Fixed,
            ..paris()
        };
        let mut sim = simulator(&settings, t0);
        sim.tick(ReplayFormat::Gps0183, t0, UNIX_EPOCH);
        let later = text(&sim.tick(ReplayFormat::Gps0183, t0 + Duration::from_secs(3600), UNIX_EPOCH));
        assert_eq!((sim.latitude, sim.longitude), (48.8584, 2.2945));
        assert_eq!(fields(&later[0]).1[6], "0.00");
    }

    #[test]
    fn no_fix_empties_the_position_and_marks_it_invalid() {
        let now = Instant::now();
        let settings = GpsSettings {
            fix: GpsFix::None,
            ..all_sentences()
        };
        let frames = text(&simulator(&settings, now).tick(ReplayFormat::Gps0183, now, UNIX_EPOCH));
        let (_, rmc) = fields(&frames[0]);
        assert_eq!(
            (rmc[1].as_str(), rmc[2].as_str(), rmc[11].as_str()),
            ("V", "", "N")
        );
        let (_, gga) = fields(&frames[1]);
        assert_eq!(
            (gga[1].as_str(), gga[5].as_str(), gga[6].as_str()),
            ("", "0", "00")
        );
        let (_, gsa) = fields(&frames[3]);
        assert_eq!(gsa[1], "1");
        assert!(gsa[2..14].iter().all(String::is_empty));
    }

    #[test]
    fn fix_kinds_talker_and_noise() {
        let now = Instant::now();
        for (fix, quality, mode) in [
            (GpsFix::Dgps, "2", "D"),
            (GpsFix::Rtk, "4", "R"),
            (GpsFix::Estimated, "6", "E"),
        ] {
            let settings = GpsSettings {
                fix,
                talker: NmeaTalker::Gn,
                ..paris()
            };
            let frames = text(&simulator(&settings, now).tick(ReplayFormat::Gps0183, now, UNIX_EPOCH));
            let (name, rmc) = fields(&frames[0]);
            assert_eq!((name.as_str(), rmc[11].as_str()), ("GNRMC", mode));
            assert_eq!(fields(&frames[1]).1[5], quality);
        }

        let settings = GpsSettings {
            motion: GpsMotion::Fixed,
            noise: "50".into(),
            ..paris()
        };
        let mut sim = simulator(&settings, now);
        let mut seen = std::collections::HashSet::new();
        for i in 0..20 {
            let (lat, lon) = sim.advance(now + Duration::from_secs(i));
            let metres = ((lat - 48.8584).to_radians().powi(2)
                + ((lon - 2.2945).to_radians() * 48.8584_f64.to_radians().cos()).powi(2))
            .sqrt()
                * EARTH_RADIUS_M;
            assert!(metres <= 50.5, "{metres} m");
            seen.insert(format!("{lat:.6},{lon:.6}"));
        }
        assert!(seen.len() > 15, "the position wanders");
        assert_eq!(
            (sim.latitude, sim.longitude),
            (48.8584, 2.2945),
            "noise does not move the track"
        );
    }

    #[test]
    fn out_of_range_values_are_refused() {
        for (field, value) in [
            ("latitude", "91"),
            ("speed", "-1"),
            ("satellites", "13"),
            ("hdop", "0"),
            ("variation", "x"),
        ] {
            let mut json = serde_json::to_value(paris()).unwrap();
            json[field] = value.into();
            let settings: GpsSettings = serde_json::from_value(json).unwrap();
            assert!(Config::parse(Some(&settings)).is_err(), "{field} = {value}");
        }
        assert!(Config::parse(None).is_err());
    }

    #[test]
    fn actisense_frames_contain_five_valid_pgns() {
        let now = Instant::now();
        let settings = GpsSettings {
            variation: "3".into(),
            ..paris()
        };
        let frames = simulator(&settings, now).tick(ReplayFormat::Gps2000, now, UNIX_EPOCH);
        let pgns = [126992, 129025, 129026, 129539, 127250];
        assert_eq!(frames.len(), pgns.len());
        for (frame, pgn) in frames.iter().zip(pgns) {
            assert_eq!(&frame[..2], &[0x10, 2]);
            assert_eq!(&frame[frame.len() - 2..], &[0x10, 3]);
            let mut bytes = frame[2..frame.len() - 2].iter().copied();
            let mut body = Vec::new();
            while let Some(byte) = bytes.next() {
                if byte == 0x10 {
                    assert_eq!(bytes.next(), Some(0x10));
                }
                body.push(byte);
            }
            assert_eq!(body.len(), 22);
            assert_eq!(&body[..2], &[0x93, 19]);
            assert_eq!(body[12], 8);
            assert_eq!(u32::from_le_bytes([body[3], body[4], body[5], 0]), pgn);
            assert_eq!(body.iter().copied().fold(0, u8::wrapping_add), 0);
        }
    }
}
