# Technical reference

Hardware details, the serial console and how the firmware works. For flashing
see the [installation guide](INSTALL.md), for everyday use the
[user manual](USER_MANUAL.md).

- [Hardware](#hardware)
- [Serial console](#serial-console)
- [Zigbee address and re-pairing](#zigbee-address-and-re-pairing)
- [Behaviour notes](#behaviour-notes)
- [Code layout](#code-layout)
- [Development notes](#development-notes)

## Hardware

- **MCU:** ESP32-C6, 8 MB flash.

| Function | GPIO | Used here |
|---|---|---|
| Relay | 4 | drives the fan |
| Status LED | 11 (inverted, LOW = lit) | status indicator |
| Onboard button | 1 | huesmith reset pin at boot |
| Switch input SW | 10 | not used |
| NTC | 3 | not used |
| BL0942 power meter | 6 / 7 | not used |
| UART0 TX / RX | 16 / 17 | console and flashing |

The ESPHome device page lists the LED on GPIO0. That is wrong for this board:
toggling GPIO0 does nothing visible, GPIO11 drives the LED.

Settings are stored in NVS, so they survive firmware updates. Their defaults are
in `Config::default` in `src/config.rs`; fixed pins and the number of power cycles
for setup mode are constants at the top of `src/main.rs`.

## Serial console

The firmware runs a command console on UART0 at 115200 baud. Replies start with
`@@` so they stand out from the log.

```bash
python3 tools/console.py -p "$PORT" status
```

`tools/console.py [-p PORT] [-t SECONDS] [command ...]` sends each command and
prints everything received for the given time (default 4 s). With no command it
just listens.

| Command | Effect |
|---|---|
| `help` | list commands |
| `status` | device id, pairing state, PAN ID, Zigbee address, all settings |
| `reboot` | restart |
| `dfu` | reboot into the ROM bootloader for flashing |
| `repair` | move to a new Zigbee address, leave the network, erase the pairing, reboot |
| `set min_on <s>` | seconds the lamp must stay on before the relay closes |
| `set run_on <s>` | seconds the relay stays closed after the lamp goes off |
| `set led <0\|1\|2>` | LED: 0 off, 1 follows the lamp, 2 follows the relay |
| `set relay_ah <0\|1>` | relay polarity (1 = active-high) |
| `set led_ah <0\|1>` | LED polarity (1 = active-high) |
| `set mac <n>` | choose the Zigbee address offset manually (see below) |
| `aps <0\|1>` | log every incoming Zigbee APS frame (currently on by default) |
| `loglevel <0-5>` | ESP-IDF log level (3 = info, 4 = debug) |

`set` stores the value; it takes effect after `reboot`.

**macOS:** `stty -f PORT 115200` followed by `cat PORT` does not work. The
setting is dropped when `stty` exits and `cat` reads at 9600 baud, which shows
garbage. `tools/console.py` sets the speed on the descriptor it reads from.

The RX pin has its pull-up enabled, so an unconnected RX can't inject characters
once the switch is installed.

## Zigbee address and re-pairing

After a light is deleted in the Hue app, the bridge still lets that device join
its network but never interviews it again, so it never reappears as a light.
Re-pairing therefore always moves the switch to a new IEEE address, chosen by an
offset stored in NVS:

| Offset | Address |
|---|---|
| 0 | the chip's hardware address |
| 1–3 | the chip's other three assigned addresses (ESP32-C6 chips are allocated four consecutive ones) |
| 4–255 | locally administered addresses (U/L bit set) built from this chip's own bytes and the offset, so they can't collide with any manufacturer-assigned address |

`repair` (and *Forget Hue & re-pair* in the setup page) moves to the next offset
and never reuses one; after 255 it wraps to 4. `set mac <n>` picks an offset by
hand. The code is in `identity_eui64` in `src/main.rs`.

The `XXXX` in the setup network name `Shelly-Fan-Setup-XXXX` is the last four
hex digits of the chip's WiFi MAC address.

## Behaviour notes

- **Zigbee OTA frames are dropped.** huesmith advertises the Zigbee OTA Upgrade
  client cluster without implementing it. The Hue Bridge sends an OTA command
  right after pairing, the Zigbee stack then asserts
  (`zcl/zcl_ota_upgrade_commands.c:94`) and the switch reboots. `aps_ind_cb` in
  `src/main.rs` consumes all frames for cluster `0x0019` before the stack sees
  them. This is unrelated to the firmware upload in the setup page.
- **Off has a short delay.** huesmith fades the light out like a real Hue lamp
  and reports "off" about half a second after the command. "On" is immediate.
- **Generic identity.** The switch presents itself as `huesmith SHELLY-FAN-1`.
  huesmith warns that posing as a genuine Philips model can make the bridge push
  real firmware to it.
- **Setup mode and Zigbee never run together.** The setup portal is a separate
  boot mode.
- **The setup page is served offline.** All CSS and JavaScript are inline; a
  captive-portal DNS answers every lookup with the switch's own address.

## Code layout

| File | Contents |
|---|---|
| `src/main.rs` | pins, relay/LED state machine, serial console, Zigbee frame hook, address rotation, boot-mode selection |
| `src/config.rs` | settings stored in NVS, power-cycle counter |
| `src/portal.rs` | WiFi access point, captive DNS, web page, firmware upload |
| `tools/console.py` | serial console client |
| `tools/backup_flash.sh` | chunked flash backup |
| `partitions.csv` | two firmware slots (`ota_0`, `ota_1`) plus Zigbee storage |
| `sdkconfig.defaults` | ESP-IDF configuration |

## Development notes

- GitHub Actions (`.github/workflows/firmware.yml`) builds the firmware on every
  push and pull request and uploads two images as a build artifact:
  `…-full.bin` (complete 8 MB flash image, written at `0x0` on a stock device)
  and `…-app.bin` (for the firmware update in the setup page), plus
  `SHA256SUMS`. Pushing a tag `v*` additionally creates a **draft** release with
  these files.
- A change to `sdkconfig.defaults` triggers a full ESP-IDF rebuild (about two
  minutes); other builds take seconds.
- Changing `partitions.csv` moves the Zigbee storage and therefore erases the
  pairing.
