use anyhow::{Context, Result};
use std::process::Command;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct CameraInfo {
    pub index: usize,
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct FrameInfo {
    pub file_size: usize,
    pub width: u32,
    pub height: u32,
}

/// List cameras by asking rpicam-still. Returns one entry per detected camera.
pub fn list_cameras() -> Result<Vec<CameraInfo>> {
    let output = Command::new("rpicam-still")
        .args(["--list-cameras"])
        .output()
        .context("Failed to run rpicam-still --list-cameras (is rpicam-still installed?)")?;

    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);

    let mut cameras = Vec::new();
    for line in text.lines() {
        // Lines like: "0 : imx708 [4608x2592] ..."
        if let Some(rest) = line.trim().strip_prefix(|c: char| c.is_ascii_digit()) {
            let id = rest.trim().to_string();
            let index = cameras.len();
            info!("Camera {}: {}", index, id);
            cameras.push(CameraInfo { index, id });
        }
    }

    if cameras.is_empty() {
        warn!("No cameras found by rpicam-still");
    }

    Ok(cameras)
}

/// Capture a JPEG using rpicam-still and write it to `output_path`.
/// Returns file size and image dimensions.
pub fn capture_frame(camera_index: usize, output_path: &str) -> Result<FrameInfo> {
    info!("Capturing via rpicam-still → {}", output_path);

    let status = Command::new("rpicam-still")
        .args([
            "--camera", &camera_index.to_string(),
            "-o",       output_path,
            "-n",                       // no preview window
            "--width",  "1920",
            "--height", "1080",
            "--timeout","2000",         // 2 s for auto-exposure to settle
            "--quality","95",
        ])
        .status()
        .context("Failed to execute rpicam-still")?;

    if !status.success() {
        anyhow::bail!("rpicam-still exited with {}", status);
    }

    let file_size = std::fs::metadata(output_path)
        .with_context(|| format!("Image file not found after capture: {}", output_path))?
        .len() as usize;

    if file_size < 5_000 {
        anyhow::bail!(
            "Captured file is only {} bytes — looks invalid",
            file_size
        );
    }

    info!("Captured {} bytes → {}", file_size, output_path);

    Ok(FrameInfo { file_size, width: 1920, height: 1080 })
}
