//! Fan controller firmware for the **Shelly 1PM Gen4** (ESP32-C6).
//!
//! Built on [huesmith](https://github.com/canack/huesmith), which makes the
//! device appear to a **Philips Hue Bridge** as a native on/off Zigbee light.
//! Pair it, drop it in the same Hue room/zone as the bathroom lamp, and your Hue
//! button controls both at once.
//!
//! Behaviour — a *switch with optional delayed-on and a run-on (overshoot) timer*:
//!   * Hue **on**  -> (after an optional minimum on-time) relay closes -> fan on.
//!   * Hue **off** -> relay stays closed for the run-on, then opens -> fan off.
//!   * Turning on again during the run-on cancels the pending stop.
//!
//! All timings/polarities/LED behaviour are **runtime-configurable via the WiFi
//! setup portal** ([`portal`]), entered by rapid power-cycling (no button needed
//! once it's built in). Defaults live in [`config::Config::default`].

mod config;
mod portal;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use huesmith::HueDeviceType;

use config::{Config, LedMode};

// ─────────────────────────── Hardware pins (fixed) ──────────────────────────
// Verified for the Shelly 1PM Gen4 (ESPHome + Tasmota community):
// relay = GPIO4, switch in = GPIO10, button = GPIO1, LED = GPIO11, NTC = GPIO3,
// BL0942 meter on GPIO6/7. GPIO0 is the boot pin (not the LED, despite ESPHome).

const RELAY_GPIO: u8 = 4;
const LED_GPIO: Option<u8> = Some(11);

/// Onboard button. Held 5–10 s and released: restart into the setup portal.
/// Held over 10 s and released: re-pair (new Zigbee address, factory reset).
/// The button drives the pin high when pressed and has no resistor of its
/// own, so it is read with the internal pull-down (verified on the device; with
/// the pull-up, presses can't be seen). Holding it from power-on works too:
/// the hold is counted from when the watcher starts.
pub(crate) const BUTTON_GPIO: Option<u8> = Some(1);

/// Whether the button is pressed (pin high).
pub(crate) fn button_pressed(pin: u8) -> bool {
    get_level(pin) == 1
}
const HOLD_SETUP: Duration = Duration::from_secs(5);
const HOLD_REPAIR: Duration = Duration::from_secs(10);
/// Held longer than this counts as stuck: no action on release.
const HOLD_STUCK: Duration = Duration::from_secs(30);

// ───────────────────────── Power-cycle → portal ─────────────────────────────

/// Rapid power cycles that enter the WiFi setup portal. `None` disables it.
const PORTAL_CYCLES: Option<u8> = Some(3);

/// Stay powered this long and the rapid-power-cycle counter resets — so only
/// quick off/on sequences accumulate toward opening the portal.
const RESET_ARM_WINDOW: Duration = Duration::from_secs(6);

/// Quick flashes at power-on, as an alive indicator. Shorter and faster than
/// the flashes of the blink codes, and shown only once.
const BOOT_FLASHES: u8 = 3;
const BOOT_FLASH_MS: u64 = 120;

// ─────────────────────────────── LED codes ──────────────────────────────────
//
// Status codes repeat N short flashes, then a long pause:
//   1  ·        not joined to a Hue network, waiting to be added
//   2  · ·      setup portal open
//   3  · · ·    re-pairing: leaving Hue, erasing, about to restart
// They show regardless of the "Status LED" setting. Keep docs/USER_MANUAL.md
// in sync when changing them.
pub const CODE_WAITING_FOR_HUE: u8 = 1;
pub const CODE_SETUP_MODE: u8 = 2;
pub const CODE_REPAIRING: u8 = 3;

const CODE_FLASH_MS: u64 = 200;
const CODE_GAP_MS: u64 = 300;
const CODE_PAUSE_MS: u64 = 2000;

/// Whether the LED is lit `t_ms` after blink code `code` started.
pub fn code_lit(code: u8, t_ms: u64) -> bool {
    let flashes = code as u64 * (CODE_FLASH_MS + CODE_GAP_MS);
    let p = t_ms % (flashes + CODE_PAUSE_MS);
    p < flashes && p % (CODE_FLASH_MS + CODE_GAP_MS) < CODE_FLASH_MS
}

/// Code shown instead of the normal LED state (0 = none); set while a
/// re-pair is pending.
static LED_CODE_OVERRIDE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);


// ─────────────────────── Raw GPIO via ESP-IDF (by pin) ───────────────────────
// huesmith calls `Peripherals::take()` in launch(), so we can't take the HAL
// pins; we drive the relay/LED through the C GPIO API by pin number instead.

