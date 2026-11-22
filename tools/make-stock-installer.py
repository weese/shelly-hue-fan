#!/usr/bin/env python3
"""Build a package that installs shelly-hue-fan from Shelly's own web UI.

    python3 tools/make-stock-installer.py --full dist/X-full.bin --app dist/X-app.bin \
        --version X --out dist/shelly-hue-fan-X-stock-installer-S1PMG4.zip

Shelly's updater only accepts a zip in its own format and only installs an
app into one of its own app slots, under its own bootloader and partition
table. So the package doesn't contain the firmware directly but a small
installer program (stock-installer/) with the firmware embedded. Started by
Shelly's bootloader, it writes the shelly-hue-fan bootloader, partition table
and app, then restarts into it.

The embedded firmware comes from the normal build: the first 0x9000 bytes of
the full image (bootloader and partition table) and the app image, so the
installer produces the same flash contents as a serial flash.

Shelly's updater installs an unsigned package only if the manifest name is the
device's own model code (S1PMG4 for the 1PM Gen4); other names need Shelly's
signature. Format and this behaviour as found by
https://github.com/hencou/shelly_gen4_matter_module (tools/make-webui-ota-zip.py).
"""
import argparse, binascii, datetime, hashlib, json, os, struct, subprocess, sys, tempfile, zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATE = os.path.join(ROOT, "stock-installer")

MODEL_CODE = "S1PMG4"
MANIFEST_VERSION = "99.0.0"  # above any stock version, so it's never refused as a downgrade
NVS_SIZE = 0xC000
FS_SIZE = 0xE0000

MAGIC = b"SHFINST1"
BOOT_PT_LEN = 0x9000
# The installer runs from 0x20000 or 0x400000. From 0x20000 it writes the app
# to 0x220000, so it must end below that.
MAX_INSTALLER = 0x220000 - 0x20000
# From 0x400000 it erases 0x420000..0x431000 (Zigbee storage) last; that range
# must lie inside the embedded payload, not in code.
ZIGBEE_IN_IMAGE = (0x420000 - 0x400000, 0x431000 - 0x400000)


def otadata_entry_for_ota_1():
    # esp_ota_select_entry_t: ota_seq, seq_label[20], ota_state, crc.
    # Sequence 2 selects ota_1; the CRC covers ota_seq only.
    seq = struct.pack("<I", 2)
    crc = binascii.crc32(seq, 0xFFFFFFFF) & 0xFFFFFFFF
    return seq + b"\xff" * 20 + struct.pack("<I", 0xFFFFFFFF) + struct.pack("<I", crc)


def make_payload(full, app, version):
    if len(full) < BOOT_PT_LEN or full[0] != 0xE9 or full[0x8000:0x8002] != b"\xaa\x50":
        sys.exit("--full doesn't look like a merged image (bootloader at 0x0, partition table at 0x8000)")
    if app[0] != 0xE9:
        sys.exit("--app doesn't look like an app image")
    ver = version.encode()[:19]
    header = MAGIC + struct.pack("<I", len(app)) + ver + b"\0" * (20 - len(ver)) + otadata_entry_for_ota_1()
    assert len(header) == 0x40
    return header + full[:BOOT_PT_LEN] + app


def build_installer(tmp):
    env = dict(os.environ)
    subprocess.run(["cargo", "build", "--release"], cwd=CRATE, env=env, check=True)
    image = os.path.join(tmp, "installer.bin")
    subprocess.run(["cargo", "espflash", "save-image", "--release", "--chip", "esp32c6",
                    "--flash-size", "8mb", "--partition-table", "partitions.csv", image],
                   cwd=CRATE, env=env, check=True)
    return open(image, "rb").read()


def check_layout(image, payload):
    if len(image) > MAX_INSTALLER:
        sys.exit(f"installer is {len(image):#x} bytes; it must stay below {MAX_INSTALLER:#x}")
    start = image.find(payload[:0x40])
    if start < 0 or image[start:start + len(payload)] != payload:
        sys.exit("payload not found in the installer image")
    end = start + len(payload)
    lo, hi = ZIGBEE_IN_IMAGE
    if not (start <= lo and end >= hi):
        sys.exit(f"payload at {start:#x}..{end:#x} doesn't cover {lo:#x}..{hi:#x}")
    print(f"installer image {len(image)} bytes, payload at {start:#x}..{end:#x}")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--full", required=True, help="merged 8 MB image (save-image --merge)")
    ap.add_argument("--app", required=True, help="app image (save-image)")
    ap.add_argument("--version", required=True)
    ap.add_argument("--out", required=True, help="zip to write")
    args = ap.parse_args()

    payload = make_payload(open(args.full, "rb").read(), open(args.app, "rb").read(), args.version)
    with open(os.path.join(CRATE, "payload.bin"), "wb") as f:
        f.write(payload)

    with tempfile.TemporaryDirectory() as tmp:
        image = build_installer(tmp)
        check_layout(image, payload)

        fs = b"\xff" * FS_SIZE
        now = datetime.datetime.now(datetime.timezone.utc)

        def part(data, member, **extra):
            return {"src": member, "size": len(data), "cs_sha256": hashlib.sha256(data).hexdigest(), **extra}

        # Shelly's updater writes app and fs to the slot that isn't running,
        # whatever ptn says; nvs is erased.
        manifest = {
            "name": MODEL_CODE,
            "platform": "esp32c6",
            "version": MANIFEST_VERSION,
            "build_id": now.strftime("%Y%m%d-%H%M%S") + f"/shelly-hue-fan-{args.version}",
            "build_timestamp": now.strftime("%Y-%m-%dT%H:%M:%SZ"),
            "parts": {
                "nvs": {"type": "nvs", "size": NVS_SIZE, "fill": 255, "ptn": "nvs"},
                "app": part(image, "app.bin", type="app", ptn="app_0", encrypt=True),
                "fs": part(fs, "fs.img", type="fs", ptn="fs_0", fs_size=FS_SIZE, encrypt=True),
            },
            "compatible": f"{MODEL_CODE}*",
        }
        # Stored, not deflated, manifest first, then parts in manifest order:
        # Shelly's updater supports nothing else.
        with zipfile.ZipFile(args.out, "w", zipfile.ZIP_STORED) as z:
            z.writestr("manifest.json", json.dumps(manifest, indent=2))
            z.writestr("app.bin", image)
            z.writestr("fs.img", fs)
    print(f"wrote {args.out} ({os.path.getsize(args.out)} bytes)")


if __name__ == "__main__":
    main()
