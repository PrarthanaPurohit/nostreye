# nostreye

`nostreye` is a project that demonstrates capturing images from a Raspberry Pi camera and integrating them with the Nostr protocol. The goal is to securely capture images, encode them as JPEGs, and sign the corresponding Nostr events and frame data.

## Project Structure

- `nostreye-cam`: A Rust application using the `libcamera` crate to interface with the camera. It captures a frame, encodes it, and generates cryptographic signatures using a hardware-linked identity.
- `deploy.sh`: A shell script designed to easily deploy and build the `nostreye-cam` project from a development environment to a Raspberry Pi over SSH.

## Prerequisites

- For the Target Device (Raspberry Pi):
  - A compatible camera module.
  - Rust toolchain (`rustc`, `cargo`).
  - `libcamera` development headers.
  - `pkg-config` tool available.

## Deployment

You can deploy the application to your Raspberry Pi by running the `deploy.sh` script. Before running, edit the `RPI` variable in the script to match your device's SSH accessible address.

```bash
./deploy.sh
```

This will automatically create the remote structure, copy source files, and initiate a `cargo build`.

## Usage

Once successfully built on the Raspberry Pi, you can run the application directly:

```bash
cargo run
```

### Process Flow

The application executes several steps when run:
1. Detect available cameras using `libcamera`.
2. Capture a frame and save it as a JPEG at `/tmp/nostreye_capture.jpg`.
3. Initialise the hardware-linked device identity using the `device-signer` crate.
4. Construct and sign a Nostr text-note event with the image's meta-data.
5. Provide an ECDSA signature natively for the raw image frame data to ensure data integrity.
6. Verify the signatures locally and output the resulting Nostr event JSON.

## Core Libraries

The codebase depends on several modern Rust libraries:
- **libcamera**: Hardware camera interaction.
- **device-signer**: Provisioning device identity and signing primitives (Schnorr, ECDSA).
- **nostr**: Formatting Nostr protocol events and verification structures.
- **image**: Converting raw captured data to JPEG format.
- **tokio**: Asynchronous task scheduling.