pub(crate) fn configure_button_input(pin: u8) -> bool {
    let cfg = esp_idf_svc::sys::gpio_config_t {
        pin_bit_mask: 1u64 << pin,
        mode: esp_idf_svc::sys::gpio_mode_t_GPIO_MODE_INPUT,
        pull_up_en: esp_idf_svc::sys::gpio_pullup_t_GPIO_PULLUP_DISABLE,
        pull_down_en: esp_idf_svc::sys::gpio_pulldown_t_GPIO_PULLDOWN_ENABLE,
        intr_type: esp_idf_svc::sys::gpio_int_type_t_GPIO_INTR_DISABLE,
    };
    unsafe { esp_idf_svc::sys::gpio_config(&cfg) == esp_idf_svc::sys::ESP_OK }
}

pub(crate) fn get_level(pin: u8) -> i32 {
    unsafe { esp_idf_svc::sys::gpio_get_level(pin as i32) }
}

fn configure_output(pin: u8) {
    let cfg = esp_idf_svc::sys::gpio_config_t {
        pin_bit_mask: 1u64 << pin,
        mode: esp_idf_svc::sys::gpio_mode_t_GPIO_MODE_OUTPUT,
        pull_up_en: esp_idf_svc::sys::gpio_pullup_t_GPIO_PULLUP_DISABLE,
        pull_down_en: esp_idf_svc::sys::gpio_pulldown_t_GPIO_PULLDOWN_DISABLE,
        intr_type: esp_idf_svc::sys::gpio_int_type_t_GPIO_INTR_DISABLE,
    };
    let err = unsafe { esp_idf_svc::sys::gpio_config(&cfg) };
    if err != esp_idf_svc::sys::ESP_OK {
        log::error!("gpio_config(GPIO{pin}) failed: {err}");
    }
}

fn set_level(pin: u8, level: u32) {
    let err = unsafe { esp_idf_svc::sys::gpio_set_level(pin as i32, level) };
    if err != esp_idf_svc::sys::ESP_OK {
        log::error!("gpio_set_level(GPIO{pin}, {level}) failed: {err}");
    }
}

fn set_relay(active_high: bool, closed: bool) {
    set_level(RELAY_GPIO, if closed == active_high { 1 } else { 0 });
}

fn set_led(cfg: &Config, lamp_on: bool, relay_closed: bool) {
    if let Some(pin) = LED_GPIO {
        let lit = match cfg.led_mode {
            LedMode::Off => false,
            LedMode::Lamp => lamp_on,
            LedMode::Relay => relay_closed,
        };
        set_level(pin, if lit == cfg.led_active_high { 1 } else { 0 });
    }
}

/// Short device id: the last two bytes of the chip's base MAC as four hex digits
/// (e.g. `2314` for `CC:BA:97:EA:23:14`). Stable for the device, used in the
/// setup-mode SSID and shown on the setup page and in `status`.
fn device_id() -> String {
    let mut mac = [0u8; 6];
    unsafe {
        esp_idf_svc::sys::esp_read_mac(
            mac.as_mut_ptr(),
            esp_idf_svc::sys::esp_mac_type_t_ESP_MAC_WIFI_STA,
        );
    }
    format!("{:02X}{:02X}", mac[4], mac[5])
}

/// Zigbee identity (IEEE EUI-64) for an address offset, in the ZBOSS
/// little-endian order huesmith's `.mac()` expects; `None` = the chip's own
/// address. A Hue Bridge does not re-interview an address it has seen before
/// (e.g. after the light was deleted in the app), so every re-pair moves on.
///
/// - `0`: the chip's hardware address.
/// - `1..=3`: the chip's other universally administered addresses. ESP32-C6
///   chips are allocated four consecutive MACs (base .. base+3).
/// - `4..=255`: locally administered addresses (U/L bit set), built from this
///   chip's own bytes plus the offset, so they can't collide with any
///   manufacturer-assigned address.
fn identity_eui64(offset: u8) -> Option<[u8; 8]> {
    if offset == 0 {
        return None;
    }
    // Display order, e.g. CC:BA:97:FF:FE:EA:23:14 (OUI, FF:FE filler, base[3..6]).
    let mut eui = [0u8; 8];
    unsafe {
        esp_idf_svc::sys::esp_read_mac(
            eui.as_mut_ptr(),
            esp_idf_svc::sys::esp_mac_type_t_ESP_MAC_IEEE802154,
        );
    }
    if offset <= 3 {
        eui[7] = eui[7].wrapping_add(offset);
    } else {
        eui[0] |= 0x02; // locally administered; I/G bit stays 0 (unicast)
        eui[4] = offset; // replaces the FE filler byte
    }
    eui.reverse();
    Some(eui)
}

