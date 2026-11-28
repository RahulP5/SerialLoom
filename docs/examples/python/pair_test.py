"""Check a virtual null-modem pair: send random data both ways and compare it.

    python pair_test.py COM20 COM21
    python pair_test.py COM20 COM21 --bytes 1000000 --rounds 5

Each round writes the data to one port, reads it from the other and checks
every byte, then does the same the other way. The exit code is 0 when every
round passes, so the script also works in a test bench.
"""
import argparse
import os
import sys
import time

import serial


def transfer(source, destination, size):
    """Writes `size` random bytes to `source`; returns (sent, received, seconds)."""
    data = os.urandom(size)
    destination.reset_input_buffer()
    started = time.perf_counter()
    source.write(data)
    received = bytearray()
    deadline = time.time() + 10 + size / 100_000
    while len(received) < size and time.time() < deadline:
        received += destination.read(size - len(received))
    return data, bytes(received), time.perf_counter() - started


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('port_a', help='first port of the pair, e.g. COM20')
    ap.add_argument('port_b', help='second port of the pair, e.g. COM21')
    ap.add_argument('--bytes', type=int, default=65536, help='bytes per direction and round (default 65536)')
    ap.add_argument('--rounds', type=int, default=3, help='number of rounds (default 3)')
    ap.add_argument('--baud', type=int, default=115200, help='baud rate (virtual ports ignore it)')
    args = ap.parse_args()

    try:
        a = serial.Serial(args.port_a, args.baud, timeout=0.5)
        b = serial.Serial(args.port_b, args.baud, timeout=0.5)
    except serial.SerialException as e:
        sys.exit(f'Cannot open the ports: {e}')

    failures = 0
    with a, b:
        for n in range(1, args.rounds + 1):
            for source, destination in ((a, b), (b, a)):
                sent, received, seconds = transfer(source, destination, args.bytes)
                if received == sent:
                    result = 'ok'
                else:
                    failures += 1
                    first = next((i for i, (x, y) in enumerate(zip(sent, received)) if x != y), len(received))
                    result = f'WRONG: {len(received)} of {len(sent)} bytes arrived, first difference at byte {first}'
                rate = len(received) / seconds / 1e6 if seconds else 0
                print(f'round {n}  {source.port} -> {destination.port}  {len(received)} bytes  '
                      f'{rate:.1f} MB/s  {result}', flush=True)

    print('PASS' if failures == 0 else f'FAIL: {failures} transfer(s) went wrong')
    sys.exit(1 if failures else 0)


if __name__ == '__main__':
    main()
