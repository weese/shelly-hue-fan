//! WiFi setup portal: a SoftAP + captive web server for field configuration and
//! over-the-air firmware updates.
//!
//! Entered after a few rapid power cycles (see `main`). The device comes up as an
//! open access point; the captive-DNS responder plus the OS connectivity probes
//! make the config page open automatically when you join. Change settings or
//! upload new firmware, and it reboots into the result. Left idle it reboots on
//! its own after `PORTAL_TIMEOUT_SECS`. WiFi and Zigbee never run at the same
//! time — the portal is a separate boot mode.
//!
//! The page is served entirely from flash (the AP has no internet), so all CSS
//! and JS are inline — no CDN fonts or libraries.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::net::UdpSocket;
use std::time::Duration;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::handle::RawHandle;
use esp_idf_svc::http::server::{Configuration as HttpConfig, EspHttpServer};
use esp_idf_svc::http::Method;
use esp_idf_svc::io::Write;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::ota::EspOta;
use esp_idf_svc::wifi::{AccessPointConfiguration, AuthMethod, Configuration as WifiConfig, EspWifi};

use crate::config::{Config, LedMode};

/// The SSID is this prefix plus the device id, e.g. `Shelly-Fan-Setup-2314`, so
/// several devices in setup mode at once don't clash.
const AP_SSID_PREFIX: &str = "Shelly-Fan-Setup";
/// Fallback only; the real gateway IP is read from the AP netif at runtime.
const AP_IP: [u8; 4] = [192, 168, 4, 1];
const PORTAL_TIMEOUT_SECS: u32 = 5 * 60;
/// How long "Flash LED" changes the LED pattern.
const IDENTIFY_SECS: u32 = 10;

type HandlerErr = Box<dyn std::error::Error>;

/// Last HTTP activity, seconds since boot (32-bit RISC-V has no `AtomicU64`).
static LAST_ACTIVITY_S: AtomicU32 = AtomicU32::new(0);
/// "Flash LED" is active until this time (seconds since boot).
static IDENTIFY_UNTIL_S: AtomicU32 = AtomicU32::new(0);
/// Relay state while in setup mode (the relay worker doesn't run here).
static RELAY_ON: AtomicBool = AtomicBool::new(false);
/// Whether the button input could be configured.
static BUTTON_OK: AtomicBool = AtomicBool::new(false);

/// Whether the LED should currently show the identify pattern (read by the
/// LED thread `main` starts for setup mode).
pub fn identify_active() -> bool {
    now_secs() < IDENTIFY_UNTIL_S.load(Ordering::Relaxed)
}

fn now_secs() -> u32 {
    ((unsafe { esp_idf_svc::sys::esp_timer_get_time() } as u64) / 1_000_000) as u32
}

fn touch() {
    LAST_ACTIVITY_S.store(now_secs(), Ordering::Relaxed);
}