/// Offset for the next re-pair. Never reuses one; after 255 it wraps to 4.
fn next_mac_offset(cur: u8) -> u8 {
    if cur == u8::MAX {
        4
    } else {
        cur + 1
    }
}

/// Move to a fresh Zigbee address, then factory-reset: leave the network, erase
/// the pairing, reboot. The device comes back factory-new under the new address,
/// so the bridge treats it as a new light even if it knew the old one.
fn rotate_identity_and_factory_reset(nvs: &EspDefaultNvsPartition) {
    let mut c = Config::load(nvs);
    let old = c.mac_offset;
    c.mac_offset = next_mac_offset(old);
    match c.save(nvs) {
        Ok(()) => console_print(&format!(
            "re-pair: address offset {old} -> {}; factory reset (leave + erase + reboot)",
            c.mac_offset
        )),
        Err(e) => console_print(&format!(
            "re-pair: could not save new address ({e}), keeping offset {old}; factory reset"
        )),
    }
    std::thread::sleep(Duration::from_millis(200));
    unsafe {
        let locked = esp_zb_lock_acquire(u32::MAX);
        esp_zb_factory_reset();
        if locked {
            esp_zb_lock_release();
        }
    }
}

/// Re-pair requested from the setup portal: wait until huesmith has the stack
/// up, then rotate the address and factory-reset.
fn schedule_zigbee_factory_reset(nvs: EspDefaultNvsPartition) {
    std::thread::Builder::new()
        .name("factoryreset".into())
        .stack_size(4 * 1024)
        .spawn(move || {
            LED_CODE_OVERRIDE.store(CODE_REPAIRING, Ordering::Relaxed);
            std::thread::sleep(Duration::from_secs(5));
            log::warn!("re-pair requested");
            rotate_identity_and_factory_reset(&nvs);
        })
        .ok();
}

// ──────────────────────────── Onboard button ────────────────────────────────

/// What a button hold has reached so far.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Hold {
    Short,
    Setup,
    Repair,
    Stuck,
}

fn hold_stage(held: Duration) -> Hold {
    if held >= HOLD_STUCK {
        Hold::Stuck
    } else if held >= HOLD_REPAIR {
        Hold::Repair
    } else if held >= HOLD_SETUP {
        Hold::Setup
    } else {
        Hold::Short
    }
}

/// Watch the onboard button while running normally. The LED shows the code of
/// the action a release would trigger; the action runs on release.
fn spawn_button_watcher(pin: u8, nvs: EspDefaultNvsPartition) {
    const POLL: Duration = Duration::from_millis(20);
    const DEBOUNCE: Duration = Duration::from_millis(60);
    std::thread::Builder::new()
        .name("button".into())
        .stack_size(4 * 1024)
        .spawn(move || {
            if !configure_button_input(pin) {
                log::error!("button: cannot configure GPIO{pin}");
                return;
            }
            std::thread::sleep(Duration::from_millis(5)); // let the pull-down settle
            loop {
                std::thread::sleep(POLL);
                if !button_pressed(pin) {
                    continue;
                }
                // Pressed: track the hold until the button reads released
                // for DEBOUNCE, showing the pending action's code meanwhile.
                let pressed_at = Instant::now();
                let mut released_since: Option<Instant> = None;
                let mut stage = Hold::Short;
                loop {
                    std::thread::sleep(POLL);
                    if !button_pressed(pin) {
                        let since = *released_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= DEBOUNCE {
                            break;
                        }
                        continue;
                    }
                    released_since = None;
                    let now = hold_stage(pressed_at.elapsed());
                    if now != stage {
                        stage = now;
                        let code = match stage {
                            Hold::Setup => CODE_SETUP_MODE,
                            Hold::Repair => CODE_REPAIRING,
                            Hold::Short | Hold::Stuck => 0,
                        };
                        LED_CODE_OVERRIDE.store(code, Ordering::Relaxed);
                        if stage == Hold::Stuck {
                            log::warn!("button: held over {}s, ignoring", HOLD_STUCK.as_secs());
                        }
                    }
                }
                match stage {
                    Hold::Setup => {
                        console_print("button: restarting into setup mode");
                        config::boot_count_force_portal(&nvs);
                        std::thread::sleep(Duration::from_millis(200));
                        unsafe { esp_idf_svc::sys::esp_restart() };
                    }
                    Hold::Repair => {
                        console_print("button: re-pair");
                        rotate_identity_and_factory_reset(&nvs);
                    }
                    Hold::Short | Hold::Stuck => {
                        LED_CODE_OVERRIDE.store(0, Ordering::Relaxed);
                    }
                }
            }
        })
        .ok();
}

