mod camera;
mod publisher;
mod signer;

use anyhow::Result;
use std::io::Write;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use tokio::io::{AsyncBufReadExt, BufReader};

const CAPTURE_PATH: &str = "/tmp/nostreye_capture.jpg";
const PROFILE_SENT_FLAG: &str = "/home/prarthana/.hardware_identity/.profile_published";

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

    let cameras = camera::list_cameras()?;

    if cameras.is_empty() {
        error!("No cameras found — make sure your camera is connected and rpicam-still works");
        println!("\n[!] No cameras detected. You can still use this session for identity only.\n");
    } else {
        println!("┌─ Detected Cameras ─────────────────────────────────┐");
        for cam in &cameras {
            println!("│  [{:>2}]  {}", cam.index, cam.id);
        }
        println!("└────────────────────────────────────────────────────┘\n");
    }

    info!("Initialising hardware-linked DeviceIdentity…");
    let camera_label = cameras
        .first()
        .map(|c| format!("cam-{}", &c.id.chars().take(12).collect::<String>()));
    let signer = signer::NostreyeSigner::new(camera_label)?;

    println!("┌─ Device Identity ──────────────────────────────────┐");
    println!("│  npub   : {}", signer.npub());
    println!("│  pubkey : {}", signer.pubkey_hex());
    println!("└────────────────────────────────────────────────────┘\n");

    if std::path::Path::new(PROFILE_SENT_FLAG).exists() {
        println!("┌─ Profile (kind 0) ───────────────────────────────────┐");
        println!("│  Already published on first run — skipping.");
        println!("└────────────────────────────────────────────────────┘\n");
    } else {
        info!("First run — publishing profile (kind 0) to relays…");
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
        if ok_count > 0 {
            let _ = std::fs::write(PROFILE_SENT_FLAG, profile_results.len().to_string());
        }
    }

    println!("Commands:  capture | snap | photo  — take a picture and publish to Nostr");
    println!("           help                    — show this again");
    println!("           quit | exit             — leave\n");

    let camera_index = cameras.first().map(|c| c.index).unwrap_or(0);
    let have_camera = !cameras.is_empty();

    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut line = String::new();

    loop {
        print!("nostreye> ");
        std::io::stdout().flush()?;

        line.clear();
        let n = stdin.read_line(&mut line).await?;
        if n == 0 {
            println!();
            break;
        }

        let cmd = line.trim();
        if cmd.is_empty() {
            continue;
        }

        let lower = cmd.to_ascii_lowercase();
        match lower.as_str() {
            "quit" | "exit" | "q" => {
                println!("Goodbye.");
                break;
            }
            "help" | "?" | "h" => {
                println!("  capture | snap | photo  — capture via rpicam-still and publish");
                println!("  help                    — this text");
                println!("  quit | exit             — exit\n");
            }
            "capture" | "snap" | "photo" | "shot" => {
                if !have_camera {
                    println!("[!] No camera — cannot capture.\n");
                    continue;
                }
                if let Err(e) = capture_and_publish(&signer, camera_index).await {
                    error!("Capture/publish failed: {:#}", e);
                    println!("[!] {}\n", e);
                }
            }
            other => {
                println!("Unknown command {:?}. Type help for commands.\n", other);
            }
        }
    }

    info!("nostreye-cam session ended.");
    Ok(())
}

async fn capture_and_publish(signer: &signer::NostreyeSigner, camera_index: usize) -> Result<()> {
    info!("Capturing frame → {}", CAPTURE_PATH);
    let fi = camera::capture_frame(camera_index, CAPTURE_PATH)?;
    println!(
        "✓  Captured {} bytes ({}x{}) → {}",
        fi.file_size, fi.width, fi.height, CAPTURE_PATH
    );

    let jpeg = std::fs::read(CAPTURE_PATH)?;
    info!("Computing ECDSA integrity signature…");
    let ecdsa_sig = signer.sign_frame_hash(&jpeg)?;
    println!("│  ECDSA sig : {}…", &ecdsa_sig[..32.min(ecdsa_sig.len())]);

    println!("┌─ Publishing to Nostr ───────────────────────────────┐");
    match publisher::publish_image(
        &jpeg,
        &ecdsa_sig,
        fi.width,
        fi.height,
        signer,
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

    Ok(())
}
