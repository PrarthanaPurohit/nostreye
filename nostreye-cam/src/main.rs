mod camera;
mod publisher;
mod signer;

use anyhow::Result;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

const CAPTURE_PATH: &str = "/tmp/nostreye_capture.jpg";

#[tokio::main]
async fn main() -> Result<()> {
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

    // 2. Capture a frame
    let frame_info_opt = if !cameras.is_empty() {
        info!("Capturing frame from camera 0 → {}", CAPTURE_PATH);
        match camera::capture_frame(0, CAPTURE_PATH) {
            Ok(fi) => {
                println!(
                    "✓  Captured {} bytes ({}x{}) → {}\n",
                    fi.file_size, fi.width, fi.height, CAPTURE_PATH
                );
                Some(fi)
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

    // Read back JPEG bytes for signing / publishing
    let frame_bytes_opt: Option<Vec<u8>> = frame_info_opt
        .as_ref()
        .and_then(|_| std::fs::read(CAPTURE_PATH).ok());

    // 3. Initialise device-signer
    info!("Initialising hardware-linked DeviceIdentity…");
    let camera_label = cameras
        .first()
        .map(|c| format!("cam-{}", &c.id.chars().take(12).collect::<String>()));
    let signer = signer::NostreyeSigner::new(camera_label)?;

    println!("┌─ Device Identity ──────────────────────────────────┐");
    println!("│  npub   : {}", signer.npub());
    println!("│  pubkey : {}", signer.pubkey_hex());
    println!("└────────────────────────────────────────────────────┘\n");

    // 4. Publish profile (kind 0) so npub is visible across clients
    info!("Publishing profile (kind 0) to relays…");
    let profile = signer.sign_metadata(
        "nostreye",
        "Nostreye Camera",
        "RPi camera captures signed and published to Nostr",
        "",
    )?;
    let profile_results = publisher::broadcast_event(&profile, publisher::RELAYS).await;
    let ok_count = profile_results.iter().filter(|(_, a)| *a).count();
    println!("┌─ Profile (kind 0) ───────────────────────────────────┐");
    println!("│  Broadcast to {} relays: {} accepted", profile_results.len(), ok_count);
    println!("└────────────────────────────────────────────────────┘\n");

    // 5. ECDSA frame integrity signature
    let ecdsa_sig_opt = if let Some(data) = &frame_bytes_opt {
        info!("Computing ECDSA integrity signature over captured frame…");
        match signer.sign_frame_hash(data) {
            Ok(sig) => {
                println!("┌─ Frame Integrity (ECDSA) ───────────────────────────┐");
                println!("│  ECDSA sig : {}…", &sig[..32.min(sig.len())]);
                println!("└────────────────────────────────────────────────────┘\n");
                Some(sig)
            }
            Err(e) => {
                error!("ECDSA signing error: {:#}", e);
                None
            }
        }
    } else {
        None
    };

    // 6. Publish image to Nostr (Blossom upload + kind 1 + kind 1063)
    if let (Some(jpeg), Some(ecdsa_sig), Some(fi)) =
        (&frame_bytes_opt, &ecdsa_sig_opt, &frame_info_opt)
    {
        println!("┌─ Publishing to Nostr ───────────────────────────────┐");
        match publisher::publish_image(
            jpeg,
            ecdsa_sig,
            fi.width,
            fi.height,
            &signer,
            publisher::BLOSSOM_SERVER,
            publisher::RELAYS,
        )
        .await
        {
            Ok(result) => {
                println!("│  Image URL : {}", result.image_url);
                for (relay, ok) in &result.relay_results {
                    let status = if *ok { "✓ accepted" } else { "✗ rejected" };
                    println!("│  {:14} {}", status, relay);
                }
            }
            Err(e) => {
                error!("Publish failed: {:#}", e);
                println!("│  [!] Publish failed: {}", e);
            }
        }
        println!("└────────────────────────────────────────────────────┘\n");
    }

    info!("nostreye-cam demo complete.");
    Ok(())
}