// ─────────── Serial "reboot to download mode" (jump to bootloader) ───────────
//
// Lets the device be reflashed over UART with no boot jumper: send a magic token
// on the serial line and the firmware sets the ESP32-C6 FORCE_DOWNLOAD_BOOT bit
// and resets, so the ROM comes up in UART download mode. (For field updates use
// OTA in the portal; this is a dev-loop convenience.)

/// LP_AON_SYS_CFG_REG (0x600B_10B4); FORCE_DOWNLOAD_BOOT is bit 30.
const LP_AON_SYS_CFG_REG: u32 = 1_611_337_780;
const FORCE_DOWNLOAD_BOOT_MASK: u32 = 1 << 30;

/// UART0 RX pin (the debug-header "RX"), same pin the console uses.
const UART0_RX_GPIO: u8 = 17;

/// Clear a stale force-download-boot bit on a normal boot, so we never get stuck
/// re-entering download mode. (LP_AON survives resets; only power-off clears it.)
fn clear_force_download_boot() {
    unsafe {
        let reg = LP_AON_SYS_CFG_REG as *mut u32;
        core::ptr::write_volatile(reg, core::ptr::read_volatile(reg) & !FORCE_DOWNLOAD_BOOT_MASK);
    }
}

/// Set FORCE_DOWNLOAD_BOOT and software-reset → ROM enters UART download mode.
fn reboot_to_download() -> ! {
    unsafe {
        let reg = LP_AON_SYS_CFG_REG as *mut u32;
        core::ptr::write_volatile(reg, core::ptr::read_volatile(reg) | FORCE_DOWNLOAD_BOOT_MASK);
        esp_idf_svc::sys::esp_rom_software_reset_system();
    }
    loop {}
}

/// Write a line to UART0 TX, prefixed with `@@` so it's easy to pick out of a
/// noisy serial capture.
fn console_print(s: &str) {
    let line = format!("@@ {s}\r\n");
    unsafe {
        esp_idf_svc::sys::uart_write_bytes(
            0u32,
            line.as_ptr() as *const core::ffi::c_void,
            line.len(),
        );
    }
}

/// Label of the firmware slot this image runs from (`ota_0` or `ota_1`).
fn running_slot() -> String {
    let part = unsafe { esp_idf_svc::sys::esp_ota_get_running_partition() };
    if part.is_null() {
        return "?".into();
    }
    let label = unsafe { std::ffi::CStr::from_ptr((*part).label.as_ptr()) };
    label.to_string_lossy().into_owned()
}

