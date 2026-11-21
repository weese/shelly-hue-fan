# Shelly 1PM Gen4 → Hue fan controller

Alternative firmware for the **Shelly 1PM Gen4** that makes it appear to a
**Philips Hue Bridge** as an ordinary Hue light, and switches a bathroom fan
with a run-on timer:

- The Hue room turns **on** → the fan starts.
- The Hue room turns **off** → the fan keeps running for 6 minutes, then stops.
- On again during those 6 minutes → the fan keeps running.

Put it in the same Hue room or zone as the bathroom lamp, and the Hue button,
app and automations control lamp and fan together. No cloud, no extra app.

## Where to start

| You want to… | Read |
|---|---|
| **Use a switch that's already installed**: change the run-on time, add it to Hue, fix a problem | 📖 **[User manual](docs/USER_MANUAL.md)** |
| **Flash a stock Shelly 1PM Gen4** with this firmware | 🔧 [Installation guide](docs/INSTALL.md) |
| Look up the serial console, pins or how it works | ⚙️ [Technical reference](docs/REFERENCE.md) |

### Quick help for the installed switch

- **Change settings:** switch the fan's power off and on three times quickly.
  The LED blinks fast; join the WiFi `Shelly-Fan-Setup-XXXX` on your phone.
  [Details](docs/USER_MANUAL.md#changing-settings-setup-mode)
- **Add it to Hue:** Hue app → Settings → Add device → Add light → Search.
  [Details](docs/USER_MANUAL.md#adding-it-to-a-hue-bridge)
- **Fan runs on after switching off:** that's the 6-minute run-on.
  [Troubleshooting](docs/USER_MANUAL.md#troubleshooting)

## Status

Running on a real Shelly 1PM Gen4 (S4SW-001P16EU), tested on the bench and on
mains.

| Feature | State |
|---|---|
| Pairs with the Hue Bridge, responds to on/off | Verified |
| Run-on logic (seen in the serial log) | Verified on the bench |
| Status LED | Verified |
| Serial console, flashing without a jumper | Verified |
| Setup mode: opens after 3 power cycles, page loads on a phone | Verified |
| Setup mode: Save & reboot | Verified |
| Setup mode: network name with device id, Flash LED and Relay buttons | Verified |
| Setup mode: Normal/Inverted wiring choices are saved | Verified |
| Re-pairing with the same bridge (new address each time) | Verified |
| Crash right after pairing (Zigbee OTA frame) | Fixed and verified |
| Firmware upload through the setup page (pairing and settings kept) | Verified |
| Serial flash after an upload (`--erase-parts otadata`) | Verified |
| Relay switching, run-on timing and relay polarity on mains | Verified |
| Onboard button reset at boot | Not tested |

## Credits

- Built on [huesmith](https://github.com/canack/huesmith) by Engin Açıkgöz, which
  provides the Hue-compatible Zigbee light. This project adds the Shelly
  hardware layer, the timer logic, a serial console and a WiFi setup page.
- Pin and header references: the ESPHome device database, the blakadder Tasmota
  templates for the [Shelly 1PM Gen4](https://templates.blakadder.com/shelly_1PM_Gen4.html)
  and the [Shelly Plus 1PM](https://templates.blakadder.com/shelly_plus_1PM.html),
  and Tasmota discussion #24407.

## License

MIT, see [LICENSE](LICENSE).
