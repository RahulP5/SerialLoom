"""Compare a SerialLoom TCP Split with and without a non-reading client."""

import argparse
import hashlib
import socket
import struct
import threading
import time
from contextlib import ExitStack

HOST = "127.0.0.1"
SOURCE_PORT, SLOW_PORT, FAST_PORT = 17000, 17001, 17002
BLOCK_SIZE = 4096


def receive(sock, expected_bytes, result):
    digest = hashlib.sha256()
    count = 0
    reason = None
    try:
        while count < expected_bytes:
            data = sock.recv(min(65536, expected_bytes - count))
            if not data:
                reason = "connection closed before the full stream arrived"
                break
            digest.update(data)
            count += len(data)
    except TimeoutError:
        reason = "no data received for 15 seconds"
    except OSError as error:
        reason = str(error)
    result.update(count=count, digest=digest.hexdigest(), reason=reason)


def run(args):
    expected_bytes = args.blocks * BLOCK_SIZE
    result = {}
    with ExitStack() as sockets:
        fast = sockets.enter_context(socket.create_connection((HOST, FAST_PORT), timeout=5))
        fast.settimeout(15)
        if args.slow:
            slow = sockets.enter_context(socket.socket())
            slow.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4096)
            slow.settimeout(5)
            slow.connect((HOST, SLOW_PORT))
            # Keep this connection open without ever reading from it.

        source = sockets.enter_context(socket.create_connection((HOST, SOURCE_PORT), timeout=10))
        print(f"Mode: {'non-reading client on output A' if args.slow else 'output A disconnected'}")
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
                source.sendall(block)
                sent_digest.update(block)
                sent_blocks += 1
                time.sleep(0.004)
        except TimeoutError:
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
    parser.add_argument("--slow", "--lent", action="store_true", help="connect output A and never read")
    parser.add_argument("--blocks", type=int, default=4096, help="number of 4096-byte blocks (default: 4096)")
    parser.add_argument("--yes", action="store_true", help="start without waiting for confirmation")
    args = parser.parse_args()
    if not 1 <= args.blocks <= 2**32:
        parser.error("--blocks must be between 1 and 4294967296")
    try:
        return run(args)
    except (OSError, EOFError) as error:
        print(f"Test setup failed: {error}")
        print("Check the TCP listeners on ports 17000, 17001 and 17002.")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
