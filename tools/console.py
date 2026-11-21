#!/usr/bin/env python3
"""Talk to the fan controller's serial console (115200 8N1).

    tools/console.py [-p PORT] [-t SECONDS] [command ...]

Sends each command (newline-terminated), then prints everything received for
SECONDS. Uses only the standard library. Note: on macOS, `stty -f PORT 115200`
followed by `cat PORT` does NOT work — tty settings are dropped when stty exits,
so the port must be configured on the same open file descriptor, as done here.
"""
import argparse
import os
import sys
import termios
import time


def open_port(path, baud=termios.B115200):
    fd = os.open(path, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    attrs = termios.tcgetattr(fd)
    attrs[0] = 0  # iflag: raw
    attrs[1] = 0  # oflag: raw
    attrs[2] = termios.CS8 | termios.CREAD | termios.CLOCAL  # cflag: 8N1, no modem ctl
    attrs[3] = 0  # lflag: raw
    attrs[4] = baud
    attrs[5] = baud
    attrs[6][termios.VMIN] = 0
    attrs[6][termios.VTIME] = 0
    termios.tcsetattr(fd, termios.TCSANOW, attrs)
    termios.tcflush(fd, termios.TCIOFLUSH)
    return fd


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-p", "--port", default="/dev/cu.usbserial-210")
    ap.add_argument("-t", "--time", type=float, default=4.0, help="seconds to listen")
    ap.add_argument("-g", "--gap", type=float, default=0.6, help="seconds between commands")
    ap.add_argument("commands", nargs="*")
    args = ap.parse_args()

    fd = open_port(args.port)
    buf = bytearray()

    def pump(seconds):
        end = time.time() + seconds
        while time.time() < end:
            try:
                chunk = os.read(fd, 4096)
            except BlockingIOError:
                chunk = b""
            if chunk:
                buf.extend(chunk)
            else:
                time.sleep(0.02)

    pump(0.3)
    for cmd in args.commands:
        os.write(fd, cmd.encode() + b"\r\n")
        pump(args.gap)
    pump(args.time)
    os.close(fd)

    text = buf.decode("utf-8", errors="replace")
    sys.stdout.write(text)
    if not text.endswith("\n"):
        sys.stdout.write("\n")
    sys.stderr.write(f"[{len(buf)} bytes received]\n")


if __name__ == "__main__":
    main()
