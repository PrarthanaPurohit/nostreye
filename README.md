# nostreye

`nostreye` captures images from a Raspberry Pi camera and integrates them with the Nostr protocol. It securely captures images, encodes them as JPEGs, signs events and frame data, and publishes them to Nostr relays.

## Project Structure

- `nostreye-cam`: A Rust application using the `libcamera` crate to interface with the camera. It captures a frame, converts YUV formats (YU12, NV12, etc.) to JPEG, generates cryptographic signatures via hardware-linked identity (`NostreyeSigner`), and publishes the image to Nostr via Blossom and relays.
- `deploy.sh`: Deploys and builds `nostreye-cam` from your development environment to a Raspberry Pi over SSH.

## Prerequisites

**Target device (Raspberry Pi):**
- Compatible camera module (e.g. IMX708 on RPi 5)
- Rust toolchain (`rustc`, `cargo`)
- `libcamera` development headers
- `pkg-config`

Network connectivity is required for publishing images to Nostr.

## Deployment

Edit the `RPI` variable in `deploy.sh` to match your device's SSH address, then:

```bash
./deploy.sh
```

This copies source files and runs `cargo build` on the Pi.

## Usage

On the Raspberry Pi:

```bash
cargo run
```

### Process flow

1. **Detect cameras** — Lists available cameras via `libcamera`.
2. **Capture** — Captures a frame and saves a valid JPEG at `/tmp/nostreye_capture.jpg`. Supports YU12/I420, YV12, NV12, NV21, YUYV, RG24, and MJPEG.
3. **Device identity** — Initialises hardware-linked identity (secp256k1) using `device-signer`.
4. **Profile (kind 0)** — Signs and broadcasts a metadata event so your npub shows a profile (name, display_name, about) across Nostr clients. Sent first so relays have the profile before any other events.
5. **Frame integrity** — Computes ECDSA signature over the JPEG bytes (attestation).
6. **Publish** — Uploads the image to Blossom (BUD-01 auth), then broadcasts:
   - **Kind 1** — Text note with the image URL (visible in Damus, Primal, Snort, etc.).
   - **Kind 1063** — NIP-94 file-metadata event with URL, SHA256, dimensions, and ECDSA attestation.
   Relays: `relay.damus.io`, `nos.lol`, `relay.primal.net`, `relay.snort.social`, `nostr.mom`.

### Viewing the captured image

Copy from the Pi to your machine:

```bash
scp user@<rpi-ip>:/tmp/nostreye_capture.jpg .
```

### Viewing in Nostr clients

Add your npub (printed at startup) to Damus, Primal, Snort, or any Nostr client. The profile (kind 0) and image posts (kind 1 with URL) will appear in your feed and on your profile page.

### Keys and nsec

The device uses `device-signer` to derive a deterministic secp256k1 key from hardware entropy (CPU serial, MAC, machine ID) and a persisted salt. The secret is derived on demand for signing and **is not exported as nsec** — this is by design to avoid leaking the key.

**What you can see:**
- **npub** — Printed at startup (`Device Identity`). Use this to follow your device from any Nostr client.
- **pubkey (hex)** — Same key in hex; useful for relay filters and event lookups.

**If you need nsec** (e.g. to import into Damus, Primal, or another client):
- `device-signer` does not expose the raw secret. The key lives only in memory during signing.
- Options: extend [device-signer](https://github.com/prarthanapurohit/device-signer) to add an `nsec()` or `export_secret()` method (requires changing the crate’s API and accepting the security tradeoff), or use a separate software-backed key for Nostr and keep the hardware identity only for attestation.

## Core libraries

- **libcamera** — Hardware camera interaction.
- **device-signer** — Hardware-linked identity; Schnorr and ECDSA signing.
- **nostr** — Event building and verification.
- **image** — YUV→RGB conversion and JPEG encoding.
- **reqwest** — HTTP client (Blossom upload).
- **tokio-tungstenite** — WebSocket client (relay publish).
- **tokio** — Async runtime.
