use anyhow::{anyhow, Context, Result};
use libcamera::{
    camera::CameraConfigurationStatus,
    camera_manager::CameraManager,
    framebuffer::AsFrameBuffer,
    framebuffer_allocator::{FrameBuffer, FrameBufferAllocator},
    pixel_format::PixelFormat,
    stream::StreamRole,
};
use tracing::{debug, info, warn};

// We removed the hardcoded override because the RPi PiSP requires native formats like YUV420 or RAW.

#[derive(Debug, Clone)]
pub struct CameraInfo {
    pub index: usize,
    pub id: String,
}

/// List every camera that libcamera can see.
pub fn list_cameras() -> Result<Vec<CameraInfo>> {
    let mgr = CameraManager::new().context("Failed to create CameraManager")?;
    let cameras = mgr.cameras();

    if cameras.is_empty() {
        warn!("No cameras found by libcamera");
    }

    let infos: Vec<CameraInfo> = cameras
        .iter()
        .enumerate()
        .map(|(i, cam)| {
            let id = cam.id().to_string();
            info!("Camera {}: {}", i, id);
            CameraInfo { index: i, id }
        })
        .collect();

    Ok(infos)
}

/// Capture a single MJPEG frame from the camera at `camera_index` and write
/// the raw JPEG bytes to `output_path`.
/// Returns the number of bytes written.
pub fn capture_frame(camera_index: usize, output_path: &str) -> Result<usize> {
    //  1. Initialise CameraManager 
    let mgr = CameraManager::new().context("Failed to create CameraManager")?;
    let cameras = mgr.cameras();

    let cam = cameras
        .get(camera_index)
        .ok_or_else(|| anyhow!("Camera index {} out of range", camera_index))?;

    info!("Using camera: {}", cam.id());

    //  2. Acquire exclusive access
    let mut cam = cam.acquire().context("Failed to acquire camera")?;

    //  3. Configure stream
    let mut cfgs = cam
        .generate_configuration(&[StreamRole::StillCapture])
        .context("Failed to generate camera configuration")?;

    // We do NOT call `stream_cfg.set_pixel_format()` here. We allow libcamera 
    // to pick the native format (usually YUV420 on PiSP) to prevent hardware assertions.

    match cfgs.validate() {
        CameraConfigurationStatus::Valid => info!("Camera configuration valid"),
        CameraConfigurationStatus::Adjusted => {
            warn!("Camera configuration was adjusted by libcamera");
        }
        CameraConfigurationStatus::Invalid => {
            return Err(anyhow!("Camera configuration is invalid"));
        }
    }

    cam.configure(&mut cfgs)
        .context("Failed to configure camera")?;

    info!(
        "Configured stream: {:?} {}x{}",
        cfgs.get(0).unwrap().get_pixel_format(),
        cfgs.get(0).unwrap().get_size().width,
        cfgs.get(0).unwrap().get_size().height,
    );

    //  4. Allocate buffers 
    let mut alloc = FrameBufferAllocator::new(&cam);
    let stream = cfgs.get(0).unwrap().stream().unwrap();
    let mut buffers = alloc.alloc(&stream).context("Failed to allocate buffers")?;

    info!("Allocated {} buffer(s)", buffers.len());

    let buffer = buffers.remove(0);
    let mapped = libcamera::framebuffer_map::MemoryMappedFrameBuffer::new(buffer).context("Failed to map buffer")?;

    //5. Create & queue a capture request 
    let mut request = cam.create_request(None).context("Failed to create request")?;
    request
        .add_buffer(&stream, mapped)
        .context("Failed to add buffer to request")?;

    // Set up a channel to receive completed requests
    let (tx, rx) = std::sync::mpsc::channel();
    cam.on_request_completed(move |req| {
        tx.send(req).ok();
    });

    cam.start(None).context("Failed to start camera")?;
    cam.queue_request(request)
        .map_err(|(_, e)| e)
        .context("Failed to queue request")?;

    info!("Waiting for capture to complete…");

    // 6. Wait for completion (5-second timeout)
    let completed = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .context("Timeout waiting for camera request")?;

    debug!("Request status: {:?}", completed.status());

    //7. Read raw bytes from the FrameBuffer
    let buf_ref: &libcamera::framebuffer_map::MemoryMappedFrameBuffer<FrameBuffer> = completed
        .buffer(&stream)
        .ok_or_else(|| anyhow!("No buffer in completed request"))?;

    let meta = buf_ref.metadata().context("No FrameBuffer metadata")?;
    let bytes_used = meta.planes().get(0).context("No planes in metadata")?.bytes_used as usize;
    info!("Frame bytes used: {}", bytes_used);

    let mapped_planes = buf_ref.data();
    let data = &mapped_planes[0][..bytes_used];

    // 8. Convert to Grayscale Image and write as JPEG
    let width = cfgs.get(0).unwrap().get_size().width;
    let height = cfgs.get(0).unwrap().get_size().height;

    info!("Encoding {}x{} Grayscale JPEG from raw pixels...", width, height);

    if let Some(img) = image::GrayImage::from_raw(width, height, data.to_vec()) {
        img.save(output_path).context("Failed to encode JPEG")?;
        info!("Successfully saved visible JPEG image to '{}'", output_path);
    } else {
        warn!("Failed to create Grayscale block. Saving raw bytes as fallback.");
        std::fs::write(output_path, data)
            .with_context(|| format!("Failed to write frame to {}", output_path))?;
    }

    cam.stop().ok();

    Ok(bytes_used)
}
