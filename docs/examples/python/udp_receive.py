"""Receive UDP datagrams (such as NMEA sentences) from SerialLoom and print them.

    python udp_receive.py                          # listens on 127.0.0.1:10111
    python udp_receive.py --listen 0.0.0.0:10111   # also from other PCs
    python udp_receive.py --seconds 10             # stop after 10 s

Each datagram is shown with its sender and size. NMEA sentences in it have
their checksum checked; anything else is shown as text and hexadecimal. Only
Python's standard library is needed. Stop with Ctrl+C; a summary is printed
at the end.
"""
import argparse
import datetime as dt
import socket
import sys
import time
from collections import Counter


def checksum_ok(sentence):
    if '*' not in sentence:
        return None
    body, given = sentence[1:].rsplit('*', 1)
    value = 0
    for ch in body.encode('ascii', 'replace'):
        value ^= ch
    return given.strip().upper() == f'{value:02X}'


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--listen', default='127.0.0.1:10111', help='IP and port to listen on (default 127.0.0.1:10111)')
    ap.add_argument('--seconds', type=float, help='stop after this many seconds')
    args = ap.parse_args()
    host, _, port = args.listen.rpartition(':')

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind((host, int(port)))
    except (OSError, ValueError) as e:
        sys.exit(f'Cannot listen on {args.listen}: {e}')
    sock.settimeout(0.2)

    datagrams, total, bad = 0, 0, 0
    kinds = Counter()
    started = time.time()
    print(f'Listening on {args.listen}. Ctrl+C to stop.', flush=True)
    try:
        while args.seconds is None or time.time() - started < args.seconds:
            try:
                data, sender = sock.recvfrom(65535)
            except socket.timeout:
                continue
            datagrams += 1
            total += len(data)
            stamp = f'{dt.datetime.now():%H:%M:%S.%f}'[:-3]
            text = data.decode('ascii', 'replace')
            sentences = [s.strip() for s in text.splitlines() if s.strip().startswith(('$', '!'))]
            if sentences:
                for sentence in sentences:
                    ok = checksum_ok(sentence)
                    bad += ok is False
                    kinds[sentence[1:].split(',')[0]] += 1
                    mark = {True: '', False: '  !! bad checksum', None: '  (no checksum)'}[ok]
                    print(f'{stamp}  from {sender[0]}:{sender[1]}  {len(data)} B  {sentence}{mark}', flush=True)
            else:
                preview = ''.join(ch if ch.isprintable() else '.' for ch in text[:60])
                print(f'{stamp}  from {sender[0]}:{sender[1]}  {len(data)} B  "{preview}"  {data[:24].hex(" ")}',
                      flush=True)
    except KeyboardInterrupt:
        pass
    finally:
        sock.close()

    seconds = time.time() - started
    print(f'\n{datagrams} datagrams, {total} bytes in {seconds:.1f} s, {bad} sentence(s) with a bad checksum')
    for kind, n in sorted(kinds.items()):
        print(f'  {kind}: {n}')


if __name__ == '__main__':
    main()
