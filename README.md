# Shelly 1PM Gen4 → Hue fan controller (with run-on timer)

Alternative firmware for the **Shelly 1PM Gen4** (ESP32-C6) that makes it appear
to a **Philips Hue Bridge** as a native on/off Zigbee light, and behaves like a
switch with a run-on timer:

- Hue turns it **on** → the relay closes (optionally after a minimum on-time) →
  the fan starts.
- Hue turns it **off** → the relay stays closed for the run-on time (default
  6 minutes), then opens.
- Turning it on again during the run-on cancels the pending stop.

Pair it to your Hue Bridge and put it in the same Hue room or zone as the
bathroom lamp. Your Hue button then controls both.

Built on [huesmith](https://github.com/canack/huesmith) (Rust, ESP32-C6), which
provides the Hue-compatible Zigbee light. This project adds the Shelly hardware
layer, the timer logic, a serial console, and a WiFi setup portal.

## Status

Flashed and running on a real Shelly 1PM Gen4 (S4SW-001P16EU).

| Feature | State |
|---|---|
| Pairs with the Hue Bridge, responds to on/off | Verified |
| Run-on logic (seen in the serial log) | Verified on the bench |
| Status LED on GPIO11, inverted | Verified |
| Serial console and jumper-free flashing | Verified |
| Re-pairing with the same bridge (`repair` + `set mac`) | Verified |
| Crash after pairing (Zigbee OTA frame) | Fixed and verified |
| Setup portal: opens after 3 power cycles, captive page, Save | Worked on the bench before the console was added; not retested since |
| Firmware upload through the portal | Never completed end to end (bench supply too weak for WiFi) |
| Relay switching, run-on timing and relay polarity on mains | Not tested |
| Onboard button reset at boot | Not tested |

## Why custom firmware

- The Hue Bridge only accepts devices that behave like Hue-compatible lights.
  The Shelly's stock Zigbee mode is reported not to join it.
- huesmith implements a Hue-compatible light on the ESP32-C6 that does pair.
- Trade-off: the Shelly app, cloud, and REST API are gone. The device is a
  Zigbee light with a setup portal. Take a full flash backup first if you want a
  way back to stock firmware.

## Safety

- **Disconnect the Shelly from mains before connecting anything to the header.**
  The header is not isolated from the mains side.
- Power it only from the adapter's **3.3 V** line while on the bench. 5 V on the
  header can destroy the ESP32-C6.
- The relay does not switch on 3.3 V bench power. Relay behaviour can only be
  checked on mains.
- Mains wiring in a bathroom may be subject to local regulations. If unsure,
  have an electrician do the install.

## Hardware

- **MCU:** ESP32-C6, 8 MB flash.
- **GPIO map:**

  | Function | GPIO | Used here |
  |---|---|---|
  | Relay | 4 | drives the fan |
  | Status LED | 11 (inverted, LOW = lit) | pairing and state indicator |
  | Onboard button | 1 | huesmith reset pin at boot |
  | Switch input SW | 10 | not used |
  | NTC | 3 | not used |
  | BL0942 power meter | 6 / 7 | not used |
  | UART0 TX / RX | 16 / 17 | console and flashing |

  The ESPHome device page lists the LED on GPIO0. That is wrong for this board;
  toggling GPIO0 does nothing visible and GPIO11 drives the LED.

- **7-pin 1.27 mm header** (same pinout as the Shelly Plus 1PM):

  | Pin | Signal | Connect to |
  |---|---|---|
  | 1 | GND | adapter GND |
  | 2 | Boot strap | GND at power-up to force download mode (first flash only) |
  | 3 | EN | leave open |
  | 4 | 3V3 | adapter 3.3 V |
  | 5 | RX | adapter TX |
  | 6 | TX | adapter RX |
  | 7 | — | leave open |

## Toolchain (one-time)

```bash
rustup component add rust-src --toolchain nightly-2026-10-18
```

```bash
cargo install ldproxy espflash cargo-espflash
```

- The nightly is pinned in `rust-toolchain.toml`; later nightlies failed to build
  `std` for the espidf target (see the comment in that file).
- ESP-IDF v5.2.2 is pinned in `.cargo/config.toml`. The first build downloads and
  builds it, which takes a while. A change to `sdkconfig.defaults` triggers a
  full ESP-IDF rebuild (about two minutes); other builds take seconds.
- `esptool` is needed for the backup and for restoring stock firmware.

## Build

```bash
cargo build --release
```

## Flashing

### First flash (stock firmware → this firmware)

1. Wire the adapter as in the header table. Bridge pin 2 to pin 1 (GND), then
   plug in the adapter. The chip starts in download mode.
2. Back up the stock firmware. This is the only way back to it.

   ```bash
   esptool --chip esp32c6 --port /dev/cu.usbserial-210 --before no-reset --after no-reset read-flash 0x0 0x800000 shelly-1pm-gen4-stock-backup.bin
   ```

   On this setup a single 8 MB read aborted repeatedly with "serial data stream
   stopped". Reading in 64 KB chunks with retries and concatenating them worked.
3. Flash:

   ```bash
   cargo espflash flash --release --port /dev/cu.usbserial-210 --before no-reset --after no-reset --baud 115200 --partition-table partitions.csv
   ```

4. Unplug, remove the bridge between pins 1 and 2, plug in again.

`--before no-reset --after no-reset` is needed because the header has no
DTR/RTS reset lines.

### Later flashes (no jumper)

The firmware can reboot itself into the ROM bootloader on a serial command:

```bash
python3 tools/console.py dfu
```

```bash
cargo espflash flash --release --port /dev/cu.usbserial-210 --before no-reset --after no-reset --baud 115200 --partition-table partitions.csv
```

Then **unplug and replug the adapter once**. The chip keeps its download-mode
latch across soft resets, and no software exit from the bootloader worked.

Flashing over serial keeps the Hue pairing and the settings, as long as
`partitions.csv` is unchanged.

Caveat: a serial flash writes the first firmware slot (`ota_0`). After a
firmware upload through the portal the device boots from `ota_1`, so a serial
flash alone would not take effect. Erase the `otadata` partition as part of the
flash in that case (espflash has `--erase-parts`; this has not been tried here).

### Restoring stock firmware

```bash
esptool --chip esp32c6 --port /dev/cu.usbserial-210 --before no-reset --after no-reset write-flash 0x0 shelly-1pm-gen4-stock-backup.bin
```

Not tested.

## Serial console

The firmware runs a command console on UART0 at 115200 baud. Replies start with
`@@` so they stand out from the log.

```bash
python3 tools/console.py status
```

`tools/console.py [-t SECONDS] [command ...]` sends each command and prints
everything received for the given time. With no command it just listens.

| Command | Effect |
|---|---|
| `help` | list commands |
| `status` | pairing state, PAN ID, and all settings |
| `reboot` | restart |
| `dfu` | reboot into the ROM bootloader for flashing |
| `repair` | Zigbee factory reset: leave the network, erase pairing, reboot |
| `set min_on <s>` | seconds the lamp must stay on before the relay closes |
| `set run_on <s>` | seconds the relay stays closed after the lamp goes off |
| `set led <0\|1\|2>` | LED: 0 off, 1 follows the lamp, 2 follows the relay |
| `set relay_ah <0\|1>` | relay polarity (1 = active-high) |
| `set led_ah <0\|1>` | LED polarity (1 = active-high) |
| `set mac <n>` | add `n` to the last byte of the IEEE address (0 = real address) |
| `aps <0\|1>` | log every incoming Zigbee APS frame (on by default for now) |
| `loglevel <0-5>` | ESP-IDF log level (3 = info, 4 = debug) |

`set` stores the value in NVS; it takes effect after `reboot`.

**macOS note:** `stty -f PORT 115200` followed by `cat PORT` does not work. The
tty settings are dropped when `stty` exits, so `cat` reads at 9600 baud and shows
garbage. `tools/console.py` sets the baud rate on the descriptor it reads from.

The RX pin has its pull-up enabled so that an unconnected RX cannot inject
characters once the device is installed.

## Pairing with the Hue Bridge

1. In the Hue app: *Settings → Add device → Add light → Search*.
2. Power the Shelly. While it is not joined to a network the **LED blinks
   slowly**. Once the search is open it joins within a few seconds.
3. It appears as a new on/off light. Rename it, add it to the bathroom room or
   zone, and make sure your Hue button controls that room or zone.

### Re-pairing and moving to another bridge

`repair` on the console (or the "Forget Hue & re-pair" switch in the setup
portal) factory-resets the Zigbee side. The device then joins the next network
that opens for joining.

**Re-adding to the same bridge needs a new address.** After a light is deleted
in the Hue app, the bridge still lets the device join but does not interview it
again, so it never shows up as a new light. Presenting a different IEEE address
makes the bridge treat it as a new device:

```bash
python3 tools/console.py "set mac 1" repair
```

Use a value you have not used with that bridge before. A different bridge has no
record of the device, so the real address (`set mac 0`) should work there; this
has not been tried.

Holding the onboard button (GPIO1) at power-up is huesmith's built-in reset and
is configured, but it was not tested and the button is unreachable once the
device is installed.

## Setup portal

For changing settings once the device is installed and the serial header is out
of reach.

1. Switch the mains off and on **3 times in quick succession** (each on-time
   shorter than about 6 seconds). On the third boot the LED blinks fast and the
   device opens the WiFi network **`Shelly-Fan-Setup`** (no password). Zigbee is
   not running in this mode.
2. Join that network. It is a captive portal, so the setup page should open by
   itself. If it does not, browse to the network's gateway address
   (`192.168.71.1` on this ESP-IDF version).
3. Change the timing, LED mode, or polarities, or switch on "Forget Hue &
   re-pair", and press **Save & reboot**.
4. *Firmware update* uploads a `.bin` to the inactive firmware slot and reboots
   into it.
5. Without any request for 5 minutes the device reboots into normal mode.

Limits:

- A phone or laptop on this network has no internet and cannot reach the Hue
  Bridge at the same time. For anything that involves the Hue app, the serial
  console is easier.
- On USB-UART bench power the network appears, but the chip browns out and
  resets when a client connects. A 470–1000 µF capacitor across 3V3 and GND, a
  stronger 3.3 V supply, or mains power avoids that.
- The firmware upload has not completed end to end yet.

The `.bin` for an upload can be produced with:

```bash
espflash save-image --chip esp32c6 --flash-size 8mb --partition-table partitions.csv target/riscv32imac-esp-espidf/release/shelly-hue-fan firmware.bin
```

## Behaviour notes

- **Hue shows "off" while the fan runs on.** The run-on happens in the firmware;
  the bridge sees a light that switched off.
- **LED:** three quick blinks at boot; slow blink while not joined; fast blink in
  portal mode; otherwise according to the LED setting.
- **Off has a short delay.** huesmith fades the light out like a real Hue lamp
  and reports "off" to the firmware about half a second after the command
  arrives (visible in the log). "On" is immediate.
- **Power loss:** the device boots with the relay open and keeps its pairing and
  settings.
- **Zigbee OTA frames are dropped.** huesmith advertises the Zigbee OTA Upgrade
  client cluster without a backend. The Hue Bridge sends an OTA command right
  after pairing, and the Zigbee stack then asserts
  (`zcl/zcl_ota_upgrade_commands.c:94`) and the device reboots. `aps_ind_cb` in
  `src/main.rs` consumes all frames for cluster `0x0019` before the stack sees
  them. This is unrelated to the firmware upload in the setup portal.
- **Use a generic identity.** huesmith warns that posing as a genuine Philips
  model can make the bridge push real firmware.

## Checks on mains before the final install

In a safe test setup, not the final box:

1. Hue on → relay closes, fan runs.
2. Hue off → fan keeps running for the run-on time, then stops.
3. If the fan runs when Hue is off, the relay polarity is inverted:
   `set relay_ah 0` on the console, or the switch in the portal.
4. Open the setup portal with 3 power cycles and confirm a phone can connect.

## Code layout

| File | Contents |
|---|---|
| `src/main.rs` | pins, relay/LED state machine, serial console, Zigbee frame hook, boot-mode selection |
| `src/config.rs` | settings stored in NVS, power-cycle counter |
| `src/portal.rs` | WiFi access point, captive DNS, web page, firmware upload |
| `tools/console.py` | serial console client |
| `partitions.csv` | two firmware slots (`ota_0`, `ota_1`) plus Zigbee storage |
| `sdkconfig.defaults` | ESP-IDF configuration |

Setting defaults are in `Config::default` in `src/config.rs`: minimum on-time
0 s, run-on 360 s, LED follows the lamp, relay active-high, LED inverted, real
IEEE address. Fixed pins and the portal trigger count are constants at the top
of `src/main.rs`.

Changing `partitions.csv` moves the Zigbee storage and therefore erases the
pairing.

## Credits

- [huesmith](https://github.com/canack/huesmith) by Engin Açıkgöz.
- Pin and header references: the ESPHome device database, the blakadder Tasmota
  template for the Shelly 1PM Gen4, and Tasmota discussion #24407.
