//! Simulated GPS fixes for NMEA 0183 and NMEA 2000 byte streams.
use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::model::{GpsSettings, ReplayFormat};

pub(crate) struct Track {
    latitude: f64,
    longitude: f64,
    course: f64,
    speed: f64,
}

impl Track {
    pub(crate) fn new(settings: Option<&GpsSettings>) -> io::Result<Self> {
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

        let settings = settings
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing simulator settings"))?;
        Ok(Self {
            latitude: value(&settings.latitude, "Latitude", -90.0, 90.0)?,
            longitude: value(&settings.longitude, "Longitude", -180.0, 180.0)?,
            course: value(&settings.course, "True course", 0.0, 360.0)? % 360.0,
            speed: value(&settings.speed, "Speed", 0.0, 100.0)?,
        })
    }

    pub(crate) fn frames(
        &self,
        format: ReplayFormat,
        elapsed: Duration,
        utc: SystemTime,
        sid: u8,
    ) -> [Vec<u8>; 3] {
        let (lat, lon, course) = self.fix(elapsed);
        let unix = utc.duration_since(UNIX_EPOCH).unwrap_or_default();
        let days = unix.as_secs() / 86_400;
        let seconds = unix.as_secs() % 86_400;
        let speed_mps = self.speed * 1852.0 / 3600.0;
        if format == ReplayFormat::Gps2000 {
            let millis = elapsed.as_millis() as u32;
            let mut time = [sid, 0xF0, 0, 0, 0, 0, 0, 0];
            time[2..4].copy_from_slice(&u16::try_from(days).unwrap_or(0xFFFF).to_le_bytes());
            let ticks = seconds as u32 * 10_000 + unix.subsec_micros() / 100;
            time[4..].copy_from_slice(&ticks.to_le_bytes());
            let mut position = [0; 8];
            position[..4].copy_from_slice(&((lat * 1e7).round() as i32).to_le_bytes());
            position[4..].copy_from_slice(&((lon * 1e7).round() as i32).to_le_bytes());
            let mut motion = [sid, 0xFC, 0, 0, 0, 0, 0xFF, 0xFF];
            motion[2..4].copy_from_slice(&((course.to_radians() * 1e4).round() as u16).to_le_bytes());
            motion[4..6].copy_from_slice(&((speed_mps * 100.0).round() as u16).to_le_bytes());
            return [
                actisense(3, 126992, millis, time),
                actisense(2, 129025, millis, position),
                actisense(2, 129026, millis, motion),
            ];
        }
        let (year, month, day) = civil_date(days as i64);
        let date = format!("{day:02}{month:02}{:02}", year % 100);
        let time = format!(
            "{:02}{:02}{:02}.{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60,
            unix.subsec_millis() / 10
        );
        let lat = nmea_coord(lat, 2, 'N', 'S');
        let lon = nmea_coord(lon, 3, 'E', 'W');
        let speed = self.speed;
        let kmh = speed * 1.852;
        [
            sentence(&format!(
                "GPRMC,{time},A,{lat},{lon},{speed:.2},{course:.2},{date},,,A"
            )),
            sentence(&format!("GPGGA,{time},{lat},{lon},1,08,1.0,0.0,M,0.0,M,,")),
            sentence(&format!("GPVTG,{course:.2},T,,M,{speed:.2},N,{kmh:.2},K,A")),
        ]
    }

    fn fix(&self, elapsed: Duration) -> (f64, f64, f64) {
        let angular_distance = elapsed.as_secs_f64() * self.speed * 1852.0 / 3600.0 / 6_371_008.8;
        if angular_distance == 0.0 {
            return (self.latitude, self.longitude, self.course);
        }

        let lat = self.latitude.to_radians();
        let lon = self.longitude.to_radians();
        let heading = self.course.to_radians();
        let (sd, cd) = angular_distance.sin_cos();
        let next_lat = (lat.sin() * cd + lat.cos() * sd * heading.cos())
            .clamp(-1.0, 1.0)
            .asin();
        let delta_lon = (heading.sin() * sd * lat.cos()).atan2(cd - lat.sin() * next_lat.sin());
        // The forward course is the reverse bearing back to the start, plus 180°.
        let reverse = (-delta_lon.sin() * lat.cos())
            .atan2(next_lat.cos() * lat.sin() - next_lat.sin() * lat.cos() * delta_lon.cos());
        (
            next_lat.to_degrees(),
            ((lon + delta_lon).to_degrees() + 180.0).rem_euclid(360.0) - 180.0,
            (reverse.to_degrees() + 180.0).rem_euclid(360.0),
        )
    }
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
    use super::*;

    fn paris() -> Track {
        Track::new(Some(&GpsSettings {
            latitude: "48.8584".into(),
            longitude: "2.2945".into(),
            course: "90".into(),
            speed: "5".into(),
        }))
        .unwrap()
    }

    #[test]
    fn nmea_sentences_follow_utc_and_the_track() {
        let track = paris();
        let utc = UNIX_EPOCH + Duration::from_secs(1_767_225_600);
        let first = track.frames(ReplayFormat::Gps0183, Duration::ZERO, utc, 0);
        let second = track.frames(
            ReplayFormat::Gps0183,
            Duration::from_secs(10),
            utc + Duration::from_secs(10),
            1,
        );
        let rmc = std::str::from_utf8(&first[0]).unwrap();
        assert!(rmc.starts_with("$GPRMC,000000.00,A,4851.5040,N,00217.6700,E,5.00,90.00,010126,"));
        assert!(
            std::str::from_utf8(&second[0])
                .unwrap()
                .starts_with("$GPRMC,000010.00,A,")
        );
        assert!(track.fix(Duration::from_secs(10)).1 > track.longitude);
        for frame in first.iter().chain(&second) {
            let (body, checksum) = std::str::from_utf8(frame).unwrap().split_once('*').unwrap();
            assert_eq!(
                u8::from_str_radix(checksum.strip_suffix("\r\n").unwrap(), 16).unwrap(),
                body[1..].bytes().fold(0, |sum, byte| sum ^ byte)
            );
        }
    }

    #[test]
    fn actisense_frames_contain_three_valid_pgns() {
        let frames = paris().frames(ReplayFormat::Gps2000, Duration::from_millis(16), UNIX_EPOCH, 16);
        for (frame, pgn) in frames.iter().zip([126992, 129025, 129026]) {
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