/// Run the setup portal. Never returns — reboots into normal mode on Save, after
/// an OTA update, or after `PORTAL_TIMEOUT_SECS` of inactivity.
pub fn run(modem: Modem, sysloop: EspSystemEventLoop, nvs: EspDefaultNvsPartition) -> ! {
    // Portal mode never reaches huesmith, which would otherwise set up logging.
    esp_idf_svc::log::EspLogger::initialize_default();
    touch();

    // Relay in a defined state (open) for the "Relay" test button.
    let relay_active_high = Config::load(&nvs).relay_active_high;
    crate::configure_output(crate::RELAY_GPIO);
    if let Some(pin) = crate::BUTTON_GPIO {
        BUTTON_OK.store(crate::configure_button_input(pin), Ordering::Relaxed);
    }
    crate::set_relay(relay_active_high, false);

    let id = crate::device_id();
    let ssid = format!("{AP_SSID_PREFIX}-{id}");

    let mut wifi = EspWifi::new(modem, sysloop, Some(nvs.clone())).expect("wifi new");
    wifi.set_configuration(&WifiConfig::AccessPoint(AccessPointConfiguration {
        ssid: ssid.as_str().try_into().expect("ssid"),
        auth_method: AuthMethod::None,
        channel: 1,
        max_connections: 4,
        ..Default::default()
    }))
    .expect("wifi ap config");
    wifi.start().expect("wifi start");

    // Read the AP's actual gateway IP — this ESP-IDF defaults the SoftAP subnet
    // to 192.168.71.x, not the classic 192.168.4.x, so never hardcode it.
    let ap_ip = wifi
        .ap_netif()
        .get_ip_info()
        .map(|i| i.ip.octets())
        .unwrap_or(AP_IP);
    log::warn!(
        "Setup portal: AP '{ssid}' up at http://{}.{}.{}.{}",
        ap_ip[0],
        ap_ip[1],
        ap_ip[2],
        ap_ip[3]
    );

    // Make the AP's DHCP server advertise *us* as the DNS server, so clients send
    // their captive-portal probe lookups here (the SoftAP doesn't offer DNS by
    // default — without this the page only opens if you type the IP manually).
    configure_ap_dns(RawHandle::handle(wifi.ap_netif()), ap_ip);

    spawn_captive_dns(ap_ip);

    let mut server = EspHttpServer::new(&HttpConfig {
        uri_match_wildcard: true,
        stack_size: 10240, // OTA writes run on the httpd task
        ..Default::default()
    })
    .expect("http server");

    // POST /save — store settings and reboot into normal mode.
    let nvs_post = nvs.clone();
    server
        .fn_handler("/save", Method::Post, move |mut req| -> Result<(), HandlerErr> {
            touch();
            let mut buf = [0u8; 1024];
            let mut len = 0usize;
            loop {
                let n = req.read(&mut buf[len..])?;
                len += n;
                if n == 0 || len >= buf.len() {
                    break;
                }
            }
            let body = core::str::from_utf8(&buf[..len]).unwrap_or("");
            let mut cfg = Config::load(&nvs_post);
            apply_form(&mut cfg, body);
            let _ = cfg.save(&nvs_post);
            log::warn!("Portal: config saved, rebooting into normal mode");
            let mut resp = req.into_ok_response()?;
            resp.write_all(reboot_page("Saved").as_bytes())?;
            drop(resp);
            std::thread::sleep(Duration::from_millis(800));
            unsafe { esp_idf_svc::sys::esp_restart() };
        })
        .expect("POST /save");

    // POST /ota — stream an uploaded firmware image to the inactive slot, reboot.
    server
        .fn_handler("/ota", Method::Post, move |mut req| -> Result<(), HandlerErr> {
            touch();
            log::warn!("Portal: OTA upload starting");
            let mut ota = EspOta::new()?;
            let mut update = ota.initiate_update()?;
            let mut buf = [0u8; 4096];
            let mut total = 0usize;
            loop {
                let n = req.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                if let Err(e) = update.write(&buf[..n]) {
                    let _ = update.abort();
                    return Err(Box::new(e));
                }
                total += n;
                touch();
            }
            update.complete()?;
            log::warn!("Portal: OTA wrote {total} bytes; rebooting into new firmware");
            let mut resp = req.into_ok_response()?;
            resp.write_all(reboot_page("Firmware flashed").as_bytes())?;
            drop(resp);
            std::thread::sleep(Duration::from_millis(1000));
            unsafe { esp_idf_svc::sys::esp_restart() };
        })
        .expect("POST /ota");

    // POST /relay — toggle the relay (switches the fan when on mains).
    server
        .fn_handler("/relay", Method::Post, move |req| -> Result<(), HandlerErr> {
            touch();
            let on = !RELAY_ON.load(Ordering::Relaxed);
            crate::set_relay(relay_active_high, on);
            RELAY_ON.store(on, Ordering::Relaxed);
            log::warn!("Portal: relay {}", if on { "on" } else { "off" });
            let mut resp =
                req.into_response(200, None, &[("Content-Type", "application/json")])?;
            resp.write_all(if on { b"{\"on\":true}" } else { b"{\"on\":false}" })?;
            Ok(())
        })
        .expect("POST /relay");

    // GET /button — live button state for the "Identify & test" card. Polled
    // by the page, so it doesn't count as activity for the idle timeout.
    server
        .fn_handler("/button", Method::Get, move |req| -> Result<(), HandlerErr> {
            let body = match crate::BUTTON_GPIO {
                Some(pin) if BUTTON_OK.load(Ordering::Relaxed) => {
                    let level = crate::get_level(pin);
                    format!("{{\"level\":{level},\"pressed\":{}}}", crate::button_pressed(pin))
                }
                _ => "{\"level\":-1,\"pressed\":false}".to_string(),
            };
            let mut resp =
                req.into_response(200, None, &[("Content-Type", "application/json")])?;
            resp.write_all(body.as_bytes())?;
            Ok(())
        })
        .expect("GET /button");

    // POST /identify — long LED flashes for IDENTIFY_SECS.
    server
        .fn_handler("/identify", Method::Post, move |req| -> Result<(), HandlerErr> {
            touch();
            IDENTIFY_UNTIL_S.store(now_secs() + IDENTIFY_SECS, Ordering::Relaxed);
            let mut resp = req.into_ok_response()?;
            resp.write_all(b"ok")?;
            Ok(())
        })
        .expect("POST /identify");

    // GET /* — catch-all: serve the form for the root AND every OS captive probe
    // (Apple /hotspot-detect.html, Android /generate_204, Windows /ncsi.txt, …),
    // so the "sign in to network" sheet opens automatically.
    let nvs_get = nvs.clone();
    server
        .fn_handler("/*", Method::Get, move |req| -> Result<(), HandlerErr> {
            touch();
            let cfg = Config::load(&nvs_get);
            let html = render_form(&cfg, &id, RELAY_ON.load(Ordering::Relaxed));
            let mut resp = req.into_ok_response()?;
            resp.write_all(html.as_bytes())?;
            Ok(())
        })
        .expect("GET /*");

    // Idle-timeout watchdog.
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let idle = now_secs().saturating_sub(LAST_ACTIVITY_S.load(Ordering::Relaxed));
        if idle >= PORTAL_TIMEOUT_SECS {
            log::warn!("Setup portal idle for {idle}s — rebooting to normal mode");
            unsafe { esp_idf_svc::sys::esp_restart() };
        }
    }
}

