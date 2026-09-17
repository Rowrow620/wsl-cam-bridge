#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera;
mod server;
mod tray;

use std::time::Duration;

use arboard::Clipboard;
use camera::CameraService;
use server::HttpServer;
use tray::TrayManager;
use tray_icon::menu::MenuEvent;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Starting WSL-Cam-Bridge ===");

    let port: u16 = 8080;

    // 1. Enumerate cameras
    let devices = CameraService::list_devices();
    println!("Discovered {} camera device(s):", devices.len());
    for dev in &devices {
        println!("  - [{}] {} ({})", dev.index, dev.name, dev.description);
    }

    // 2. Initialize and start camera service
    let mut cam_service = CameraService::new();
    if let Some(first) = devices.first() {
        cam_service.set_camera_index(first.index);
    }
    cam_service.start();

    // 3. Start HTTP / MJPEG streaming server
    let frame_state = cam_service.get_frame_state();
    let mut http_server = match HttpServer::start(port, frame_state) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[Error] Failed to bind HTTP server: {e}");
            return Err(e.into());
        }
    };

    // 4. Initialize System Tray
    let tray_manager = match TrayManager::new(port, &devices) {
        Ok(tm) => tm,
        Err(e) => {
            eprintln!("[Error] Failed to create system tray icon: {e}");
            return Err(e);
        }
    };

    println!("[Tray] Icon registered in Windows system tray.");
    println!("[Ready] WSL-Cam-Bridge is active! Press 'Exit' from the system tray menu to stop.");

    // 5. Main Win32 Event & Tray Loop
    let menu_channel = MenuEvent::receiver();
    let mut clipboard = Clipboard::new().ok();

    let mut running = true;
    while running {
        // Windows message pump
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // Process menu events
        while let Ok(event) = menu_channel.try_recv() {
            let id = event.id.0;
            if id == tray_manager.quit_id {
                println!("[Tray] Exit clicked. Shutting down...");
                running = false;
                break;
            } else if id == tray_manager.open_web_id {
                let url = format!("http://localhost:{port}");
                let _ = open::that(url);
            } else if id == tray_manager.copy_url_id {
                let stream_url = format!("http://localhost:{port}/video");
                if let Some(cb) = clipboard.as_mut() {
                    let _ = cb.set_text(stream_url.clone());
                    println!("[Tray] Copied stream URL to clipboard: {stream_url}");
                }
            } else if id == tray_manager.res_720p_id {
                println!("[Tray] Switching to 1280x720");
                cam_service.set_resolution(1280, 720);
            } else if id == tray_manager.res_1080p_id {
                println!("[Tray] Switching to 1920x1080");
                cam_service.set_resolution(1920, 1080);
            } else if id == tray_manager.res_480p_id {
                println!("[Tray] Switching to 640x480");
                cam_service.set_resolution(640, 480);
            } else if let Some(&cam_idx) = tray_manager.camera_item_ids.get(&id) {
                println!("[Tray] Switching to camera index {cam_idx}");
                cam_service.set_camera_index(cam_idx);
            }
        }

        std::thread::sleep(Duration::from_millis(16));
    }

    println!("[Cleanup] Stopping services...");
    http_server.stop();
    cam_service.stop();
    drop(tray_manager);

    println!("=== WSL-Cam-Bridge Stopped ===");
    Ok(())
}
