mod camera;
mod signer;

use anyhow::Result;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

const CAPTURE_PATH: &str = "/tmp/nostreye_capture.jpg";

#[tokio::main]
async fn main() -> Result<()> {
    // 0. Logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║       nostreye-cam  ·  RPi Camera + Nostr        ║");
    println!("╚══════════════════════════════════════════════════╝\n");

    // 1. List cameras
    let cameras = camera::list_cameras()?;

    if cameras.is_empty() {
        error!("No cameras found — make sure your camera is connected and libcamera is working");
        println!("\n[!] No cameras detected. Continuing with signing demo only.\n");
    } else {
        println!("┌─ Detected Cameras ─────────────────────────────────┐");
        for cam in &cameras {
            println!("│  [{:>2}]  {}", cam.index, cam.id);
        }
        println!("└────────────────────────────────────────────────────┘\n");
    }

    // 2. Capture a frame (only if a camera is available) 
    let frame_bytes_opt: Option<Vec<u8>> = if !cameras.is_empty() {
        info!("Capturing frame from camera 0 → {}", CAPTURE_PATH);
        match camera::capture_frame(0, CAPTURE_PATH) {
            Ok(n) => {
                println!("✓  Captured {} bytes → {}\n", n, CAPTURE_PATH);
                // Read back bytes for hashing
                std::fs::read(CAPTURE_PATH).ok()
            }
            Err(e) => {
                error!("Capture failed: {:#}", e);
                println!("[!] Capture failed: {}\n", e);
                None
            }
        }
    } else {
        None
    };

    //3. Initialise device-signer 
    info!("Initialising hardware-linked DeviceIdentity…");
    // Pass the camera ID (if any) as additional entropy
    let camera_label = cameras
        .first()
        .map(|c| format!("cam-{}", &c.id.chars().take(12).collect::<String>()));
    let signer = signer::NostreYeSigner::new(camera_label)?;

    println!("┌─ Device Identity ──────────────────────────────────┐");
    println!("│  npub   : {}", signer.npub());
    println!("│  pubkey : {}", signer.pubkey_hex());
    println!("└────────────────────────────────────────────────────┘\n");

    // 4. Build Nostr event content with camera metadata
    let camera_model = cameras.first().map(|c| c.id.clone()).unwrap_or_else(|| "no-camera".to_string());
    let capture_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let content = format!(
        "📷 nostreye capture — camera: {camera_model} — frame_path: {CAPTURE_PATH} — ts: {capture_ts}"
    );
    info!("Event content: {}", content);

    // 5. Sign the Nostr event (Schnorr / NIP-01)
    info!("Signing Nostr text-note event…");
    let event = signer.sign_text_note(&content, vec![])?;

    println!("┌─ Signed Nostr Event (NIP-01) ──────────────────────┐");
    println!("│  kind       : {}", event.kind);
    println!("│  event_id   : {}", event.id);
    println!("│  sig (16B)  : {}…", &event.sig[..16]);
    println!("└────────────────────────────────────────────────────┘\n");

    // 6. Sign the frame with ECDSA (integrity attestation)
    if let Some(data) = &frame_bytes_opt {
        info!("Computing ECDSA integrity signature over captured frame…");
        match signer.sign_frame_hash(data) {
            Ok(ecdsa_sig) => {
                println!("┌─ Frame Integrity (ECDSA) ───────────────────────────┐");
                println!("│  ECDSA sig : {}…", &ecdsa_sig[..32.min(ecdsa_sig.len())]);
                println!("└────────────────────────────────────────────────────┘\n");
            }
            Err(e) => {
                error!("ECDSA signing error: {:#}", e);
            }
        }
    }

    // 7. Verify the signed event locally
    info!("Verifying Schnorr signature on signed event…");
    match signer::NostreYeSigner::verify_event(&event) {
        Ok(true) => {
            println!("✓  Signature verified: VALID\n");
        }
        Ok(false) => {
            println!("✗  Signature verified: INVALID\n");
        }
        Err(e) => {
            println!("[!] Verification error: {}\n", e);
        }
    }

    // 8. Print the full event JSON 
    println!("┌─ Full Event JSON ───────────────────────────────────┐");
    println!("{}", event.json);
    println!("└────────────────────────────────────────────────────┘\n");

    info!("nostreye-cam demo complete.");
    Ok(())
}