/// Configure the SoftAP's DHCP server to hand clients our own IP as their DNS
/// server (DHCP option 6), so captive-portal probe lookups come to us.
fn configure_ap_dns(netif: *mut esp_idf_svc::sys::esp_netif_t, ip: [u8; 4]) {
    use esp_idf_svc::sys;
    let addr = (ip[0] as u32)
        | ((ip[1] as u32) << 8)
        | ((ip[2] as u32) << 16)
        | ((ip[3] as u32) << 24);
    unsafe {
        sys::esp_netif_dhcps_stop(netif);
        let mut dns: sys::esp_netif_dns_info_t = core::mem::zeroed();
        dns.ip.u_addr.ip4.addr = addr;
        dns.ip.type_ = sys::ESP_IPADDR_TYPE_V4 as u8;
        sys::esp_netif_set_dns_info(netif, sys::esp_netif_dns_type_t_ESP_NETIF_DNS_MAIN, &mut dns);
        let mut offer: u8 = 2; // OFFER_DNS
        sys::esp_netif_dhcps_option(
            netif,
            sys::esp_netif_dhcp_option_mode_t_ESP_NETIF_OP_SET,
            sys::esp_netif_dhcp_option_id_t_ESP_NETIF_DOMAIN_NAME_SERVER,
            &mut offer as *mut u8 as *mut core::ffi::c_void,
            core::mem::size_of::<u8>() as u32,
        );
        sys::esp_netif_dhcps_start(netif);
    }
}