/// Handle one serial console command line.
fn handle_command(cmd: &str, nvs: &EspDefaultNvsPartition) {
    let mut it = cmd.split_whitespace();
    match it.next() {
        Some("help") => console_print(
            "cmds: status | reboot | dfu | repair (new address + factory reset) | aps <0|1> | loglevel <0-5> | set <min_on|run_on|led|relay_ah|led_ah|mac> <n>",
        ),
        Some("status") => {
            let c = Config::load(nvs);
            let joined = unsafe { esp_zb_bdb_dev_joined() } as u8;
            let fnew = unsafe { esp_zb_bdb_is_factory_new() } as u8;
            let pan = unsafe { esp_zb_get_pan_id() };
            let mut ieee = [0u8; 8];
            unsafe { esp_zb_get_long_address(ieee.as_mut_ptr()) };
            let ieee = ieee
                .iter()
                .rev()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(":");
            console_print(&format!(
                "status fw={} id={} slot={} btn={} factory_new={fnew} joined={joined} pan=0x{pan:04X} ieee={ieee} mac_off={} min_on={} run_on={} led={} relay_ah={} led_ah={}",
                env!("FW_VERSION"),
                device_id(),
                running_slot(),
                BUTTON_GPIO.map_or(-1, get_level),
                c.mac_offset,
                c.min_on_secs,
                c.run_on_secs,
                c.led_mode.as_u8(),
                c.relay_active_high as u8,
                c.led_active_high as u8
            ));
        }
        Some("reboot") => {
            console_print("rebooting");
            std::thread::sleep(Duration::from_millis(200));
            unsafe { esp_idf_svc::sys::esp_restart() };
        }
        Some("dfu") => {
            console_print("entering download mode");
            std::thread::sleep(Duration::from_millis(200));
            reboot_to_download();
        }
        Some("repair") => rotate_identity_and_factory_reset(nvs),
        Some("set") => {
            let key = it.next();
            let val = it.next().and_then(|v| v.parse::<u32>().ok());
            let mut c = Config::load(nvs);
            let ok = match (key, val) {
                (Some("min_on"), Some(v)) => {
                    c.min_on_secs = v;
                    true
                }
                (Some("run_on"), Some(v)) => {
                    c.run_on_secs = v;
                    true
                }
                (Some("led"), Some(v)) => {
                    c.led_mode = LedMode::from_u8(v as u8);
                    true
                }
                (Some("relay_ah"), Some(v)) => {
                    c.relay_active_high = v != 0;
                    true
                }
                (Some("led_ah"), Some(v)) => {
                    c.led_active_high = v != 0;
                    true
                }
                (Some("mac"), Some(v)) => {
                    c.mac_offset = v as u8;
                    true
                }
                _ => false,
            };
            if ok {
                let _ = c.save(nvs);
                console_print("ok (reboot to apply)");
            } else {
                console_print("usage: set <min_on|run_on|led|relay_ah|led_ah|mac> <number>");
            }
        }
        Some("aps") => {
            let on = it.next() != Some("0");
            APS_LOG.store(on, Ordering::Relaxed);
            console_print(if on { "aps logging on" } else { "aps logging off" });
        }
        Some("loglevel") => {
            // 0 none, 1 error, 2 warn, 3 info, 4 debug, 5 verbose
            let n = it.next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(3).min(5);
            unsafe { esp_idf_svc::sys::esp_log_level_set(c"*".as_ptr(), n) };
            log::set_max_level(match n {
                0 => log::LevelFilter::Off,
                1 => log::LevelFilter::Error,
                2 => log::LevelFilter::Warn,
                3 => log::LevelFilter::Info,
                4 => log::LevelFilter::Debug,
                _ => log::LevelFilter::Trace,
            });
            console_print(&format!("loglevel {n}"));
        }
        Some(other) => console_print(&format!("unknown cmd '{other}' (try help)")),
        None => {}
    }
}

/// Serial command console on UART0. Routes RX (the console only wires TX by
/// default — why earlier RX reads got nothing), sets an RX pull-up (floating-safe
/// in production), and parses newline-terminated commands so config, re-pair,
/// reboots and status can all be driven over the wire.
fn spawn_serial_console(nvs: EspDefaultNvsPartition) {
    std::thread::Builder::new()
        .name("console".into())
        .stack_size(8 * 1024)
        .spawn(move || {
            // Register the APS frame hook once the Zigbee stack is up (it
            // initialises ~150 ms after this thread starts) and well before a
            // join can complete. It filters OTA frames (see `aps_ind_cb`) and
            // feeds the optional frame log.
            std::thread::sleep(Duration::from_millis(500));
            unsafe {
                let locked = esp_zb_lock_acquire(u32::MAX);
                esp_zb_aps_data_indication_handler_register(aps_ind_cb);
                if locked {
                    esp_zb_lock_release();
                }
            }
            // Let huesmith/ESP-IDF finish setting up the console UART.
            std::thread::sleep(Duration::from_millis(1500));
            unsafe {
                // Route only RX=GPIO17; leave TX as the console set it (forcing TX
                // to the wrong pin silences all output). -1 = UART_PIN_NO_CHANGE.
                esp_idf_svc::sys::uart_set_pin(0u32, -1, UART0_RX_GPIO as i32, -1, -1);
                esp_idf_svc::sys::gpio_set_pull_mode(
                    UART0_RX_GPIO as i32,
                    esp_idf_svc::sys::gpio_pull_mode_t_GPIO_PULLUP_ONLY,
                );
                // Buffered RX (INVALID_STATE just means a driver is already there).
                let _ = esp_idf_svc::sys::uart_driver_install(
                    0u32,
                    512,
                    0,
                    0,
                    core::ptr::null_mut(),
                    0,
                );
            }
            console_print("serial console ready — type 'help'");

            let mut line: Vec<u8> = Vec::with_capacity(80);
            let mut b = [0u8; 1];
            let mut last_hb = Instant::now();
            let mut last_state = (2u8, 2u8);
            loop {
                // Print queued APS frame summaries.
                let pending: Vec<String> = match APS_Q.try_lock() {
                    Ok(mut q) => q.drain(..).collect(),
                    Err(_) => Vec::new(),
                };
                for l in pending {
                    console_print(&l);
                }

                // 100 ms read timeout so the loop also services the heartbeat.
                let n = unsafe {
                    esp_idf_svc::sys::uart_read_bytes(
                        0u32,
                        b.as_mut_ptr() as *mut core::ffi::c_void,
                        1,
                        10,
                    )
                };
                if n == 1 {
                    match b[0] {
                        b'\n' | b'\r' => {
                            if !line.is_empty() {
                                let cmd = String::from_utf8_lossy(&line).trim().to_string();
                                handle_command(&cmd, &nvs);
                                line.clear();
                            }
                        }
                        c if line.len() < 80 => line.push(c),
                        _ => {}
                    }
                }
                // Report pairing state when it changes, and every 30 s.
                let state = (
                    unsafe { esp_zb_bdb_is_factory_new() } as u8,
                    unsafe { esp_zb_bdb_dev_joined() } as u8,
                );
                if state != last_state || last_hb.elapsed() >= Duration::from_secs(30) {
                    last_hb = Instant::now();
                    last_state = state;
                    console_print(&format!("hb factory_new={} joined={}", state.0, state.1));
                }
            }
        })
        .ok();
}

