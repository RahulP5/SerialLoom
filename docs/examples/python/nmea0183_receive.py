"""Receive NMEA 0183 sentences from a COM port, check them and print their values.

    python nmea0183_receive.py COM15
    python nmea0183_receive.py COM15 --raw            # also show each sentence as received
    python nmea0183_receive.py COM15 --seconds 10     # stop after 10 s
    python nmea0183_receive.py COM15 --type RMC       # only RMC sentences

Checks every checksum and decodes RMC, GGA, GLL, GSA, GSV, VTG, ZDA and HDT;
other sentences are shown as they are. Stop with Ctrl+C; a summary of what
arrived is printed at the end.
"""
import argparse
import datetime as dt
import sys
import time
from collections import Counter

import serial

QUALITY = {'0': 'none', '1': 'GPS', '2': 'DGPS', '3': 'PPS', '4': 'RTK fixed', '5': 'RTK float',
           '6': 'estimated', '7': 'manual', '8': 'simulation'}
MODE = {'A': 'autonomous', 'D': 'differential', 'E': 'estimated', 'F': 'RTK float', 'M': 'manual',
        'N': 'not valid', 'P': 'precise', 'R': 'RTK fixed', 'S': 'simulator'}


def checksum_ok(line):
    """True or False for a sentence with a checksum, None for one without."""
    if '*' not in line:
        return None
    body, given = line[1:].rsplit('*', 1)
    value = 0
    for ch in body.encode('ascii', 'replace'):
        value ^= ch
    return given.strip().upper() == f'{value:02X}'


def coordinate(value, hemisphere, degree_digits):
    """`4851.5040`, `N` -> '48.858400 N'; empty fields -> 'n/a'."""
    if not value or not hemisphere:
        return 'n/a'
    degrees = int(value[:degree_digits]) + float(value[degree_digits:]) / 60
    return f'{degrees:.6f} {hemisphere}'


def utc(hhmmss):
    return f'{hhmmss[0:2]}:{hhmmss[2:4]}:{hhmmss[4:]}' if len(hhmmss) >= 6 else 'n/a'


def text(value, unit=''):
    return f'{value}{unit}' if value else 'n/a'


def rmc(f):
    date = f'20{f[9][4:6]}-{f[9][2:4]}-{f[9][0:2]}' if len(f[9]) == 6 else 'n/a'
    variation = f'{f[10]}° {f[11]}' if f[10] else 'n/a'
    mode = MODE.get(f[12], f[12]) if len(f) > 12 else 'n/a'
    return (f'{"valid" if f[2] == "A" else "NOT valid"}, {coordinate(f[3], f[4], 2)}, {coordinate(f[5], f[6], 3)}, '
            f'{text(f[7], " kn")}, course {text(f[8], "°")}, {date} {utc(f[1])} UTC, variation {variation}, {mode}')


def gga(f):
    return (f'{coordinate(f[2], f[3], 2)}, {coordinate(f[4], f[5], 3)}, fix {QUALITY.get(f[6], f[6])}, '
            f'{text(f[7].lstrip("0") or f[7][-1:])} satellites, HDOP {text(f[8])}, altitude {text(f[9], " m")}, {utc(f[1])} UTC')


def gll(f):
    mode = MODE.get(f[7], f[7]) if len(f) > 7 else 'n/a'
    return (f'{coordinate(f[1], f[2], 2)}, {coordinate(f[3], f[4], 3)}, {utc(f[5])} UTC, '
            f'{"valid" if f[6] == "A" else "NOT valid"}, {mode}')


def gsa(f):
    fix = {'1': 'no fix', '2': '2D', '3': '3D'}.get(f[2], f[2])
    used = [p for p in f[3:15] if p]
    return f'{fix}, {len(used)} used ({" ".join(used) or "none"}), PDOP {text(f[15])}, HDOP {text(f[16])}, VDOP {text(f[17])}'


def gsv(f):
    satellites = []
    for i in range(4, len(f) - 3, 4):
        prn, elevation, azimuth, snr = f[i:i + 4]
        satellites.append(f'{prn} el {text(elevation, "°")} az {text(azimuth, "°")} SNR {text(snr)}')
    return f'message {f[2]} of {f[1]}, {text(f[3])} in view: ' + '; '.join(satellites)


def vtg(f):
    mode = MODE.get(f[9], f[9]) if len(f) > 9 else 'n/a'
    return (f'course {text(f[1], "° true")}, {text(f[3], "° magnetic")}, '
            f'speed {text(f[5], " kn")} = {text(f[7], " km/h")}, {mode}')


def zda(f):
    return f'{f[4]}-{f[3]}-{f[2]} {utc(f[1])} UTC' if f[4] else 'n/a'


def hdt(f):
    return f'heading {text(f[1], "° true")}'


DECODERS = {'RMC': rmc, 'GGA': gga, 'GLL': gll, 'GSA': gsa, 'GSV': gsv, 'VTG': vtg, 'ZDA': zda, 'HDT': hdt}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('port', help='COM port, e.g. COM15')
    ap.add_argument('--baud', type=int, default=4800, help='baud rate (default 4800; virtual ports ignore it)')
    ap.add_argument('--raw', action='store_true', help='also print each sentence as received')
    ap.add_argument('--seconds', type=float, help='stop after this many seconds')
    ap.add_argument('--type', action='append', help='only show this sentence type, e.g. RMC (can be repeated)')
    args = ap.parse_args()
    wanted = {t.upper() for t in args.type or []}

    counts, bad = Counter(), 0
    started = time.time()
    try:
        with serial.Serial(args.port, args.baud, timeout=0.2) as port:
            print(f'Listening on {args.port}. Ctrl+C to stop.', flush=True)
            while args.seconds is None or time.time() - started < args.seconds:
                raw = port.readline()
                if not raw:
                    continue
                line = raw.decode('ascii', 'replace').strip()
                if not line.startswith(('$', '!')):
                    continue
                ok = checksum_ok(line)
                if ok is False:
                    bad += 1
                    print(f'!! bad checksum: {line}', flush=True)
                    continue
                fields = line[1:].split('*')[0].split(',')
                address = fields[0]
                kind = address[-3:]
                counts[address] += 1
                if wanted and kind not in wanted:
                    continue
                decode = DECODERS.get(kind)
                try:
                    detail = decode(fields) if decode else ','.join(fields[1:])
                except (IndexError, ValueError):
                    detail = f'could not decode: {line}'
                stamp = f'{dt.datetime.now():%H:%M:%S.%f}'[:-3]
                print(f'{stamp}  {address:<6} {detail}{"" if ok else "  (no checksum)"}', flush=True)
                if args.raw:
                    print(f'    {line}', flush=True)
    except serial.SerialException as e:
        sys.exit(f'Cannot use {args.port}: {e}')
    except KeyboardInterrupt:
        pass

    seconds = time.time() - started
    print(f'\n{sum(counts.values())} valid sentences in {seconds:.1f} s, {bad} with a bad checksum')
    for address, n in sorted(counts.items()):
        print(f'  {address}: {n}, {n / seconds:.2f}/s')


if __name__ == '__main__':
    main()