/// Minimal captive-portal DNS: answer every query with the AP's own IP so the
/// client's connectivity check is redirected here.
fn spawn_captive_dns(ip: [u8; 4]) {
    std::thread::Builder::new()
        .name("dns".into())
        .stack_size(4096)
        .spawn(move || {
            let sock = match UdpSocket::bind("0.0.0.0:53") {
                Ok(s) => s,
                Err(e) => {
                    log::error!("captive DNS bind failed: {e}");
                    return;
                }
            };
            let mut buf = [0u8; 512];
            loop {
                let (n, src) = match sock.recv_from(&mut buf) {
                    Ok(x) => x,
                    Err(_) => continue,
                };
                if n < 12 {
                    continue;
                }
                let mut r: Vec<u8> = Vec::with_capacity(n + 16);
                r.extend_from_slice(&buf[0..2]); // transaction ID
                r.extend_from_slice(&[0x81, 0x80]); // flags: response + recursion available
                r.extend_from_slice(&buf[4..6]); // QDCOUNT (echo)
                r.extend_from_slice(&[0x00, 0x01]); // ANCOUNT = 1
                r.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // NSCOUNT / ARCOUNT = 0

                // Copy the question section (QNAME..QTYPE,QCLASS).
                let mut i = 12usize;
                while i < n && buf[i] != 0 {
                    i += 1 + buf[i] as usize;
                }
                let qend = core::cmp::min(i + 1 + 4, n);
                if qend <= 12 {
                    continue;
                }
                r.extend_from_slice(&buf[12..qend]);

                // Answer: pointer to the question name, type A / class IN, our IP.
                r.extend_from_slice(&[0xC0, 0x0C]);
                r.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]);
                r.extend_from_slice(&[0x00, 0x00, 0x00, 0x3C]); // TTL 60s
                r.extend_from_slice(&[0x00, 0x04]); // RDLENGTH
                r.extend_from_slice(&ip);

                let _ = sock.send_to(&r, src);
            }
        })
        .ok();
}

// ─────────────────────────────── Web UI ─────────────────────────────────────

fn checked(b: bool) -> &'static str {
    if b {
        "checked"
    } else {
        ""
    }
}

fn rsel(mode: LedMode, want: LedMode) -> &'static str {
    if mode == want {
        "checked"
    } else {
        ""
    }
}

fn render_form(c: &Config, id: &str, relay_on: bool) -> String {
    PAGE.replace("__ID__", id)
        .replace("__FW__", env!("FW_VERSION"))
        .replace("__RELAY_ON__", if relay_on { "true" } else { "false" })
        .replace("__MIN_ON__", &c.min_on_secs.to_string())
        .replace("__RUN_ON__", &c.run_on_secs.to_string())
        .replace("__SEL_OFF__", rsel(c.led_mode, LedMode::Off))
        .replace("__SEL_LAMP__", rsel(c.led_mode, LedMode::Lamp))
        .replace("__SEL_RELAY__", rsel(c.led_mode, LedMode::Relay))
        .replace("__RELAY_NORMAL__", checked(c.relay_active_high))
        .replace("__RELAY_INV__", checked(!c.relay_active_high))
        .replace("__LED_NORMAL__", checked(c.led_active_high))
        .replace("__LED_INV__", checked(!c.led_active_high))
}

fn reboot_page(title: &str) -> String {
    format!(
        "<!doctype html><html lang=en><meta charset=utf-8>\
<meta name=viewport content=\"width=device-width,initial-scale=1\"><title>{title}</title>\
<style>:root{{color-scheme:dark light}}body{{margin:0;min-height:100vh;display:flex;\
align-items:center;justify-content:center;background:#0d1117;color:#e6edf3;\
font-family:ui-sans-serif,system-ui,-apple-system,Segoe UI,Roboto,sans-serif;text-align:center}}\
.c{{padding:2rem}}.d{{width:14px;height:14px;border-radius:50%;background:#2dd4bf;margin:0 auto 1rem;\
box-shadow:0 0 0 6px rgba(45,212,191,.2)}}h1{{font-size:1.25rem;margin:0 0 .4rem}}\
p{{color:#9aa7b4;margin:0}}@media(prefers-color-scheme:light){{body{{background:#f3f5f7;color:#111820}}}}</style>\
<body><div class=c><div class=d></div><h1>{title}</h1><p>Rebooting\u{2026} you can close this page.</p></div></body></html>"
    )
}

