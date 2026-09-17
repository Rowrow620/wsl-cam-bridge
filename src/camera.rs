use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    CameraIndex, RequestedFormat, RequestedFormatType, Resolution,
};
use nokhwa::Camera;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct CameraDeviceInfo {
    pub index: u32,
    pub name: String,
    pub description: String,
}

#[derive(Clone)]
pub struct FrameState {
    pub jpeg: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub frame_count: u64,
    pub fps: f32,
    pub camera_name: String,
}

impl Default for FrameState {
    fn default() -> Self {
        Self {
            jpeg: Arc::new(Vec::new()),
            width: 0,
            height: 0,
            frame_count: 0,
            fps: 0.0,
            camera_name: String::from("Waiting for camera..."),
        }
    }
}

pub struct CameraService {
    frame_state: Arc<RwLock<FrameState>>,
    running: Arc<AtomicBool>,
    target_camera_idx: Arc<AtomicU32>,
    target_width: Arc<AtomicU32>,
    target_height: Arc<AtomicU32>,
    worker_handle: Option<JoinHandle<()>>,
}

impl CameraService {
    pub fn new() -> Self {
        Self {
            frame_state: Arc::new(RwLock::new(FrameState::default())),
            running: Arc::new(AtomicBool::new(false)),
            target_camera_idx: Arc::new(AtomicU32::new(0)),
            target_width: Arc::new(AtomicU32::new(1280)),
            target_height: Arc::new(AtomicU32::new(720)),
            worker_handle: None,
        }
    }

    pub fn get_frame_state(&self) -> Arc<RwLock<FrameState>> {
        Arc::clone(&self.frame_state)
    }

    pub fn list_devices() -> Vec<CameraDeviceInfo> {
        let backend = match nokhwa::native_api_backend() {
            Some(b) => b,
            None => {
                eprintln!("[Camera] No native camera API backend available on this platform.");
                return Vec::new();
            }
        };

        match nokhwa::query(backend) {
            Ok(devices) => devices
                .into_iter()
                .enumerate()
                .map(|(i, dev)| CameraDeviceInfo {
                    index: match dev.index() {
                        CameraIndex::Index(idx) => *idx,
                        CameraIndex::String(_) => i as u32,
                    },
                    name: dev.human_name(),
                    description: dev.description().to_string(),
                })
                .collect(),
            Err(e) => {
                eprintln!("[Camera] Failed to enumerate devices: {e}");
                Vec::new()
            }
        }
    }

    pub fn set_camera_index(&self, idx: u32) {
        self.target_camera_idx.store(idx, Ordering::SeqCst);
    }

    pub fn set_resolution(&self, width: u32, height: u32) {
        self.target_width.store(width, Ordering::SeqCst);
        self.target_height.store(height, Ordering::SeqCst);
    }

