use anyhow::{anyhow, Context, Result};
use image::RgbImage;
use libcamera::{
    camera::CameraConfigurationStatus,
    camera_manager::CameraManager,
    framebuffer::AsFrameBuffer,
    framebuffer_allocator::{FrameBuffer, FrameBufferAllocator},
    stream::StreamRole,
};
use tracing::{debug, info, warn};

// DRM / V4L2-style fourcc codes (little-endian)
const FOURCC_NV12: u32 = u32::from_le_bytes(*b"NV12");
const FOURCC_NV21: u32 = u32::from_le_bytes(*b"NV21");
const FOURCC_MJPG: u32 = u32::from_le_bytes(*b"MJPG");
const FOURCC_YUYV: u32 = u32::from_le_bytes(*b"YUYV");
const FOURCC_RG24: u32 = u32::from_le_bytes(*b"RG24"); // DRM_FORMAT_RGB888
// I420 / YU12 — planar YUV 4:2:0: Y plane, then U plane, then V plane
const FOURCC_YU12: u32 = u32::from_le_bytes(*b"YU12");
const FOURCC_I420: u32 = u32::from_le_bytes(*b"I420");
// YV12 — same as I420 but U/V swapped
const FOURCC_YV12: u32 = u32::from_le_bytes(*b"YV12");

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

#[inline]
fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// BT.601 limited range (typical for camera YUV → display RGB).
fn yuv_to_rgb_bt601(y: u8, u: u8, v: u8) -> [u8; 3] {
    let y = y as i32 - 16;
    let u = u as i32 - 128;
    let v = v as i32 - 128;
    let r = (298 * y + 409 * v + 128) >> 8;
    let g = (298 * y - 100 * u - 208 * v + 128) >> 8;
    let b = (298 * y + 516 * u + 128) >> 8;
    [clamp_u8(r), clamp_u8(g), clamp_u8(b)]
}

fn nv12_to_rgb(
    width: u32,
    height: u32,
    y_stride: u32,
    y_plane: &[u8],
    uv_plane: &[u8],
    swap_uv: bool,
) -> Option<RgbImage> {
    let w = width as usize;
    let h = height as usize;
    let ys = y_stride as usize;
    if w == 0 || h == 0 || ys < w {
        return None;
    }
    let uv_rows = h / 2;
    let uv_stride = if uv_rows > 0 {
        uv_plane.len() / uv_rows
    } else {
        w
    };
    if uv_stride < w {
        return None;
    }

    let mut buf = vec![0u8; w * h * 3];
    let mut i = 0usize;
    for y in 0..h {
        let row_off = y * ys;
        let cy = y / 2;
        let uv_row_off = cy * uv_stride;
        for x in 0..w {
            let yi = y_plane.get(row_off + x).copied()?;
            let cx = (x / 2) * 2;
            let (u, v) = if swap_uv {
                let v0 = *uv_plane.get(uv_row_off + cx + 1)?;
                let u0 = *uv_plane.get(uv_row_off + cx)?;
                (u0, v0)
            } else {
                let u0 = *uv_plane.get(uv_row_off + cx)?;
                let v0 = *uv_plane.get(uv_row_off + cx + 1)?;
                (u0, v0)
            };
            let rgb = yuv_to_rgb_bt601(yi, u, v);
            buf[i] = rgb[0];
            buf[i + 1] = rgb[1];
            buf[i + 2] = rgb[2];
            i += 3;
        }
    }
    RgbImage::from_raw(width, height, buf)
}

/// Single-plane NV12: Y with `stride` × height, then UV with `stride` × (height/2).
fn nv12_single_plane(
    width: u32,
    height: u32,
    stride: u32,
    data: &[u8],
    swap_uv: bool,
) -> Option<RgbImage> {
    let h = height as usize;
    let ys = stride as usize;
    let y_size = ys * h;
    let uv_size = ys * (h / 2);
    if data.len() < y_size + uv_size {
        return None;
    }
    let y_plane = &data[..y_size];
    let uv_plane = &data[y_size..y_size + uv_size];
    nv12_to_rgb(width, height, stride, y_plane, uv_plane, swap_uv)
}

fn yuyv_to_rgb(width: u32, height: u32, stride: u32, data: &[u8]) -> Option<RgbImage> {
    let w = width as usize;
    let h = height as usize;
    let row_bytes = stride as usize;
    if row_bytes < w * 2 || h == 0 {
        return None;
    }
    let mut buf = vec![0u8; w * h * 3];
    let mut o = 0usize;
    for y in 0..h {
        let row = y * row_bytes;
        for x in (0..w).step_by(2) {
            let base = row + x * 2;
            let y0 = *data.get(base)?;
            let u = *data.get(base + 1)?;
            let y1 = *data.get(base + 2)?;
            let v = *data.get(base + 3)?;
            let p0 = yuv_to_rgb_bt601(y0, u, v);
            buf[o..o + 3].copy_from_slice(&p0);
            o += 3;
            if x + 1 < w {
                let p1 = yuv_to_rgb_bt601(y1, u, v);
                buf[o..o + 3].copy_from_slice(&p1);
                o += 3;
            }
        }
    }
    RgbImage::from_raw(width, height, buf)
}

