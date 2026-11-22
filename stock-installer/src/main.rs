//! One-shot installer: replaces Shelly's own firmware with shelly-hue-fan.
//!
//! Shelly's updater installs this program like one of its own firmware updates,
//! into whichever of its two app slots is not running, and its bootloader
//! starts it there. The program carries a complete shelly-hue-fan install
//! (bootloader, partition table, app; see `tools/make-stock-installer.py`) and
//! writes it in an order that stays recoverable as long as possible:
//!
//! 1. the app, into the shelly-hue-fan slot that doesn't overlap this program
//! 2. the shelly-hue-fan partition table (0x8000), empty settings, boot record
//! 3. the bootloader at 0x0, the only step a power cut can't recover from
//! 4. clean-up: Shelly's partition table, boot record and settings, and the
//!    shelly-hue-fan Zigbee storage, which overlaps Shelly's second app slot
//!
//! Until step 3, Shelly's bootloader and partition table are untouched, so after
//! an interruption Shelly's bootloader starts this installer again (or falls
//! back to Shelly's firmware). Shelly's `shelly`
//! partition (device data, at 0x7F0000) is never touched.

use std::time::Duration;

use esp_idf_sys as sys;

const PAYLOAD_LEN: usize = include_bytes!("../payload.bin").len();

/// Placed right after the app description, at the start of the image. On the
/// second Shelly slot (0x400000) the Zigbee storage erased in the last step
/// (0x420000..0x431000) therefore falls inside the payload, which has been
/// copied by then, and never inside code or other read-only data.
#[used]
#[link_section = ".rodata_custom_desc.shf_payload"]
static PAYLOAD: [u8; PAYLOAD_LEN] = *include_bytes!("../payload.bin");

const MAGIC: &[u8; 8] = b"SHFINST1";
const HEADER_LEN: usize = 0x40;
const BOOT_PT_LEN: usize = 0x9000; // bootloader 0x0..0x8000, partition table 0x8000..0x9000
const OTADATA_ENTRY_LEN: usize = 32;

const SECTOR: u32 = 0x1000;
const SHELLY_LOADER_SIG: &[u8] = b"Shelly OS loader";

// Shelly layout
const SHELLY_APP_0: core::ops::Range<u32> = 0x20000..0x320000;
const SHELLY_APP_1: core::ops::Range<u32> = 0x400000..0x700000;

// shelly-hue-fan layout (partitions.csv in the repository root)
const PT: u32 = 0x8000;
const NVS_AND_OTADATA0: core::ops::Range<u32> = 0x9000..0x10000;
const OTADATA0: u32 = 0xF000;
const OTA_0: u32 = 0x20000;
const OTA_1: u32 = 0x220000;
const SLOT_SIZE: u32 = 0x200000;
const ZIGBEE: core::ops::Range<u32> = 0x420000..0x431000;
/// Shelly's partition table, boot record and settings; also the second
/// otadata sector and phy_init of shelly-hue-fan.
const SHELLY_SYSTEM: core::ops::Range<u32> = 0x10000..0x20000;

const LED_GPIO: i32 = 11; // low = lit

struct Payload {
    version: &'static str,
    boot_pt: &'static [u8],
    otadata_ota_1: &'static [u8],
    app: &'static [u8],
}

fn payload() -> Result<Payload, String> {
    let p: &'static [u8] = core::hint::black_box(&PAYLOAD[..]);
    if p.len() < HEADER_LEN || &p[..8] != MAGIC {
        return Err("payload header missing".into());
    }
    let app_len = u32::from_le_bytes(p[8..12].try_into().unwrap()) as usize;
    let version_bytes = &p[12..12 + 20];
    let version_end = version_bytes.iter().position(|&b| b == 0).unwrap_or(20);
    let version = core::str::from_utf8(&version_bytes[..version_end]).unwrap_or("?");
    let otadata_ota_1 = &p[32..32 + OTADATA_ENTRY_LEN];
    let boot_pt = &p[HEADER_LEN..HEADER_LEN + BOOT_PT_LEN];
    let app = p
        .get(HEADER_LEN + BOOT_PT_LEN..HEADER_LEN + BOOT_PT_LEN + app_len)
        .ok_or("payload truncated")?;
    if boot_pt[0] != 0xE9 || boot_pt[PT as usize..PT as usize + 2] != [0xAA, 0x50] || app[0] != 0xE9 {
        return Err("payload contents invalid".into());
    }
    if app.len() as u32 > SLOT_SIZE {
        return Err("app larger than a slot".into());
    }
    Ok(Payload { version, boot_pt, otadata_ota_1, app })
}

fn phys(ptr: *const u8) -> Option<u32> {
    let a = unsafe { sys::spi_flash_cache2phys(ptr as *const core::ffi::c_void) };
    // SPI_FLASH_CACHE2PHYS_FAIL (UINT32_MAX) isn't in the bindings.
    (a != u32::MAX as usize).then_some(a as u32)
}

fn check(err: sys::esp_err_t, what: &str, addr: u32) -> Result<(), String> {
    sys::EspError::convert(err).map_err(|e| format!("{what} at {addr:#x}: {e}"))
}

