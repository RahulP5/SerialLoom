"""Receive NMEA 2000 messages (Actisense format) from a COM port, check them and print their values.

    python nmea2000_receive.py COM16
    python nmea2000_receive.py COM16 --raw          # also show each frame in hex
    python nmea2000_receive.py COM16 --seconds 10   # stop after 10 s
    python nmea2000_receive.py COM16 --pgn 129025   # only positions

Checks every frame and decodes PGNs 126992 (system time), 129025 (position),
129026 (course and speed), 129539 (DOPs) and 127250 (heading); other PGNs
are shown as raw data. Works with SerialLoom's NMEA 2000 simulator and with
Actisense NGT-1 style gateways. Stop with Ctrl+C; a summary of what arrived
is printed at the end.
"""
import argparse
import datetime as dt
import math
import struct
import sys
import time
from collections import Counter

import serial

DLE, STX, ETX = 0x10, 0x02, 0x03
N2K_RECEIVED = 0x93  # Actisense "NMEA 2000 message received"
KNOT = 1852 / 3600  # metres per second


class ActisenseReader:
    """Splits the byte stream into Actisense messages (DLE STX ... DLE ETX).

    A 0x10 inside a message is sent twice (DLE DLE); this undoes that.
    """

    def __init__(self):
        self.message = None  # bytes of the message being read, None between messages
        self.escape = False

    def feed(self, data):
        for byte in data:
            if self.escape:
                self.escape = False
                if byte == STX:
                    self.message = bytearray()
                elif byte == ETX and self.message is not None:
                    yield bytes(self.message)
                    self.message = None
                elif byte == DLE and self.message is not None:
                    self.message.append(DLE)
                else:
                    self.message = None  # out of step: wait for the next DLE STX
            elif byte == DLE:
                self.escape = True
            elif self.message is not None:
                self.message.append(byte)


def parse(message):
    """Checks an Actisense N2K message and returns its fields, or an error text."""
    if len(message) < 13 or message[0] != N2K_RECEIVED:
        return None, f'not an NMEA 2000 message (command 0x{message[0]:02X})' if message else 'empty message'
    if sum(message) % 256:
        return None, 'bad checksum'
    if message[1] != len(message) - 3:
        return None, f'length byte {message[1]} does not match {len(message) - 3}'
    size = message[12]
    data = message[13:13 + size]
    if len(data) != size:
        return None, 'data shorter than its length byte'
    return {
        'priority': message[2],
        'pgn': int.from_bytes(message[3:6], 'little'),
        'destination': message[6],
        'source': message[7],
        'timestamp_ms': int.from_bytes(message[8:12], 'little'),
        'data': data,
    }, None


# Field helpers: NMEA 2000 marks a missing value with all bits set (0x7F.. for signed).
def u16(d, i):
    v = struct.unpack_from('<H', d, i)[0]
    return None if v == 0xFFFF else v


def i16(d, i):
    v = struct.unpack_from('<h', d, i)[0]
    return None if v == 0x7FFF else v


def i32(d, i):
    v = struct.unpack_from('<i', d, i)[0]
    return None if v == 0x7FFFFFFF else v


def angle(raw):
    """1e-4 rad to degrees."""
    return None if raw is None else math.degrees(raw * 1e-4)


def show(value, fmt, unit=''):
    return 'n/a' if value is None else f'{value:{fmt}}{unit}'


def system_time(d):
    days, ticks = u16(d, 2), struct.unpack_from('<I', d, 4)[0]
    sources = {0: 'GPS', 1: 'GLONASS', 2: 'radio station', 3: 'local cesium clock', 4: 'local rubidium clock'}
    source = sources.get(d[1] & 0x0F, f'source {d[1] & 0x0F}')
    if days is None or ticks == 0xFFFFFFFF:
        return f'System time: n/a ({source})'
    when = dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(days=days, seconds=ticks / 10_000)
    return f'System time: {when:%Y-%m-%d %H:%M:%S.%f}'[:-2] + f' UTC ({source})'