/// The setup page. Served offline; all CSS/JS inline. `__TOKENS__` are filled by
/// `render_form`.
const PAGE: &str = r###"<!doctype html>
<html lang=en>
<meta charset=utf-8>
<meta name=viewport content="width=device-width,initial-scale=1">
<title>Fan Controller __ID__</title>
<style>
:root{
  --bg:#0d1117;--panel:#161b22;--panel2:#1b212a;--line:#2a313b;
  --txt:#e6edf3;--muted:#9aa7b4;--accent:#2dd4bf;--accent-ink:#04120f;--danger:#f87171;--r:14px;
}
@media(prefers-color-scheme:light){
  :root{--bg:#eef1f4;--panel:#fff;--panel2:#f6f8fa;--line:#e2e7ec;--txt:#0f1720;--muted:#5b6774;--accent:#0d9488;--accent-ink:#fff;--danger:#dc2626;}
}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--txt);-webkit-font-smoothing:antialiased;
  font-family:ui-sans-serif,system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;line-height:1.5;
  padding:26px 16px 48px;display:flex;justify-content:center}
.wrap{width:100%;max-width:460px;animation:rise .4s ease both}
@keyframes rise{from{opacity:0;transform:translateY(8px)}to{opacity:1;transform:none}}
.brand{display:flex;align-items:center;gap:12px;margin:2px 2px 20px}
.dot{width:11px;height:11px;border-radius:50%;background:var(--accent);flex:none;
  box-shadow:0 0 0 5px color-mix(in srgb,var(--accent) 22%,transparent);animation:pulse 2.6s ease-in-out infinite}
@keyframes pulse{50%{box-shadow:0 0 0 9px color-mix(in srgb,var(--accent) 6%,transparent)}}
.brand h1{font-size:19px;margin:0;font-weight:650;letter-spacing:.2px}
.brand div span{color:var(--muted);font-size:12.5px}
.card{background:var(--panel);border:1px solid var(--line);border-radius:var(--r);padding:18px;margin-bottom:14px}
.card h2{font-size:12px;letter-spacing:.14em;text-transform:uppercase;color:var(--muted);margin:0 0 14px;font-weight:600}
.field{margin-bottom:16px}.field:last-child{margin-bottom:0}
.field>label{display:block;font-size:14px;font-weight:550;margin-bottom:6px}
.sub{color:var(--muted);font-size:12.5px;margin:6px 2px 0}
input[type=number]{width:100%;background:var(--panel2);border:1px solid var(--line);color:var(--txt);
  border-radius:10px;padding:11px 12px;font-size:16px;outline:none;transition:border-color .15s,box-shadow .15s}