fn rgb888_packed(width: u32, height: u32, stride: u32, data: &[u8]) -> Option<RgbImage> {
    let w = width as usize;
    let h = height as usize;
    let row_bytes = stride as usize;
    if row_bytes < w * 3 || h == 0 {
        return None;
    }
    let mut buf = vec![0u8; w * h * 3];
    for y in 0..h {
        let src_row = y * row_bytes;
        let dst_row = y * w * 3;
        for x in 0..w {
            let s = src_row + x * 3;
            let d = dst_row + x * 3;
            buf[d] = *data.get(s)?;
            buf[d + 1] = *data.get(s + 1)?;
            buf[d + 2] = *data.get(s + 2)?;
        }
    }
    RgbImage::from_raw(width, height, buf)
}

/// I420 / YU12: three separate planes — Y (stride × h), U (uv_stride × h/2), V (uv_stride × h/2).
/// When `swap_uv` is true the U and V planes are exchanged (YV12).
/// Handles both multi-plane (3 slices) and single contiguous buffer layouts.
fn i420_to_rgb(
    width: u32,
    height: u32,
    y_stride: u32,
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
) -> Option<RgbImage> {
    let w = width as usize;
    let h = height as usize;
    let ys = y_stride as usize;
    if w == 0 || h == 0 || ys < w {
        return None;
    }
    let uv_h = h / 2;
    let uv_stride = if uv_h > 0 && u_plane.len() >= uv_h {
        u_plane.len() / uv_h
    } else {
        (w + 1) / 2
    };

    let mut buf = vec![0u8; w * h * 3];
    let mut out = 0usize;
    for row in 0..h {
        let y_row = row * ys;
        let uv_row = (row / 2) * uv_stride;
        for col in 0..w {
            let yi = *y_plane.get(y_row + col)?;
            let uv_col = col / 2;
            let u = *u_plane.get(uv_row + uv_col)?;
            let v = *v_plane.get(uv_row + uv_col)?;
            let rgb = yuv_to_rgb_bt601(yi, u, v);
            buf[out] = rgb[0];
            buf[out + 1] = rgb[1];
            buf[out + 2] = rgb[2];
            out += 3;
        }
    }
    RgbImage::from_raw(width, height, buf)
}

fn i420_single_plane(width: u32, height: u32, stride: u32, data: &[u8], swap_uv: bool) -> Option<RgbImage> {
    let h = height as usize;
    let ys = stride as usize;
    let uv_stride = ((stride as usize) + 1) / 2;
    let y_size = ys * h;
    let uv_size = uv_stride * (h / 2);
    if data.len() < y_size + 2 * uv_size {
        return None;
    }
    let y_plane = &data[..y_size];
    let u_start = y_size;
    let v_start = y_size + uv_size;
    let (u_plane, v_plane) = if swap_uv {
        (&data[v_start..v_start + uv_size], &data[u_start..u_start + uv_size])
    } else {
        (&data[u_start..u_start + uv_size], &data[v_start..v_start + uv_size])
    };
    i420_to_rgb(width, height, stride, y_plane, u_plane, v_plane)
}

fn is_jpeg_magic(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xff && data[1] == 0xd8
}

/// Decode the captured buffer according to `fourcc` and stream geometry, then save as JPEG.
fn write_buffer_as_jpeg(
    fourcc: u32,
    width: u32,
    height: u32,
    stride: u32,
    plane_data: &[&[u8]],
    output_path: &str,
) -> Result<()> {
    let first_plane = plane_data.get(0).copied().unwrap_or(&[]);

    // Hardware MJPEG or already-encoded JPEG
    if fourcc == FOURCC_MJPG || is_jpeg_magic(first_plane) {
        let raw: Vec<u8> = plane_data.iter().flat_map(|s| s.iter().copied()).collect();
        let slice = if is_jpeg_magic(&raw) {
            raw.as_slice()
        } else {
            return Err(anyhow!(
                "MJPEG fourcc but buffer does not start with JPEG SOI (0xFF 0xD8)"
            ));
        };
        std::fs::write(output_path, slice).with_context(|| format!("write {}", output_path))?;
        return Ok(());
    }

    let stride_eff = if stride > 0 { stride } else { width };

    let img = match fourcc {
        FOURCC_NV12 => {
            if plane_data.len() >= 2 {
                nv12_to_rgb(
                    width,
                    height,
                    stride_eff,
                    plane_data[0],
                    plane_data[1],
                    false,
                )
            } else {
                nv12_single_plane(width, height, stride_eff, plane_data[0], false)
            }
        }
        FOURCC_NV21 => {
            if plane_data.len() >= 2 {
                nv12_to_rgb(
                    width,
                    height,
                    stride_eff,
                    plane_data[0],
                    plane_data[1],
                    true,
                )
            } else {
                nv12_single_plane(width, height, stride_eff, plane_data[0], true)
            }
        }
        FOURCC_YU12 | FOURCC_I420 => {
            if plane_data.len() >= 3 {
                i420_to_rgb(width, height, stride_eff, plane_data[0], plane_data[1], plane_data[2])
            } else {
                i420_single_plane(width, height, stride_eff, plane_data[0], false)
            }
        }
        FOURCC_YV12 => {
            if plane_data.len() >= 3 {
                i420_to_rgb(width, height, stride_eff, plane_data[0], plane_data[2], plane_data[1])
            } else {
                i420_single_plane(width, height, stride_eff, plane_data[0], true)
            }
        }
        FOURCC_YUYV => yuyv_to_rgb(width, height, stride_eff, plane_data[0]),
        FOURCC_RG24 => rgb888_packed(width, height, stride_eff, plane_data[0]),
        _ => {
            return Err(anyhow!(
                "Unsupported pixel format fourcc 0x{:08x} — extend camera.rs for this format",
                fourcc
            ));
        }
    };

    let img = img.ok_or_else(|| {
        anyhow!(
            "Failed to convert frame (fourcc 0x{:08x}, {} planes, {}×{}, stride {})",
            fourcc,
            plane_data.len(),
            width,
            height,
            stride_eff
        )
    })?;

    img.save(output_path)
        .with_context(|| format!("Failed to encode JPEG to {}", output_path))?;
    Ok(())
}

