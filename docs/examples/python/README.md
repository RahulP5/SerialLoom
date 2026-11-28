# Python test scripts

Small scripts that show what SerialLoom delivers, so you can check a setup before pointing your own software at it. Each one stands alone; copy and adapt them freely.

| Script | Checks |
|---|---|
| [`pair_test.py`](pair_test.py) | A virtual null-modem pair carries every byte, both ways |
| [`nmea0183_receive.py`](nmea0183_receive.py) | NMEA 0183 sentences arriving on a COM port, such as the GPS simulator's |
| [`nmea2000_receive.py`](nmea2000_receive.py) | NMEA 2000 messages in Actisense format arriving on a COM port |
| [`udp_receive.py`](udp_receive.py) | Datagrams, such as NMEA sentences, that SerialLoom sends over UDP |

They need Python 3.8 or later. The three COM port scripts also use the `pyserial` package; `udp_receive.py` uses only Python's standard library.

## Running a script

1. Set up the connection in SerialLoom, as described in the script's section below, and make sure it is running.
2. Open Command Prompt or PowerShell in this folder (`docs\examples\python`).
3. Run the script with `python`, followed by the script name and the COM port, for example:

   ```
   python nmea0183_receive.py COM15
   ```

4. The receiving scripts run until you press **Ctrl+C**, then print a summary of what arrived. To stop after a set time instead, add `--seconds 10`.

Run any script with `--help` to see all its options. A COM port can be open in only one program at a time, so close other programs that use the port first.

## pair_test.py: check a virtual pair

**In SerialLoom:** on the Topology page, click **Virtual pair**, enter `COM20` and `COM21`, and click **Create pair**.

**Run:**

```
python pair_test.py COM20 COM21
python pair_test.py COM20 COM21 --bytes 1000000 --rounds 5
```

It writes random data to one port, reads it from the other and compares every byte, then does the same the other way. It prints `PASS` and exits with code 0 when everything matched, so it also works in a test bench.

```
round 1  COM20 -> COM21  65536 bytes  10.2 MB/s  ok
round 1  COM21 -> COM20  65536 bytes  12.6 MB/s  ok
...
PASS
```

## nmea0183_receive.py: NMEA 0183 GPS sentences

**In SerialLoom:** on the Topology page, click **NMEA 0183 simulator** at the top of the Ports list. The New connection window opens with the simulator as the source and two outputs. Set **Output 1** to **New virtual port** `COM15`, remove **Output 2** (or keep it to feed a second program), and click **Create split**. To send more sentence types, select the connection, tick them in the GPS panel and click **Apply**.

**Run:**

```
python nmea0183_receive.py COM15
python nmea0183_receive.py COM15 --type RMC --type GGA   # only these sentences
python nmea0183_receive.py COM15 --raw                   # also show each sentence as received
```

It checks every checksum and decodes RMC, GGA, GLL, GSA, GSV, VTG, ZDA and HDT. Other sentences are shown as they are.

```
23:20:41.347  GPRMC  valid, 48.858400 N, 2.294535 E, 5.00 kn, course 90.00°, 2026-11-28 17:50:41.34 UTC, variation 1.5° W, autonomous
23:20:41.349  GPGGA  48.858400 N, 2.294535 E, fix GPS, 8 satellites, HDOP 1.0, altitude 35.0 m, 17:50:41.34 UTC
23:20:41.351  GPGSA  3D, 8 used (02 05 07 09 12 13 15 17), PDOP 1.8, HDOP 1.0, VDOP 1.5
23:20:41.358  GPVTG  course 90.00° true, 91.50° magnetic, speed 5.00 kn = 9.26 km/h, autonomous
```

## nmea2000_receive.py: NMEA 2000 messages

**In SerialLoom:** as above, but click **NMEA 2000 simulator** and set **Output 1** to **New virtual port** `COM16`.

**Run:**

```
python nmea2000_receive.py COM16
python nmea2000_receive.py COM16 --pgn 129025   # only positions
python nmea2000_receive.py COM16 --raw          # also show each frame in hex
```

It checks each Actisense frame and decodes PGNs 126992 (system time), 129025 (position), 129026 (course and speed), 129539 (DOPs) and 127250 (heading). It also works with Actisense NGT-1 style gateways; use `--baud` to set their speed (default 115200).

```
23:21:06.752  PGN 126992 src 0   prio 3  System time: 2026-11-28 17:51:06.7202 UTC (GPS)
23:21:06.752  PGN 129025 src 0   prio 2  Position: 48.8584000 N, 2.2945352 E
23:21:06.752  PGN 129026 src 0   prio 2  Course 90.00° true, speed 5.00 kn
23:21:06.752  PGN 129539 src 0   prio 6  DOPs: fix 3D (wanted auto), HDOP 1.00, VDOP 1.50, TDOP n/a
23:21:06.752  PGN 127250 src 0   prio 2  Heading 90.00° true, deviation n/a, variation n/a
```

## udp_receive.py: NMEA over UDP

**In SerialLoom:** click **NMEA 0183 simulator** at the top of the Ports list. In the New connection window, remove **Output 2** and set **Output 1** to **UDP**:

- **Local IP and port:** `127.0.0.1:10110`
- **Receive:** off
- **Destination IP and port:** `127.0.0.1:10111`
- **Output framing:** NMEA 0183, so each sentence is sent as one datagram

Then click **Create split**.

**Run:**

```
python udp_receive.py                          # listens on 127.0.0.1:10111
python udp_receive.py --listen 0.0.0.0:10111   # also from other PCs
```

To receive on another PC, run the script there with `--listen 0.0.0.0:10111`, and set SerialLoom's destination to that PC's IP address and port 10111.

```
23:23:22.414  from 127.0.0.1:10110  70 B  $GPRMC,175322.41,A,4851.5040,N,00217.6721,E,5.00,90.00,281126,,,A*66
23:23:22.415  from 127.0.0.1:10110  70 B  $GPGGA,175322.41,4851.5040,N,00217.6721,E,1,08,1.0,35.0,M,0.0,M,,*69
23:23:22.415  from 127.0.0.1:10110  38 B  $GPVTG,90.00,T,,M,5.00,N,9.26,K,A*0C
```