input[type=number]:focus{border-color:var(--accent);box-shadow:0 0 0 3px color-mix(in srgb,var(--accent) 22%,transparent)}
.seg{display:grid;grid-template-columns:repeat(3,1fr);gap:4px;background:var(--panel2);border:1px solid var(--line);border-radius:11px;padding:4px}
.seg input{position:absolute;opacity:0;pointer-events:none}
.seg label{text-align:center;padding:10px 4px;border-radius:8px;font-size:13.5px;color:var(--muted);cursor:pointer;transition:.15s;user-select:none}
.seg input:checked+label{background:var(--accent);color:var(--accent-ink);font-weight:600}
.seg.two{grid-template-columns:1fr 1fr}
.row{display:flex;align-items:center;justify-content:space-between;gap:14px;padding:13px 0;border-top:1px solid var(--line)}
.row:first-of-type{border-top:0;padding-top:0}
.row .l{font-size:14px;font-weight:500}.row .l small{display:block;color:var(--muted);font-size:12px;font-weight:400}
.sw{position:relative;width:46px;height:27px;flex:none}
.sw input{position:absolute;opacity:0;width:100%;height:100%;margin:0;cursor:pointer;z-index:1}
.tk{position:absolute;inset:0;background:var(--line);border-radius:999px;transition:.2s}
.tk:before{content:"";position:absolute;top:3px;left:3px;width:21px;height:21px;border-radius:50%;background:#fff;transition:.2s}
.sw input:checked+.tk{background:var(--accent)}
.sw input:checked+.tk:before{transform:translateX(19px)}
.btn{width:100%;border:0;border-radius:11px;padding:14px;font-size:15px;font-weight:650;cursor:pointer;
  background:var(--accent);color:var(--accent-ink);transition:filter .15s,transform .05s}
.btn:hover{filter:brightness(1.08)}.btn:active{transform:translateY(1px)}
.btn.ghost{background:transparent;color:var(--txt);border:1px solid var(--line)}
.btn.ghost.on{background:var(--accent);color:var(--accent-ink);border-color:transparent}
.acts{display:grid;grid-template-columns:1fr 1fr;gap:10px}
.file{display:flex;align-items:center;justify-content:center;gap:8px;width:100%;background:var(--panel2);
  border:1.5px dashed var(--line);border-radius:11px;padding:16px;text-align:center;color:var(--muted);font-size:13.5px;cursor:pointer;transition:border-color .15s}
.file:hover{border-color:var(--accent)}.file input{display:none}
.status{font-size:13px;color:var(--muted);margin:12px 2px 0;min-height:1.1em}
.foot{color:var(--muted);font-size:11.5px;text-align:center;margin-top:10px;line-height:1.6}
.danger{color:var(--danger)}
</style>
<body>
<div class=wrap>
  <div class=brand><div class=dot></div>
    <div><h1>Fan Controller</h1><span>Shelly 1PM Gen4 &middot; Hue &middot; __ID__</span></div></div>

  <form method=POST action=/save>
    <div class=card>
      <h2>Timing</h2>
      <div class=field><label for=min_on>Minimum on-time</label>
        <input id=min_on type=number min=0 max=3600 name=min_on value=__MIN_ON__>
        <p class=sub>Seconds the switch must stay on before the relay engages.</p></div>
      <div class=field><label for=run_on>Run-on after off</label>
        <input id=run_on type=number min=0 max=86400 name=run_on value=__RUN_ON__>
        <p class=sub>Seconds the fan keeps running after Hue turns off.</p></div>
    </div>

    <div class=card>
      <h2>Status LED</h2>
      <div class=seg>
        <input type=radio id=l0 name=led_mode value=0 __SEL_OFF__><label for=l0>Off</label>
        <input type=radio id=l1 name=led_mode value=1 __SEL_LAMP__><label for=l1>Lamp</label>
        <input type=radio id=l2 name=led_mode value=2 __SEL_RELAY__><label for=l2>Relay</label>
      </div>
      <p class=sub>&ldquo;Lamp&rdquo; follows the Hue state; &ldquo;Relay&rdquo; follows the physical fan.</p>
    </div>

    <div class=card>
      <h2>Wiring</h2>
      <div class=field><label>Relay output</label>
        <div class="seg two">
          <input type=radio id=rn name=relay_ah value=1 __RELAY_NORMAL__><label for=rn>Normal</label>
          <input type=radio id=ri name=relay_ah value=0 __RELAY_INV__><label for=ri>Inverted</label>
        </div>
        <p class=sub>Choose &ldquo;Inverted&rdquo; if the fan runs while Hue shows it off.</p></div>
      <div class=field><label>Status LED output</label>
        <div class="seg two">
          <input type=radio id=en name=led_ah value=1 __LED_NORMAL__><label for=en>Normal</label>
          <input type=radio id=ei name=led_ah value=0 __LED_INV__><label for=ei>Inverted</label>
        </div>
        <p class=sub>The Shelly 1PM Gen4&rsquo;s LED is wired inverted (the default).</p></div>
      <div class=row><div class="l danger">Forget Hue &amp; re-pair<small>Erase pairing on next reboot</small></div>
        <label class=sw><input type=checkbox name=repair><span class=tk></span></label></div>
    </div>

    <button class=btn type=submit>Save &amp; reboot</button>
  </form>

  <div class=card style="margin-top:14px">
    <h2>Firmware update</h2>
    <form id=ota>
      <label class=file><span id=fname>Choose a .bin firmware file</span>
        <input type=file id=fw accept=.bin required></label>
      <button class="btn ghost" type=submit style="margin-top:12px">Upload &amp; flash</button>
      <p class=status id=otastatus></p>
    </form>
  </div>

  <div class=card>
    <h2>Identify &amp; test</h2>
    <div class=acts>
      <button class="btn ghost" type=button id=ident>Flash LED</button>
      <button class="btn ghost" type=button id=relay>Relay</button>
    </div>
    <p class=sub>&ldquo;Flash LED&rdquo; makes this device&rsquo;s LED give long flashes for 10&nbsp;s. &ldquo;Relay&rdquo; switches the fan on and off when the switch is on mains.</p>
    <p class=sub id=btn>Button: &hellip;</p>
  </div>

  <p class=foot>Firmware __FW__<br>Portal closes after 5 min idle<br>Do not power off during a firmware update</p>
</div>
<script>
var rb=document.getElementById('relay');
function rs(on){rb.textContent='Relay: '+(on?'on':'off');rb.classList.toggle('on',on)}
rs(__RELAY_ON__);
rb.onclick=async function(){try{var r=await fetch('/relay',{method:'POST'});rs((await r.json()).on)}catch(e){}};
var bt=document.getElementById('btn');
setInterval(async function(){try{var b=await(await fetch('/button')).json();
 bt.textContent=b.level<0?'Button: not available':'Button: '+(b.pressed?'pressed':'released')+' (input '+(b.level?'high':'low')+')';}catch(e){}},400);
var ib=document.getElementById('ident');
ib.onclick=async function(){try{await fetch('/identify',{method:'POST'});ib.textContent='Flashing…';setTimeout(function(){ib.textContent='Flash LED'},10000)}catch(e){}};
var fw=document.getElementById('fw'),fn=document.getElementById('fname');
fw.onchange=function(){fn.textContent=fw.files[0]?fw.files[0].name:'Choose a .bin firmware file'};
document.getElementById('ota').onsubmit=async function(e){e.preventDefault();
 var f=fw.files[0];if(!f)return;var s=document.getElementById('otastatus');
 s.textContent='Uploading '+f.name+' ('+((f.size/1024)|0)+' KB), keep power on…';
 try{var r=await fetch('/ota',{method:'POST',body:f});
  s.textContent=r.ok?'Flashed — rebooting into new firmware…':'Failed: HTTP '+r.status;}
 catch(err){s.textContent='Upload ended (device may be rebooting)';}};
</script>
</body>
</html>
"###;

/// Find `key=value` in an `application/x-www-form-urlencoded` body.
fn field<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    body.split('&').find_map(|kv| {
        let mut it = kv.splitn(2, '=');
        if it.next()? == key {
            it.next()
        } else {
            None
        }
    })
}

/// Checkboxes appear in the body only when ticked.
fn present(body: &str, key: &str) -> bool {
    body.split('&').any(|kv| kv.split('=').next() == Some(key))
}

fn apply_form(cfg: &mut Config, body: &str) {
    if let Some(v) = field(body, "min_on").and_then(|s| s.parse().ok()) {
        cfg.min_on_secs = v;
    }
    if let Some(v) = field(body, "run_on").and_then(|s| s.parse().ok()) {
        cfg.run_on_secs = v;
    }
    if let Some(v) = field(body, "led_mode").and_then(|s| s.parse::<u8>().ok()) {
        cfg.led_mode = LedMode::from_u8(v);
    }
    // Radio groups: "1" = Normal (active-high), "0" = Inverted.
    if let Some(v) = field(body, "relay_ah") {
        cfg.relay_active_high = v == "1";
    }
    if let Some(v) = field(body, "led_ah") {
        cfg.led_active_high = v == "1";
    }
    cfg.repair_on_boot = present(body, "repair");
}
