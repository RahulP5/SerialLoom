"""Compare a SerialLoom serial Split with and without a non-reading client."""

import argparse
import hashlib
import struct
import threading
import time
from contextlib import ExitStack

import serial

READ_IDLE_TIMEOUT = 15
BLOCK_SIZE = 4096


def receive(port, expected_bytes, result):
    digest = hashlib.sha256()
    count = 0
    reason = None
    last_data = time.monotonic()
    try:
        while count < expected_bytes:
            data = port.read(min(4096, expected_bytes - count))
            if data:
                digest.update(data)
                count += len(data)
                last_data = time.monotonic()
            elif time.monotonic() - last_data >= READ_IDLE_TIMEOUT:
                reason = f"no data received for {READ_IDLE_TIMEOUT} seconds"
                break
    except OSError as error:
        reason = str(error)
    result.update(count=count, digest=digest.hexdigest(), reason=reason)


def run(args):
    expected_bytes = args.blocks * BLOCK_SIZE
    result = {}
    with ExitStack() as ports:
        fast = ports.enter_context(serial.Serial(args.fast, args.fast_baud, timeout=0.25))
        if args.slow:
            ports.enter_context(serial.Serial(args.slow_port, args.slow_baud))
            # Keep output A open without ever reading from it.
        source = ports.enter_context(
            serial.Serial(args.source, args.source_baud, write_timeout=10)
        )
        print(f"Source: {args.source} @ {args.source_baud} baud")
        print(f"Output B: {args.fast} @ {args.fast_baud} baud")
        if args.slow:
            print(f"Output A: {args.slow_port} @ {args.slow_baud} baud")
        print(f"Mode: {'non-reading client on output A' if args.slow else 'output A closed'}")
        if not args.yes:
            input("Confirm the expected connections in SerialLoom, then press Enter. ")

        sent_digest = hashlib.sha256()
        sent_blocks = 0
        send_error = None
        padding = bytes(BLOCK_SIZE - 4)
        reader = threading.Thread(target=receive, args=(fast, expected_bytes, result))
        started = time.monotonic()
        reader.start()
        try:
            for index in range(args.blocks):
                block = struct.pack("<I", index) + padding
                if source.write(block) != len(block):
                    raise OSError("incomplete serial write")
                sent_digest.update(block)
                sent_blocks += 1
                time.sleep(0.004)
        except serial.SerialTimeoutException:
            send_error = "send timed out after 10 seconds"
        except OSError as error:
            send_error = str(error)
        finally:
            # Keep both outputs open until the receiver finishes or times out.
            reader.join()
        elapsed = time.monotonic() - started

        passed = (
            send_error is None
            and result["count"] == expected_bytes
            and result["digest"] == sent_digest.hexdigest()
        )
        print(f"Source: {sent_blocks:,}/{args.blocks:,} complete blocks sent")
        if send_error:
            print(f"Source error: {send_error}")
        print(f"Output B: {result['count']:,}/{expected_bytes:,} bytes received")
        if result["reason"]:
            print(f"Output B: {result['reason']}")
        print(f"Elapsed: {elapsed:.1f} s")
        print("PASS: full stream received in order" if passed else "FAIL: stream incomplete or different")
        if args.slow and passed:
            print("Check that output A recorded dropped bytes to confirm saturation.")
        return 0 if passed else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", help="sender-side serial port, e.g. COM41")
    parser.add_argument("slow_port", help="serial port for output A, e.g. COM43")
    parser.add_argument("fast", help="serial port for output B, e.g. COM45")
    parser.add_argument("--source-baud", type=int, default=115200)
    parser.add_argument("--slow-baud", type=int, default=115200)
    parser.add_argument("--fast-baud", type=int, default=115200)
    parser.add_argument("--slow", "--lent", action="store_true", help="connect output A and never read")
    parser.add_argument("--blocks", type=int, default=4096, help="number of 4096-byte blocks (default: 4096)")
    parser.add_argument("--yes", action="store_true", help="start without waiting for confirmation")
    args = parser.parse_args()
    if not 1 <= args.blocks <= 2**32:
        parser.error("--blocks must be between 1 and 4294967296")
    if len({port.casefold() for port in (args.source, args.slow_port, args.fast)}) != 3:
        parser.error("source, output A and output B must use distinct serial ports")
    if min(args.source_baud, args.slow_baud, args.fast_baud) <= 0:
        parser.error("baud rates must be positive")
    try:
        return run(args)
    except (OSError, EOFError) as error:
        print(f"Test setup failed: {error}")
        print("Check that the three serial ports are available on the test side.")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
