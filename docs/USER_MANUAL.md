# User manual

For everyday use of a bathroom fan switch running this firmware. To flash the
firmware onto a Shelly, see the [installation guide](INSTALL.md).

- [What it does](#what-it-does)
- [The status light](#the-status-light)
- [The button](#the-button)
- [Changing settings (setup mode)](#changing-settings-setup-mode)
- [Adding it to a Hue Bridge](#adding-it-to-a-hue-bridge)
- [Re-adding or moving to another bridge](#re-adding-or-moving-to-another-bridge)
- [Firmware update](#firmware-update)
- [Troubleshooting](#troubleshooting)

## What it does

The switch shows up in the Hue app as a light. It sits in the same Hue room or
zone as the bathroom lamp, so whatever controls that room also controls the fan:
the Hue button, the app, automations, voice assistants.

- **Room on** → the fan starts.
- **Room off** → the fan keeps running for **6 minutes**, then stops.
- **Room on again within those 6 minutes** → the fan simply keeps running.

The Hue app shows the fan as *off* as soon as you switch off, even though it is
still running out its 6 minutes. That is expected.

After a power cut the fan stays off until it is switched on again. Pairing and
settings are kept.

## The status light

In normal operation the LED is simply on or off, following the fan (see the
*Status LED* setting). When something needs attention it repeats a blink code
instead: **short flashes, one long flash, a pause**. Count the short flashes:

| Code | Pattern | Meaning |
|---|---|---|
| **1** | `·  —` | Not connected to a Hue Bridge; waiting to be added. |
| **2** | `·  ·  —` | Setup mode is open. |
| **3** | `·  ·  ·  —` | Re-pairing: the switch is leaving Hue and restarts in a few seconds. |

While you hold [the button](#the-button), codes 2 and 3 show what letting go will do.

Other patterns:

| LED | Meaning |
|---|---|
| Rapid flicker for under a second | Just powered up. |
| Long flashes, no short ones | *Flash LED* was pressed on the setup page. |
| On / off | Normal operation; follows the fan (default setting). |

The codes show even when *Status LED* is set to *Off*.

## The button

The small button on the front of the switch does two things, depending on how
long you hold it. While you hold it, the LED shows the code of what will happen
when you let go:

| Hold | LED while holding | When you let go |
|---|---|---|
| less than 5 seconds | unchanged | nothing |
| 5 to 10 seconds | code 2 (`·  ·  —`) | the switch restarts in [setup mode](#changing-settings-setup-mode) |
| more than 10 seconds | code 3 (`·  ·  ·  —`) | [re-pair](#re-adding-or-moving-to-another-bridge): the switch leaves Hue and waits to be added again |

Once the switch is built in, the button is usually out of reach. Everything it
does can also be done by switching the power off and on (see below).

## Changing settings (setup mode)

Settings are changed from a phone, on a small web page served by the switch
itself. No app or internet is needed.

**1. Open setup mode.** Switch the fan's circuit **off and on three times in
quick succession**, using the switch or circuit breaker that powers it. Leave it
off for a few seconds each time so the switch really powers down, and on for no
more than about 5 seconds. After the third time the LED shows code 2 (`·  ·  —`).
If you can reach [the button](#the-button), holding it for 5 seconds does the
same.

**2. Connect.** On your phone, join the WiFi network **`Shelly-Fan-Setup-XXXX`**
(no password). `XXXX` is a code unique to your switch, so several switches in
setup mode don't get mixed up. The settings page usually opens by itself. If
not, open **192.168.71.1** in a browser.

While your phone is on this network it has no internet and the Hue app doesn't
work. Switch back to your home WiFi when you're done.

<table>
  <tr>
    <td valign="top"><img src="images/portal-1-timing-led.png" alt="Setup page, top: timing and status LED" width="300"></td>
    <td valign="top"><img src="images/portal-2-wiring-firmware.png" alt="Setup page, bottom: wiring, re-pairing, firmware update, identify and test" width="300"></td>
  </tr>
</table>

**3. Change settings and press Save & reboot.** The switch stores the settings
and restarts normally. If you do nothing for 5 minutes, it leaves setup mode by
itself and changes nothing.

| Setting | What it does |
|---|---|
| **Minimum on-time** | Seconds the room must stay on before the fan starts. `0` = immediately. With `60`, a quick visit doesn't start the fan. |
| **Run-on after off** | Seconds the fan keeps running after the room is switched off. Default `360` (6 minutes). |
| **Status LED** | *Off*: dark in normal operation (blink codes still show). *Lamp*: lit while Hue shows the fan as on. *Relay*: lit while the fan actually runs, including the run-on. |
| **Relay output** | *Normal* or *Inverted*. Set once at installation: if the fan runs when Hue says off, choose *Inverted*. |
| **Status LED output** | *Normal* or *Inverted*. The default (*Inverted*) is right for this switch; change it only if the LED is lit when it should be dark. |
| **Forget Hue & re-pair** | Disconnects from the Hue Bridge so the switch can be added again. See [below](#re-adding-or-moving-to-another-bridge). |

**Identify & test**, at the bottom of the page, has two buttons that act
immediately and change no settings:

- **Flash LED**: this switch's LED gives long flashes for 10 seconds, so you can
  see which switch you're connected to.
- **Relay**: switches the fan on and off directly, for testing. It starts off
  each time setup mode opens.

## Adding it to a Hue Bridge

1. In the Hue app: **Settings → Add device → Add light → Search**.
2. While the search runs, the switch's LED should show code 1 (`·  —`). It joins
   within a few seconds and appears as a new light.
3. Give it a name (e.g. *Bathroom fan*) and put it in the **same room or zone as
   the bathroom lamp**. Check that your Hue button controls that room or zone.

## Re-adding or moving to another bridge

Open setup mode, turn on **Forget Hue & re-pair**, press **Save & reboot**. A few
seconds after the restart the switch shows code 3 (`·  ·  ·  —`), disconnects
from Hue, restarts again and shows code 1 (`·  —`). Holding
[the button](#the-button) for more than 10 seconds does the same. Then add it as described above, on the same bridge or a
new one.

The switch comes back as a new light, because a Hue Bridge won't take back a
light it has seen before. The old entry in the Hue app becomes unreachable;
delete it.

## Firmware update

The setup page can install a new firmware file (`.bin`): choose the file, press
**Upload & flash**, and **don't switch off the power** until the switch has
restarted. Settings and the Hue pairing are kept.

Only use files from this project's
[releases page](https://github.com/weese/shelly-hue-fan/releases) or from
someone you trust. The upload takes about 10 seconds.

## Troubleshooting

| Problem | What to try |
|---|---|
| The fan keeps running after switching off | Normal for 6 minutes (or whatever *Run-on after off* is set to). |
| The fan does nothing | Check the circuit breaker. Code 1 (`·  —`) means the switch isn't connected to Hue. |
| The fan runs when Hue says off | In setup mode, switch *Relay output* to the other setting. |
| Hue shows the fan as unreachable | The switch has no power, or it is too far from the nearest Hue lamp or the bridge. |
| Setup mode opened by accident | It opens when the power comes on three times in a row for less than 6 seconds each. Press *Save & reboot*, or wait 5 minutes. |
| `Shelly-Fan-Setup-…` doesn't appear | The off/on cycles were too slow or too short. Try again: off a few seconds, on briefly, three times. |
| The settings page doesn't open | Check the phone is connected to `Shelly-Fan-Setup-…`, then open `192.168.71.1`. |
| Several switches are in setup mode | Each has its own network name. On the page, **Flash LED** shows which switch you're on. |
| Hue can't find the switch | The LED must show code 1 (`·  —`) while the app searches. If it was added to this bridge before, use *Forget Hue & re-pair* first. |

The switch is connected to mains. Don't open the cover or the wall box yourself;
leave changes to the wiring to an electrician.