// ─────────────────────────── Relay worker ───────────────────────────────────

/// Command from the Zigbee task to the relay worker: the Hue (lamp) on/off state.
enum Cmd {
    Lamp(bool),
}

/// Relay state machine, running off the Zigbee task so `LightOutput` callbacks
/// return instantly.
enum St {
    /// Relay open, lamp off.
    Idle,
    /// Lamp on; waiting out the minimum on-time before closing the relay.
    PendingOn(Instant),
    /// Relay closed, lamp on.
    On,
    /// Lamp off; relay still closed, running on until the deadline.
    RunOn(Instant),
}

// Zigbee API from the esp-zigbee lib — not in esp-idf-sys's bindings (huesmith
// declares its own), so we declare the few symbols we need; they resolve against
// the already-linked esp-zigbee library.
extern "C" {
    fn esp_zb_bdb_dev_joined() -> bool;
    fn esp_zb_bdb_is_factory_new() -> bool;
    fn esp_zb_get_pan_id() -> u16;
    fn esp_zb_get_long_address(addr: *mut u8);
    fn esp_zb_factory_reset();
    fn esp_zb_lock_acquire(block_ticks: u32) -> bool;
    fn esp_zb_lock_release();
    fn esp_zb_aps_data_indication_handler_register(cb: extern "C" fn(ApsInd) -> bool);
}

// ───────────────────── APS traffic logging (diagnostics) ─────────────────────

/// `esp_zb_apsde_data_ind_t` from esp-zigbee-lib 1.6.8 (aps/esp_zigbee_aps.h).
#[repr(C)]
struct ApsInd {
    status: u8,
    dst_addr_mode: u8,
    dst_short_addr: u16,
    dst_endpoint: u8,
    src_addr_mode: u8,
    src_short_addr: u16,
    src_endpoint: u8,
    profile_id: u16,
    cluster_id: u16,
    asdu_length: u32,
    asdu: *const u8,
    security_status: u8,
    lqi: i32,
    rx_time: i32,
}

/// ZCL OTA Upgrade cluster id.
const OTA_UPGRADE_CLUSTER: u16 = 0x0019;

/// General APS frame logging (toggle with the `aps` console command). Dropped
/// OTA frames are always reported.
static APS_LOG: AtomicBool = AtomicBool::new(true);
static APS_Q: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Called on the Zigbee task for every incoming APS data frame. Returning `true`
/// consumes the frame (the stack does not process it); `false` lets it through.
extern "C" fn aps_ind_cb(ind: ApsInd) -> bool {
    // huesmith advertises the OTA Upgrade client cluster (0x0019) for Hue
    // compatibility but sets up no OTA backend; when the Bridge sends an OTA
    // command (it does right after pairing), ZBOSS asserts in
    // zcl_ota_upgrade_commands.c and the device reboots. Swallow those frames so
    // the stack never sees them.
    if ind.profile_id == 0x0104 && ind.cluster_id == OTA_UPGRADE_CLUSTER {
        if let Ok(mut q) = APS_Q.try_lock() {
            if q.len() < 64 {
                q.push(format!(
                    "aps DROPPED ota frame src=0x{:04X} len={}",
                    ind.src_short_addr, ind.asdu_length
                ));
            }
        }
        return true;
    }
    if APS_LOG.load(Ordering::Relaxed) {
        let n = (ind.asdu_length as usize).min(20);
        let mut hex = String::with_capacity(n * 2);
        if !ind.asdu.is_null() {
            for i in 0..n {
                hex.push_str(&format!("{:02x}", unsafe { *ind.asdu.add(i) }));
            }
        }
        if let Ok(mut q) = APS_Q.try_lock() {
            if q.len() < 64 {
                q.push(format!(
                    "aps src=0x{:04X} ep{}->ep{} prof=0x{:04X} cl=0x{:04X} len={} st={} data={}",
                    ind.src_short_addr,
                    ind.src_endpoint,
                    ind.dst_endpoint,
                    ind.profile_id,
                    ind.cluster_id,
                    ind.asdu_length,
                    ind.status,
                    hex
                ));
            }
        }
    }
    false
}

