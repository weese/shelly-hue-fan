# Installation guide

This takes you from a stock Shelly 1PM Gen4 to a switch paired with your Hue
Bridge. Plan for an hour or two the first time; much of it is waiting for the
first build.

For using the switch once it's installed, see the [user manual](USER_MANUAL.md).

1. [Install the toolchain](#1-install-the-toolchain)
2. [Get the code and build](#2-get-the-code-and-build)
3. [Wire the adapter](#3-wire-the-adapter)
4. [Back up the stock firmware](#4-back-up-the-stock-firmware)
5. [Flash](#5-flash)
6. [Pair with Hue](#6-pair-with-hue)
7. [Test on mains, then install](#7-test-on-mains-then-install)

Later: [updating the firmware](#updating-the-firmware-later),
[restoring stock firmware](#restoring-stock-firmware).

There is also an [experimental way without opening the
switch](#from-shellys-web-ui-experimental), through Shelly's own web UI. It
hasn't been tested on a device yet.

## What you need

- **Shelly 1PM Gen4**, model S4SW-001P16EU. Other Shelly models use other pins.
- **USB-to-UART adapter with 3.3 V logic and a 3.3 V supply pin**, for example a
  CP2102 or CH340 board with a 3.3 V/5 V jumper.
- **Five jumper wires that fit a 1.27 mm header** (finer than the usual
  2.54 mm), or a 1.27 mm pin header or pogo pins.
- **A computer** with macOS or Linux. Everything here was done on macOS.
- **A Philips Hue Bridge.**
- Optional: a 470–1000 µF capacitor, to try the WiFi setup mode on the bench
  (see [Bench power](#bench-power)).

## Safety

- **The Shelly must be disconnected from mains whenever anything is connected to
  its header.** The header is not isolated from the mains side.
- On the bench, power it only from the adapter's **3.3 V** pin. 5 V can destroy
  the ESP32-C6.
- The relay does not switch on 3.3 V, so relay behaviour can only be checked on
  mains, with the adapter disconnected.
- Mains wiring in a bathroom may be subject to local regulations. If in doubt,
  have an electrician install it.

## 1. Install the toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

```bash
cargo install ldproxy espflash cargo-espflash
```

```bash
pip3 install esptool
```

- The Rust nightly version is pinned in `rust-toolchain.toml` and installed
  automatically on the first build. Later nightlies failed to build `std` for
  this target (see the comment in that file).
- ESP-IDF v5.2.2 is pinned in `.cargo/config.toml` and is downloaded and built
  on the first build.
- Linux: install `libudev-dev` and `pkg-config` first.
- If Rust is also installed another way (e.g. Homebrew), make sure rustup's
  `~/.cargo/bin` comes first in your `PATH`. A stable `cargo` fails with
  "can't find crate for `core`".

## 2. Get the code and build

```bash
git clone https://github.com/weese/shelly-hue-fan.git
```

```bash
cd shelly-hue-fan
```

```bash
cargo build --release
```

The first build downloads and compiles ESP-IDF and takes a long while. Later
builds take seconds.

**Or use a prebuilt image** from the
[releases page](https://github.com/weese/shelly-hue-fan/releases): you then only
need `esptool` from step 1. In step 5, write `shelly-hue-fan-<version>-full.bin`
instead of running `cargo espflash`:

```bash
esptool --chip esp32c6 --port "$PORT" --baud 115200 --before no-reset --after no-reset write-flash 0x0 shelly-hue-fan-<version>-full.bin
```

The full image covers the whole 8 MB flash, so this also erases everything the
stock firmware left behind; take the backup in step 4 first.

## 3. Wire the adapter

![Wiring the Shelly header to a USB-UART adapter](images/wiring.svg)

| Shelly header pin | Signal | Connect to |
|---|---|---|
| 1 | GND | adapter GND |
| 2 | Boot strap | bridge to pin 1, **first flash only** |
| 3 | EN | nothing |
| 4 | 3V3 | adapter 3.3 V |
| 5 | RX | adapter **TX** |
| 6 | TX | adapter **RX** |
| 7 | — | nothing |

The header is the row of seven holes on the back of the Shelly, at the lower
left. Pin 1 is the hole at the lower end of the row:

<img src="images/shelly-1pm-gen4-backside.png" alt="Back of the Shelly 1PM Gen4 with the 7-pin header at the lower left; pin 1 is the lowest hole" width="400">

Find pin 1 on your board before you connect power.

Plug in the adapter and find its serial port:

```bash
ls /dev/cu.usbserial* /dev/ttyUSB* 2>/dev/null
```

The commands below use `$PORT`. Set it to the port you found, for example:

```bash
export PORT=/dev/cu.usbserial-210
```

## 4. Back up the stock firmware

This backup is your only way back to Shelly's own firmware. With pins 1 and 2
bridged, unplug and replug the adapter so the chip starts in download mode, then:

```bash
tools/backup_flash.sh "$PORT" shelly-1pm-gen4-stock-backup.bin
```

It reads the 8 MB flash in small chunks and retries failed ones, because a single
large read kept aborting over jumper wires. Keep the file private: it contains
your Shelly's device-specific data, and your WiFi password if you ever set the
Shelly up with the Shelly app. `.gitignore` excludes `*.bin` so it can't be
committed by accident.

## 5. Flash

```bash
cargo espflash flash --release --port "$PORT" --before no-reset --after no-reset --baud 115200 --partition-table partitions.csv --erase-parts otadata
```

Then unplug the adapter, **remove the bridge between pins 1 and 2**, and plug it
in again. The LED flashes three times quickly, then repeats a single flash with
a long pause (code 1): the firmware is running and waiting to be added to a Hue
Bridge. All
codes are listed in the [user manual](USER_MANUAL.md#the-status-light).

You can check it over the [serial console](REFERENCE.md#serial-console):

```bash
python3 tools/console.py -p "$PORT" status
```

`factory_new=1 joined=0` is expected at this point.

`--before no-reset --after no-reset` is needed because the header has no reset
lines for the adapter. `--erase-parts otadata` clears the record of which
firmware slot to start, so the chip starts the slot that was just written (see
[updating the firmware later](#updating-the-firmware-later)).

## 6. Pair with Hue

Follow [Adding it to a Hue Bridge](USER_MANUAL.md#adding-it-to-a-hue-bridge) in
the user manual. This works on the bench; the adapter powers the Zigbee radio
fine.

## 7. Test on mains, then install

Disconnect the adapter. Wire the switch like this, in the test setup and later
in the wall box:

![Wiring the Shelly and the fan to mains](images/wiring-mains.svg)

- The Shelly needs **permanent** power. If the fan used to hang off the light
  switch, take L from before that switch; otherwise the fan can't run on and
  Hue loses the switch whenever the light switch is off.
- The terminal order and wire sizes are from Shelly's own manual for the
  1PM Gen4. A fan with its own run-on timer or humidity sensor may have extra
  terminals; check its manual.

In a safe test setup, not yet in the wall box:

1. Hue on → the relay clicks and the fan runs.
2. Hue off → the fan keeps running for the run-on time, then stops.
3. If the fan runs when Hue says off, the relay polarity is inverted. Set
   *Relay output* to *Inverted* in
   [setup mode](USER_MANUAL.md#changing-settings-setup-mode).
4. Open setup mode once and confirm your phone can connect. On the bench,
   holding the button for 5 seconds is quicker than power cycling.

Then have it installed, and hand the [user manual](USER_MANUAL.md) to whoever
uses the bathroom.

## Bench power

Setup mode uses WiFi, which needs more power than a USB-UART adapter supplies:
on the bench the network appears, but the switch resets as soon as a phone
connects. On mains this isn't an issue. On the bench, a 470–1000 µF capacitor
across 3V3 and GND helps. Zigbee pairing works on adapter power alone.

## Updating the firmware later

No bridge between pins 1 and 2 is needed anymore. The firmware reboots into
download mode on a console command:

```bash
python3 tools/console.py -p "$PORT" dfu
```

```bash
cargo espflash flash --release --port "$PORT" --before no-reset --after no-reset --baud 115200 --partition-table partitions.csv --erase-parts otadata
```

Then **unplug and replug the adapter once**. The chip stays in download mode
across soft resets, and no software way out of it worked.

Flashing over serial keeps the Hue pairing and all settings, as long as
`partitions.csv` is unchanged.

Once installed, the setup page can install a firmware file instead (see
[Firmware update](USER_MANUAL.md#firmware-update)). Build the file with:

```bash
espflash save-image --chip esp32c6 --flash-size 8mb --partition-table partitions.csv target/riscv32imac-esp-espidf/release/shelly-hue-fan firmware.bin
```

The switch has two firmware slots. An upload through the setup page goes to the
slot not currently running and switches to it; a serial flash always writes the
first slot (`ota_0`). That is why the serial flash command erases `otadata`:
without it, a switch that last ran an uploaded firmware from `ota_1` would keep
starting the old firmware there. The console's `status` shows the running slot
as `slot=ota_0` or `slot=ota_1`.

## From Shelly's web UI (experimental)

> **Not tested on a device yet.** If it goes wrong at the wrong moment, the
> switch only comes back with the serial adapter from
> [step 3](#3-wire-the-adapter). There's no stock-firmware backup either, so
> going back to Shelly's firmware later would need an image from another
> switch.

Shelly's own firmware can install a firmware file from its web page. It only
accepts a zip in Shelly's own format with the switch's model code (`S1PMG4`) in
it, and puts the app into one of its own two slots. So the package
`shelly-hue-fan-<version>-stock-installer-S1PMG4.zip` contains a small installer
program, with the firmware embedded. When Shelly's bootloader starts it, the
installer:

1. writes the shelly-hue-fan app into a slot that doesn't overlap itself,
2. writes the shelly-hue-fan partition table and an empty settings area,
3. replaces Shelly's bootloader with the shelly-hue-fan one,
4. erases Shelly's partition table, settings (including your WiFi password) and
   leftover data in the Zigbee storage area, and restarts.

A power cut during steps 1 and 2 is harmless: Shelly's bootloader then starts
the installer again, or falls back to Shelly's firmware. Step 3 takes well under a second; a power cut exactly then
leaves the switch needing a serial flash. Shelly's factory data area
(`shelly`, at `0x7F0000`) is never touched.

The result is the same flash contents as a serial flash. Afterwards the switch
behaves as described in the [user manual](USER_MANUAL.md) and can be updated
through the setup page.

Steps:

1. Update the switch to the latest Shelly firmware (2.0 or later) in the Shelly
   app. Switches from the factory carry two Shelly firmwares (WiFi/Matter and
   Zigbee), and older versions refuse a foreign app in that state.
2. Open the switch's web page (its IP address in a browser), then
   **Settings → Firmware** and install the zip from a file.
3. The switch restarts twice. The LED is lit while the installer runs and
   flashes quickly if it stops with an error. When it's done, the LED shows code 1
   (`·`): continue with [step 6](#6-pair-with-hue). If it flashes quickly,
   switch the power off and on to retry.

The package is built in CI as a separate artifact
(`shelly-hue-fan-<version>-stock-installer`), not yet attached to releases. To
build it locally after `cargo build --release`:

```bash
V=$(git describe --tags --always --dirty)
```

```bash
cargo espflash save-image --release --chip esp32c6 --flash-size 8mb --partition-table partitions.csv shelly-hue-fan-$V-app.bin
```

```bash
cargo espflash save-image --release --chip esp32c6 --flash-size 8mb --partition-table partitions.csv --merge shelly-hue-fan-$V-full.bin
```

```bash
python3 tools/make-stock-installer.py --full shelly-hue-fan-$V-full.bin --app shelly-hue-fan-$V-app.bin --version $V --out shelly-hue-fan-$V-stock-installer-S1PMG4.zip
```

The package format and the fact that Shelly's updater accepts an unsigned
package under the switch's own model code come from
[hencou/shelly_gen4_matter_module](https://github.com/hencou/shelly_gen4_matter_module),
which installs this way on the Shelly 1 Gen4 and 1 Mini Gen4.

## Restoring stock firmware

With the chip in download mode:

```bash
esptool --chip esp32c6 --port "$PORT" --before no-reset --after no-reset write-flash 0x0 shelly-1pm-gen4-stock-backup.bin
```

Not tested.