/// Capture a single frame from the camera at `camera_index` and write a valid JPEG to `output_path`.
/// Returns the number of bytes written to the file.
pub fn capture_frame(camera_index: usize, output_path: &str) -> Result<FrameInfo> {
    let mgr = CameraManager::new().context("Failed to create CameraManager")?;
    let cameras = mgr.cameras();

    let cam = cameras
        .get(camera_index)
        .ok_or_else(|| anyhow!("Camera index {} out of range", camera_index))?;

    info!("Using camera: {}", cam.id());

    let mut cam = cam.acquire().context("Failed to acquire camera")?;

    let mut cfgs = cam
        .generate_configuration(&[StreamRole::StillCapture])
        .context("Failed to generate camera configuration")?;

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

    let stream_cfg = cfgs.get(0).context("No stream in camera configuration")?;
    let width = stream_cfg.get_size().width;
    let height = stream_cfg.get_size().height;
    let stride = stream_cfg.get_stride();
    let pixel_format = stream_cfg.get_pixel_format();
    let fourcc = pixel_format.fourcc();

    info!(
        "Configured stream: {:?} {}x{} stride={}",
        pixel_format,
        width,
        height,
        stride
    );

    let mut alloc = FrameBufferAllocator::new(&cam);
    let stream = stream_cfg.stream().context("No stream after configure")?;
    let mut buffers = alloc.alloc(&stream).context("Failed to allocate buffers")?;

    info!("Allocated {} buffer(s)", buffers.len());

    let buffer = buffers.remove(0);
    let mapped = libcamera::framebuffer_map::MemoryMappedFrameBuffer::new(buffer)
        .context("Failed to map buffer")?;

    let mut request = cam.create_request(None).context("Failed to create request")?;
    request
        .add_buffer(&stream, mapped)
        .context("Failed to add buffer to request")?;

    let (tx, rx) = std::sync::mpsc::channel();
    cam.on_request_completed(move |req| {
        tx.send(req).ok();
    });

    cam.start(None).context("Failed to start camera")?;
    cam.queue_request(request)
        .map_err(|(_, e)| e)
        .context("Failed to queue request")?;

    info!("Waiting for capture to complete…");

    let completed = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .context("Timeout waiting for camera request")?;

    debug!("Request status: {:?}", completed.status());

    let buf_ref: &libcamera::framebuffer_map::MemoryMappedFrameBuffer<FrameBuffer> = completed
        .buffer(&stream)
        .ok_or_else(|| anyhow!("No buffer in completed request"))?;

    let meta = buf_ref.metadata().context("No FrameBuffer metadata")?;
    let mapped_planes = buf_ref.data();

    let meta_planes = meta.planes();
    let num_planes = meta_planes.len();
    let mut plane_slices: Vec<&[u8]> = Vec::with_capacity(num_planes);
    for i in 0..num_planes {
        let m = mapped_planes.get(i).copied().unwrap_or(&[]);
        let n = meta_planes.get(i).map(|p| p.bytes_used as usize).unwrap_or(0);
        let end = if n > 0 && n <= m.len() { n } else { m.len() };
        plane_slices.push(&m[..end]);
    }

    let bytes_in_frame: usize = plane_slices.iter().map(|s: &&[u8]| s.len()).sum();

    write_buffer_as_jpeg(fourcc, width, height, stride, &plane_slices, output_path)?;

    cam.stop().ok();

    let file_len = std::fs::metadata(output_path)
        .map(|m| m.len() as usize)
        .unwrap_or(bytes_in_frame);

    info!("Saved JPEG to '{}'", output_path);

    Ok(FrameInfo { file_size: file_len, width, height })
}