fn relay_worker(rx: Receiver<Cmd>, cfg: Config) {
    configure_output(RELAY_GPIO);
    if let Some(pin) = LED_GPIO {
        configure_output(pin);
    }

    let min_on = Duration::from_secs(cfg.min_on_secs as u64);
    let run_on = Duration::from_secs(cfg.run_on_secs as u64);

    let mut lamp_on = false;
    let mut relay_closed = false;
    let mut st = St::Idle;
    set_relay(cfg.relay_active_high, false);

    // LED: a status code while one applies (re-pair pending, or not joined to a
    // Hue network), otherwise on/off per led_mode. The Zigbee stack isn't
    // queried for the first few seconds while it is still coming up.
    let start = Instant::now();
    let tick = Duration::from_millis(50);
    let mut code = 0u8;
    let mut code_since = Instant::now();

    log::info!(
        "relay worker: relay=GPIO{RELAY_GPIO} min_on={}s run_on={}s led={:?}",
        cfg.min_on_secs,
        cfg.run_on_secs,
        cfg.led_mode
    );

    loop {
        // Service commands; wake every `tick` to run timers and the blink.
        match rx.recv_timeout(tick) {
            Ok(Cmd::Lamp(true)) => {
                lamp_on = true;
                match st {
                    St::Idle => {
                        if min_on > Duration::ZERO {
                            st = St::PendingOn(Instant::now() + min_on);
                            log::info!("lamp ON; delaying relay {}s", min_on.as_secs());
                        } else {
                            set_relay(cfg.relay_active_high, true);
                            relay_closed = true;
                            st = St::On;
                            log::info!("fan ON");
                        }
                    }
                    St::RunOn(_) => {
                        st = St::On; // relay already closed; cancel the run-off
                        log::info!("fan stays ON (run-on cancelled)");
                    }
                    _ => {}
                }
            }
            Ok(Cmd::Lamp(false)) => {
                lamp_on = false;
                match st {
                    St::PendingOn(_) => {
                        st = St::Idle; // never engaged
                        log::info!("lamp OFF before min on-time; relay stays off");
                    }
                    St::On => {
                        if run_on > Duration::ZERO {
                            st = St::RunOn(Instant::now() + run_on);
                            log::info!("lamp OFF; running on for {}s", run_on.as_secs());
                        } else {
                            set_relay(cfg.relay_active_high, false);
                            relay_closed = false;
                            st = St::Idle;
                            log::info!("fan OFF");
                        }
                    }
                    _ => {}
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                log::error!("relay worker: channel closed; leaving fan as-is");
                break;
            }
        }

        // Relay timers (checked each tick).
        let now = Instant::now();
        match st {
            St::PendingOn(dl) if now >= dl => {
                set_relay(cfg.relay_active_high, true);
                relay_closed = true;
                st = St::On;
                log::info!("min on-time elapsed; fan ON");
            }
            St::RunOn(dl) if now >= dl => {
                set_relay(cfg.relay_active_high, false);
                relay_closed = false;
                st = St::Idle;
                log::info!("run-on elapsed; fan OFF");
            }
            _ => {}
        }

        let stack_up = start.elapsed() >= Duration::from_secs(3);
        let want = match LED_CODE_OVERRIDE.load(Ordering::Relaxed) {
            0 if stack_up && !unsafe { esp_zb_bdb_dev_joined() } => CODE_WAITING_FOR_HUE,
            c => c,
        };
        if want != code {
            code = want;
            code_since = Instant::now();
        }
        if code != 0 {
            if let Some(pin) = LED_GPIO {
                let lit = code_lit(code, code_since.elapsed().as_millis() as u64);
                set_level(pin, if lit == cfg.led_active_high { 1 } else { 0 });
            }
        } else {
            set_led(&cfg, lamp_on, relay_closed);
        }
    }
}

// ─────────────────────────── Hue light backend ──────────────────────────────

struct FanDriver {
    tx: Sender<Cmd>,
}

impl huesmith::LightOutput for FanDriver {
    fn set_on(&mut self, on: bool) {
        if self.tx.send(Cmd::Lamp(on)).is_err() {
            log::error!("relay worker gone; dropping set_on({on})");
        }
    }
    fn set_brightness(&mut self, _level: u8) {}
    fn set_rgb(&mut self, _r: u8, _g: u8, _b: u8) {}
}

// ────────────────────────────────── main ────────────────────────────────────

fn main() {
    esp_idf_svc::sys::link_patches();

    // Clear any stale force-download-boot bit so we don't get stuck in the ROM
    // bootloader after a serial-DFU reflash + reset.
    clear_force_download_boot();

    let nvs = EspDefaultNvsPartition::take().expect("take NVS partition");

    // Rapid-power-cycle detection: enough quick off/on cycles opens the WiFi
    // setup portal (a separate boot mode; Zigbee is not started there).
    let count = config::boot_count_inc(&nvs);
    if let Some(threshold) = PORTAL_CYCLES {
        if count >= threshold {
            config::boot_count_clear(&nvs);
            // Show the setup-mode code while the portal is up; "Flash LED" on
            // the setup page switches to long flashes so this device stands out.
            if let Some(pin) = LED_GPIO {
                configure_output(pin);
                let led_ah = Config::load(&nvs).led_active_high;
                std::thread::Builder::new()
                    .name("ledblink".into())
                    .stack_size(2 * 1024)
                    .spawn(move || {
                        let lit = |on: bool| set_level(pin, if on == led_ah { 1 } else { 0 });
                        let since = Instant::now();
                        loop {
                            if portal::identify_active() {
                                // "Flash LED": long flashes, unlike any code.
                                lit(true);
                                std::thread::sleep(Duration::from_millis(700));
                                lit(false);
                                std::thread::sleep(Duration::from_millis(150));
                            } else {
                                let t = since.elapsed().as_millis() as u64;
                                lit(code_lit(CODE_SETUP_MODE, t));
                                std::thread::sleep(Duration::from_millis(25));
                            }
                        }
                    })
                    .ok();
            }
            let sysloop = EspSystemEventLoop::take().expect("take event loop");
            let peripherals = Peripherals::take().expect("take peripherals");
            portal::run(peripherals.modem, sysloop, nvs); // never returns
        }
    }

    // Normal boot: clear the counter after the arm window if we stay powered.
    {
        let nvs_clear = nvs.clone();
        std::thread::Builder::new()
            .name("pcclear".into())
            .stack_size(3 * 1024)
            .spawn(move || {
                std::thread::sleep(RESET_ARM_WINDOW);
                config::boot_count_clear(&nvs_clear);
            })
            .ok();
    }

    let cfg = Config::load(&nvs);
    let mac_offset = cfg.mac_offset;

    // One-shot: re-pair requested from the portal. Clear the flag now, then do a
    // proper Zigbee factory reset a few seconds after the stack is up (it reboots
    // the device factory-new, so this boot's remaining setup is just interim).
    if cfg.repair_on_boot {
        log::warn!("re-pair requested from portal — scheduling address change + factory reset");
        Config::clear_repair_flag(&nvs);
        schedule_zigbee_factory_reset(nvs.clone());
    }

    // LED: quick power-on flashes, then settle per config.
    if let Some(pin) = LED_GPIO {
        configure_output(pin);
        let ah = cfg.led_active_high;
        for _ in 0..BOOT_FLASHES {
            set_level(pin, if ah { 1 } else { 0 });
            std::thread::sleep(Duration::from_millis(BOOT_FLASH_MS));
            set_level(pin, if ah { 0 } else { 1 });
            std::thread::sleep(Duration::from_millis(BOOT_FLASH_MS));
        }
    }

    // Relay worker owns the relay, LED and timers.
    let (tx, rx) = mpsc::channel::<Cmd>();
    std::thread::Builder::new()
        .name("relay".into())
        .stack_size(6 * 1024)
        .spawn(move || relay_worker(rx, cfg))
        .expect("spawn relay worker");

    if let Some(pin) = BUTTON_GPIO {
        spawn_button_watcher(pin, nvs.clone());
    }

    // Serial command console on UART0 (config / reboot / dfu / repair / status),
    // with the RX pull-up for a floating-safe RX in production.
    spawn_serial_console(nvs.clone());

    // Keep the NVS partition alive for the rest of the program (main never
    // returns once launch() is called).
    let _nvs_keepalive = nvs;

    let mut light = huesmith::custom(HueDeviceType::OnOffLight, FanDriver { tx })
        .name("huesmith", "SHELLY-FAN-1");
    if let Some(eui) = identity_eui64(mac_offset) {
        light = light.mac(eui);
    }
    light.launch(); // never returns; runs the Zigbee stack
}