    pub fn start(&mut self) {
        if self.running.load(Ordering::SeqCst) {
            return;
        }

        self.running.store(true, Ordering::SeqCst);
        let running = Arc::clone(&self.running);
        let frame_state = Arc::clone(&self.frame_state);
        let target_camera_idx = Arc::clone(&self.target_camera_idx);
        let target_width = Arc::clone(&self.target_width);
        let target_height = Arc::clone(&self.target_height);

        let handle = thread::spawn(move || {
            let mut current_idx = u32::MAX;
            let mut current_w = 0;
            let mut current_h = 0;
            let mut camera_opt: Option<Camera> = None;
            let mut cam_name = String::from("Camera");

            let mut frame_count: u64 = 0;
            let mut fps_counter: u32 = 0;
            let mut last_fps_check = Instant::now();
            let mut last_device_retry = Instant::now() - Duration::from_secs(10);
            let mut current_fps = 0.0f32;

            while running.load(Ordering::SeqCst) {
                let req_idx = target_camera_idx.load(Ordering::SeqCst);
                let req_w = target_width.load(Ordering::SeqCst);
                let req_h = target_height.load(Ordering::SeqCst);

                // Check if device or resolution changed, or if we need to attempt finding a camera
                let should_try_open = camera_opt.is_none() && last_device_retry.elapsed() >= Duration::from_millis(2000);
                let config_changed = req_idx != current_idx || req_w != current_w || req_h != current_h;

                if (should_try_open || config_changed) && running.load(Ordering::SeqCst) {
                    last_device_retry = Instant::now();
                    if let Some(mut old_cam) = camera_opt.take() {
                        let _ = old_cam.stop_stream();
                    }

                    let res = Resolution::new(req_w, req_h);
                    let req_format = RequestedFormat::new::<RgbFormat>(
                        RequestedFormatType::Closest(nokhwa::utils::CameraFormat::new(
                            res,
                            nokhwa::utils::FrameFormat::MJPEG,
                            30,
                        )),
                    );

                    match Camera::new(CameraIndex::Index(req_idx), req_format) {
                        Ok(mut cam) => {
                            cam_name = cam.info().human_name();
                            match cam.open_stream() {
                                Ok(()) => {
                                    println!("[Camera] Stream opened successfully for '{cam_name}'");
                                    camera_opt = Some(cam);
                                    current_idx = req_idx;
                                    current_w = req_w;
                                    current_h = req_h;
                                }
                                Err(e) => {
                                    eprintln!("[Camera] Failed to open stream: {e}");
                                }
                            }
                        }
                        Err(_) => {
                            // Camera not available yet, will use test pattern
                        }
                    }
                }

                // If physical camera is active, read from it
                if let Some(cam) = camera_opt.as_mut() {
                    match cam.frame() {
                        Ok(frame) => {
                            if let Ok(rgb_img) = frame.decode_image::<RgbFormat>() {
                                let (width, height) = (rgb_img.width(), rgb_img.height());
                                let raw_bytes = rgb_img.as_raw();

                                let mut jpeg_buf = Vec::with_capacity((width * height) as usize / 4);
                                let mut cursor = Cursor::new(&mut jpeg_buf);
                                let encoder = JpegEncoder::new_with_quality(&mut cursor, 75);

                                if encoder
                                    .write_image(raw_bytes, width, height, ExtendedColorType::Rgb8)
                                    .is_ok()
                                {
                                    frame_count += 1;
                                    fps_counter += 1;

                                    let elapsed = last_fps_check.elapsed();
                                    if elapsed >= Duration::from_millis(1000) {
                                        current_fps = (fps_counter as f32) / elapsed.as_secs_f32();
                                        fps_counter = 0;
                                        last_fps_check = Instant::now();
                                    }

                                    if let Ok(mut state) = frame_state.write() {
                                        state.jpeg = Arc::new(jpeg_buf);
                                        state.width = width;
                                        state.height = height;
                                        state.frame_count = frame_count;
                                        state.fps = current_fps;
                                        state.camera_name = cam_name.clone();
                                    }
                                    continue;
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[Camera] Frame capture error: {e}");
                            camera_opt = None; // Reset camera on failure
                        }
                    }
                }

                // Fallback: Generate smooth 30 FPS animated test pattern
                let width = req_w;
                let height = req_h;
                let raw_rgb = generate_test_card(width, height, frame_count);

                let mut jpeg_buf = Vec::with_capacity((width * height) as usize / 4);
                let mut cursor = Cursor::new(&mut jpeg_buf);
                let encoder = JpegEncoder::new_with_quality(&mut cursor, 75);
                let _ = encoder.write_image(&raw_rgb, width, height, ExtendedColorType::Rgb8);

                frame_count += 1;
                fps_counter += 1;

                let elapsed = last_fps_check.elapsed();
                if elapsed >= Duration::from_millis(1000) {
                    current_fps = (fps_counter as f32) / elapsed.as_secs_f32();
                    fps_counter = 0;
                    last_fps_check = Instant::now();
                }

                if let Ok(mut state) = frame_state.write() {
                    state.jpeg = Arc::new(jpeg_buf);
                    state.width = width;
                    state.height = height;
                    state.frame_count = frame_count;
                    state.fps = current_fps;
                    state.camera_name = String::from("WSL-Cam-Bridge: Live Test Pattern (Connect camera anytime)");
                }

                thread::sleep(Duration::from_millis(33)); // ~30 FPS
            }

            if let Some(mut cam) = camera_opt.take() {
                let _ = cam.stop_stream();
            }
            println!("[Camera] Capture thread stopped.");
        });

        self.worker_handle = Some(handle);
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CameraService {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Generates an animated test pattern with color bars and a motion bar
fn generate_test_card(width: u32, height: u32, frame_num: u64) -> Vec<u8> {
    let mut buf = vec![0u8; (width * height * 3) as usize];

    // Standard 8 color bars
    let colors = [
        [220, 220, 220], // White
        [220, 220, 30],  // Yellow
        [30, 220, 220],  // Cyan
        [30, 220, 30],   // Green
        [220, 30, 220],  // Magenta
        [220, 30, 30],   // Red
        [30, 30, 220],   // Blue
        [20, 20, 20],    // Black
    ];

    let bar_width = width / 8;
    let split_y = (height as f32 * 0.75) as u32;

    // Moving block in lower section
    let bounce_speed = 8;
    let bounce_range = (width - 100).max(1);
    let block_pos_x = ((frame_num * bounce_speed) % (bounce_range as u64 * 2)) as i64;
    let block_x = if block_pos_x > bounce_range as i64 {
        (bounce_range as i64 * 2) - block_pos_x
    } else {
        block_pos_x
    } as u32;

    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 3) as usize;

            if y < split_y {
                let bar_idx = (x / bar_width).min(7) as usize;
                let c = colors[bar_idx];
                buf[idx] = c[0];
                buf[idx + 1] = c[1];
                buf[idx + 2] = c[2];
            } else {
                // Bottom control section: dark background
                if x >= block_x && x < block_x + 90 && y >= split_y + 20 && y < split_y + 70 {
                    // Cyan motion tracker
                    buf[idx] = 56;
                    buf[idx + 1] = 189;
                    buf[idx + 2] = 248;
                } else {
                    buf[idx] = 30;
                    buf[idx + 1] = 41;
                    buf[idx + 2] = 59;
                }
            }
        }
    }

    buf
}