fn erase(range: core::ops::Range<u32>) -> Result<(), String> {
    // Sector by sector, with a pause now and then so the idle task (watchdog)
    // gets to run during long erases.
    let mut a = range.start;
    while a < range.end {
        check(unsafe { sys::esp_flash_erase_region(core::ptr::null_mut(), a, SECTOR) }, "erase", a)?;
        a += SECTOR;
        if a % 0x10000 == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    Ok(())
}

/// Erase, write and read back. The source lives in memory-mapped flash, so
/// each chunk goes through a RAM buffer first.
fn write_verify(addr: u32, data: &[u8]) -> Result<(), String> {
    let len = data.len() as u32;
    erase(addr..addr + len.div_ceil(SECTOR) * SECTOR)?;
    let mut buf = vec![0u8; SECTOR as usize];
    let mut back = vec![0u8; SECTOR as usize];
    for (i, chunk) in data.chunks(SECTOR as usize).enumerate() {
        let a = addr + i as u32 * SECTOR;
        let n = chunk.len();
        buf[..n].copy_from_slice(chunk);
        check(unsafe { sys::esp_flash_write(core::ptr::null_mut(), buf.as_ptr().cast(), a, n as u32) }, "write", a)?;
        check(unsafe { sys::esp_flash_read(core::ptr::null_mut(), back.as_mut_ptr().cast(), a, n as u32) }, "read", a)?;
        if buf[..n] != back[..n] {
            return Err(format!("verify failed at {a:#x}"));
        }
    }
    Ok(())
}

fn shelly_loader_present() -> bool {
    let mut boot = vec![0u8; 0x8000];
    let err = unsafe { sys::esp_flash_read(core::ptr::null_mut(), boot.as_mut_ptr().cast(), 0, boot.len() as u32) };
    err == sys::ESP_OK && boot.windows(SHELLY_LOADER_SIG.len()).any(|w| w == SHELLY_LOADER_SIG)
}

fn led(on: bool) {
    unsafe { sys::gpio_set_level(LED_GPIO, u32::from(!on)) };
}

fn install() -> Result<(), String> {
    let p = payload()?;
    println!("installer: shelly-hue-fan {} ({} byte app)", p.version, p.app.len());

    if !shelly_loader_present() {
        return Err("Shelly's bootloader is not at 0x0; nothing to do".into());
    }

    let here = phys(install as *const u8).ok_or("can't locate the running slot")?;
    let (target, second_slot) = if SHELLY_APP_0.contains(&here) {
        (OTA_1, false)
    } else if SHELLY_APP_1.contains(&here) {
        (OTA_0, true)
    } else {
        return Err(format!("running at unexpected address {here:#x}"));
    };
    println!("installer: running from {here:#x}, installing to {target:#x}");

    // 1. App. From the first Shelly slot it goes to ota_1, above this
    //    program (the packaging script checks the installer ends below it);
    //    from the second Shelly slot to ota_0.
    write_verify(target, p.app)?;
    println!("installer: app written");

    // 2. Partition table, empty settings, boot record. Shelly's bootloader
    //    doesn't read any of this.
    write_verify(PT, &p.boot_pt[PT as usize..])?;
    erase(NVS_AND_OTADATA0)?;
    if target == OTA_1 {
        write_verify(OTADATA0, p.otadata_ota_1)?;
    }
    println!("installer: partition table written");

    // 3. Bootloader. From here on a power cut needs a serial flash.
    write_verify(0, &p.boot_pt[..PT as usize])?;
    println!("installer: bootloader written");

    // 4. Clean-up. Failures here are harmless, so they don't stop the restart.
    if let Err(e) = erase(SHELLY_SYSTEM) {
        println!("installer: {e}");
    }
    let payload_phys = phys(PAYLOAD.as_ptr()).zip(phys(PAYLOAD.as_ptr().wrapping_add(PAYLOAD_LEN - 1)));
    let zigbee_safe = !second_slot
        || matches!(payload_phys, Some((s, e)) if s <= ZIGBEE.start && e >= ZIGBEE.end - 1);
    if zigbee_safe {
        if let Err(e) = erase(ZIGBEE) {
            println!("installer: {e}");
        }
    } else {
        println!("installer: Zigbee storage overlaps installer code; left as is");
    }
    Ok(())
}

fn main() {
    sys::link_patches();
    unsafe {
        sys::gpio_reset_pin(LED_GPIO);
        sys::gpio_set_direction(LED_GPIO, sys::gpio_mode_t_GPIO_MODE_OUTPUT);
    }
    led(true);

    match install() {
        Ok(()) => {
            println!("installer: done, restarting into shelly-hue-fan");
            std::thread::sleep(Duration::from_millis(200));
            unsafe { sys::esp_restart() };
        }
        Err(e) => {
            // Stay here rather than restart into a loop. Before the
            // bootloader step, a power cycle retries.
            println!("installer: FAILED: {e}");
            loop {
                led(true);
                std::thread::sleep(Duration::from_millis(100));
                led(false);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}