def position(d):
    lat, lon = i32(d, 0), i32(d, 4)
    if lat is None or lon is None:
        return 'Position: n/a (no fix)'
    lat, lon = lat * 1e-7, lon * 1e-7
    return f'Position: {abs(lat):.7f} {"N" if lat >= 0 else "S"}, {abs(lon):.7f} {"E" if lon >= 0 else "W"}'


def cog_sog(d):
    reference = {0: 'true', 1: 'magnetic'}.get(d[1] & 0x03, 'n/a')
    cog, sog = angle(u16(d, 2)), u16(d, 4)
    knots = None if sog is None else sog * 0.01 / KNOT
    return f'Course {show(cog, ".2f", "°")} {reference}, speed {show(knots, ".2f", " kn")}'


def dops(d):
    modes = {0: '1D', 1: '2D', 2: '3D', 3: 'auto', 6: 'error', 7: 'n/a'}
    desired, actual = modes.get(d[1] & 0x07, '?'), modes.get((d[1] >> 3) & 0x07, '?')
    hdop, vdop, tdop = (None if v is None else v * 0.01 for v in (i16(d, 2), i16(d, 4), i16(d, 6)))
    return (f'DOPs: fix {actual} (wanted {desired}), HDOP {show(hdop, ".2f")}, '
            f'VDOP {show(vdop, ".2f")}, TDOP {show(tdop, ".2f")}')


def heading(d):
    reference = {0: 'true', 1: 'magnetic'}.get(d[7] & 0x03, 'n/a')
    value, deviation, variation = angle(u16(d, 1)), angle(i16(d, 3)), angle(i16(d, 5))
    var = 'n/a' if variation is None else f'{abs(variation):.1f}° {"E" if variation >= 0 else "W"}'
    return f'Heading {show(value, ".2f", "°")} {reference}, deviation {show(deviation, ".1f", "°")}, variation {var}'


DECODERS = {
    126992: system_time,
    129025: position,
    129026: cog_sog,
    129539: dops,
    127250: heading,
}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('port', help='COM port, e.g. COM16')
    ap.add_argument('--baud', type=int, default=115200, help='baud rate (default 115200; virtual ports ignore it)')
    ap.add_argument('--raw', action='store_true', help='also print each frame in hex')
    ap.add_argument('--seconds', type=float, help='stop after this many seconds')
    ap.add_argument('--pgn', type=int, action='append', help='only show this PGN (can be repeated)')
    args = ap.parse_args()

    reader = ActisenseReader()
    counts, errors = Counter(), Counter()
    started = time.time()
    try:
        with serial.Serial(args.port, args.baud, timeout=0.2) as port:
            print(f'Listening on {args.port}. Ctrl+C to stop.', flush=True)
            while args.seconds is None or time.time() - started < args.seconds:
                for message in reader.feed(port.read(4096)):
                    fields, error = parse(message)
                    if error:
                        errors[error] += 1
                        print(f'!! {error}: {message.hex(" ")}', flush=True)
                        continue
                    pgn = fields['pgn']
                    counts[pgn] += 1
                    if args.pgn and pgn not in args.pgn:
                        continue
                    decode = DECODERS.get(pgn)
                    text = decode(fields['data']) if decode else f'(no decoder) data {fields["data"].hex(" ")}'
                    print(f'{dt.datetime.now():%H:%M:%S.%f}'[:-3]
                          + f'  PGN {pgn:<6} src {fields["source"]:<3} prio {fields["priority"]}  {text}', flush=True)
                    if args.raw:
                        print(f'    {message.hex(" ")}', flush=True)
    except serial.SerialException as e:
        sys.exit(f'Cannot use {args.port}: {e}')
    except KeyboardInterrupt:
        pass

    seconds = time.time() - started
    print(f'\n{sum(counts.values())} valid messages in {seconds:.1f} s, {sum(errors.values())} bad')
    for pgn, n in sorted(counts.items()):
        name = DECODERS[pgn].__name__ if pgn in DECODERS else 'other'
        print(f'  PGN {pgn} ({name}): {n}, {n / seconds:.2f}/s')
    for error, n in errors.items():
        print(f'  bad: {error}: {n}')


if __name__ == '__main__':
    main()
