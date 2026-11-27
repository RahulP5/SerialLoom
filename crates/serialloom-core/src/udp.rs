use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, UdpSocket};

use crate::model::{UdpFraming, UdpSettings};

const MAX_DATAGRAM: usize = 65_507;
const MAX_SENTENCE: usize = 1024;

pub(crate) fn addresses(settings: &UdpSettings) -> io::Result<(SocketAddr, Option<SocketAddr>)> {
    fn invalid(message: &str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidInput, message)
    }
    let bind: SocketAddr = settings
        .bind
        .trim()
        .parse()
        .map_err(|_| invalid("UDP local address must be an IP and port, such as 0.0.0.0:10110"))?;
    let destination: Option<SocketAddr> =
        if settings.destination.trim().is_empty() {
            None
        } else {
            Some(
                settings.destination.trim().parse().map_err(|_| {
                    invalid("UDP destination must be an IP and port, such as 127.0.0.1:10110")
                })?,
            )
        };
    if settings.receive && bind.port() == 0 {
        return Err(invalid("UDP reception requires a nonzero local port"));
    }
    if !settings.receive && destination.is_none() {
        return Err(invalid("enable UDP reception or specify a destination"));
    }
    if destination.is_some_and(|addr| {
        addr.is_ipv4() != bind.is_ipv4() || addr.port() == 0 || addr.ip().is_unspecified()
    }) {
        return Err(invalid(
            "UDP destination needs a specific IP and nonzero port in the local address family",
        ));
    }
    Ok((bind, destination))
}

pub(crate) struct UdpWriter {
    socket: UdpSocket,
    destination: SocketAddr,
    framing: UdpFraming,
    sentences: HashMap<usize, Vec<u8>>,
}

impl UdpWriter {
    pub(crate) fn new(socket: UdpSocket, destination: SocketAddr, framing: UdpFraming) -> Self {
        Self {
            socket,
            destination,
            framing,
            sentences: HashMap::new(),
        }
    }

    /// Returns bytes sent, bytes discarded, and the most recent send error.
    pub(crate) fn send(
        &mut self,
        source: usize,
        data: &[u8],
        mut sent: impl FnMut(&[u8]),
    ) -> (usize, usize, Option<io::Error>) {
        let mut written = 0;
        let mut dropped = 0;
        let mut error = None;
        let socket = &self.socket;
        let destination = self.destination;
        let mut send = |bytes: &[u8]| match socket.send_to(bytes, destination) {
            Ok(count) => {
                sent(&bytes[..count]);
                written += count;
                dropped += bytes.len() - count;
            }
            Err(err) => {
                dropped += bytes.len();
                error = Some(err);
            }
        };

        match self.framing {
            UdpFraming::Raw => {
                for chunk in data.chunks(MAX_DATAGRAM) {
                    send(chunk);
                }
            }
            UdpFraming::Nmea0183 => {
                let sentence = self.sentences.entry(source).or_default();
                let mut discarded = 0;
                for &byte in data {
                    if matches!(byte, b'$' | b'!') {
                        discarded += sentence.len();
                        sentence.clear();
                    } else if sentence.is_empty() || sentence.len() == MAX_SENTENCE {
                        discarded += sentence.len() + 1;
                        sentence.clear();
                        continue;
                    }
                    sentence.push(byte);
                    if byte == b'\n' {
                        send(sentence);
                        sentence.clear();
                    }
                }
                dropped += discarded;
            }
        }
        (written, dropped, error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn raw_udp_preserves_binary_and_nmea_frames_each_source() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut writer = UdpWriter::new(sender, receiver.local_addr().unwrap(), UdpFraming::Raw);
        let mut buf = [0; 1024];
        let n = writer.send(0, b"\x00\xFF", |_| {}).0;
        assert_eq!(n, 2);
        let (n, _) = receiver.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"\x00\xFF");

        writer.framing = UdpFraming::Nmea0183;
        let mut captured = Vec::new();
        let mut sent = |bytes: &[u8]| captured.push(bytes.to_vec());
        assert_eq!(writer.send(0, b"$GP", &mut sent).0, 0);
        writer.send(1, b"!AIS\r\n", &mut sent);
        writer.send(0, b"RMC\r\n$GPGGA\r\n", sent);
        let expected = [&b"!AIS\r\n"[..], &b"$GPRMC\r\n"[..], &b"$GPGGA\r\n"[..]];
        assert_eq!(captured, expected);
        for expected in expected {
            let (n, _) = receiver.recv_from(&mut buf).unwrap();
            assert_eq!(&buf[..n], expected);
        }
    }
}
