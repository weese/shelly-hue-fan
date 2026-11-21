//! Persistent configuration (NVS) and rapid-power-cycle counting.
//!
//! The device has no reachable button once it's built in behind the fan, so all
//! runtime options live in NVS and are edited through the WiFi setup portal
//! (see [`crate::portal`]), which is entered after a few rapid power cycles.

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs};
use esp_idf_svc::sys::EspError;

const CFG_NS: &str = "fancfg";
const BOOT_NS: &str = "fanboot";
const BOOT_KEY: &str = "pc";

/// What the status LED indicates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LedMode {
    /// Always off.
    Off,
    /// Follows the Hue on/off state (the "virtual lamp").
    Lamp,
    /// Follows the physical relay (stays lit through the run-on).
    Relay,
}

impl LedMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => LedMode::Off,
            2 => LedMode::Relay,
            _ => LedMode::Lamp,
        }
    }
    pub fn as_u8(self) -> u8 {
        match self {
            LedMode::Off => 0,
            LedMode::Lamp => 1,
            LedMode::Relay => 2,
        }
    }
}

/// All field-editable settings.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Minimum time the Hue switch must stay ON before the relay engages (s).
    pub min_on_secs: u32,
    /// How long the relay stays ON after the Hue switch goes OFF (run-on, s).
    pub run_on_secs: u32,
    pub led_mode: LedMode,
    pub relay_active_high: bool,
    pub led_active_high: bool,
    /// One-shot: on the next boot, forget the Hue pairing and re-enter pairing.
    pub repair_on_boot: bool,
    /// Which Zigbee (IEEE) address to present. A different address lets the
    /// device appear to the bridge as a brand-new device.
    /// 0 = hardware address, 1..=3 = the chip's other assigned addresses,
    /// 4..=255 = locally administered addresses (see `identity_eui64` in main.rs).
    /// Every re-pair moves to the next value automatically.
    pub mac_offset: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            min_on_secs: 0,
            run_on_secs: 360,
            led_mode: LedMode::Lamp,
            relay_active_high: true, // Shelly relays are active-high
            led_active_high: false,  // 1PM Gen4 LED (GPIO11) is inverted
            repair_on_boot: false,
            mac_offset: 0,
        }
    }
}

impl Config {
    /// Load from NVS, falling back to defaults for any missing key.
    pub fn load(part: &EspDefaultNvsPartition) -> Self {
        let mut c = Config::default();
        if let Ok(nvs) = EspNvs::new(part.clone(), CFG_NS, true) {
            if let Ok(Some(v)) = nvs.get_u32("min_on") {
                c.min_on_secs = v;
            }
            if let Ok(Some(v)) = nvs.get_u32("run_on") {
                c.run_on_secs = v;
            }
            if let Ok(Some(v)) = nvs.get_u8("led_mode") {
                c.led_mode = LedMode::from_u8(v);
            }
            if let Ok(Some(v)) = nvs.get_u8("relay_ah") {
                c.relay_active_high = v != 0;
            }
            if let Ok(Some(v)) = nvs.get_u8("led_ah") {
                c.led_active_high = v != 0;
            }
            if let Ok(Some(v)) = nvs.get_u8("repair") {
                c.repair_on_boot = v != 0;
            }
            if let Ok(Some(v)) = nvs.get_u8("mac_off") {
                c.mac_offset = v;
            }
        }
        c
    }

    /// Persist all fields to NVS.
    pub fn save(&self, part: &EspDefaultNvsPartition) -> Result<(), EspError> {
        let nvs = EspNvs::new(part.clone(), CFG_NS, true)?;
        nvs.set_u32("min_on", self.min_on_secs)?;
        nvs.set_u32("run_on", self.run_on_secs)?;
        nvs.set_u8("led_mode", self.led_mode.as_u8())?;
        nvs.set_u8("relay_ah", self.relay_active_high as u8)?;
        nvs.set_u8("led_ah", self.led_active_high as u8)?;
        nvs.set_u8("repair", self.repair_on_boot as u8)?;
        nvs.set_u8("mac_off", self.mac_offset)?;
        Ok(())
    }

    /// Clear only the one-shot re-pair flag (after acting on it at boot).
    pub fn clear_repair_flag(part: &EspDefaultNvsPartition) {
        if let Ok(nvs) = EspNvs::new(part.clone(), CFG_NS, true) {
            let _ = nvs.set_u8("repair", 0);
        }
    }
}

/// Increment the rapid-power-cycle counter and return the new value.
pub fn boot_count_inc(part: &EspDefaultNvsPartition) -> u8 {
    if let Ok(nvs) = EspNvs::new(part.clone(), BOOT_NS, true) {
        let n = nvs
            .get_u8(BOOT_KEY)
            .ok()
            .flatten()
            .unwrap_or(0)
            .saturating_add(1);
        let _ = nvs.set_u8(BOOT_KEY, n);
        n
    } else {
        0
    }
}

/// Make the next boot open the setup portal, as if the power had been cycled
/// enough times (used by the onboard button).
pub fn boot_count_force_portal(part: &EspDefaultNvsPartition) {
    if let Ok(nvs) = EspNvs::new(part.clone(), BOOT_NS, true) {
        let _ = nvs.set_u8(BOOT_KEY, u8::MAX - 1);
    }
}

/// Reset the rapid-power-cycle counter to zero.
pub fn boot_count_clear(part: &EspDefaultNvsPartition) {
    if let Ok(nvs) = EspNvs::new(part.clone(), BOOT_NS, true) {
        let _ = nvs.set_u8(BOOT_KEY, 0);
    }
}
