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

fn parse_port() -> u16 {
    let args: Vec<String> = std::env::args().collect();

    // Check --port <N>
    for i in 0..args.len() {
        if args[i] == "--port" {
            if let Some(val) = args.get(i + 1) {
                if let Ok(p) = val.parse::<u16>() {
                    return p;
                }
            }
        }
    }

    // Check WSL_CAM_PORT environment variable
    if let Ok(val) = std::env::var("WSL_CAM_PORT") {
        if let Ok(p) = val.parse::<u16>() {
            return p;
        }
    }

    8080
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Starting WSL-Cam-Bridge ===");

    let port = parse_port();
    println!("[Config] Using port: {port}");

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
    let paused_flag = cam_service.get_paused_flag();
    let target_width = cam_service.get_target_width();
    let target_height = cam_service.get_target_height();

    let mut http_server = match HttpServer::start(port, frame_state, paused_flag, target_width, target_height) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[Error] Failed to bind HTTP server: {e}");
            return Err(e.into());
        }
    };

    // 4. Initialize System Tray
    let mut tray_manager = match TrayManager::new(port, &devices) {
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
            } else if id == tray_manager.pause_id {
                let is_paused = cam_service.toggle_pause();
                if is_paused {
                    tray_manager.pause_item.set_text("Resume Stream");
                    tray_manager.show_notification("WSL-Cam-Bridge: Stream Paused");
                    println!("[Tray] Stream paused.");
                } else {
                    tray_manager.pause_item.set_text("Pause Stream");
                    tray_manager.show_notification("WSL-Cam-Bridge (Running)");
                    println!("[Tray] Stream resumed.");
                }
            } else if id == tray_manager.open_web_id {
                let url = format!("http://localhost:{port}");
                let _ = open::that(url);
            } else if id == tray_manager.copy_url_id {
                let stream_url = format!("http://localhost:{port}/video");
                if let Some(cb) = clipboard.as_mut() {
                    let _ = cb.set_text(stream_url.clone());
                    tray_manager.show_notification("Stream URL copied to clipboard");
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
            } else if id == tray_manager.refresh_cameras_id {
                println!("[Tray] Refreshing camera list...");
                let new_devices = CameraService::list_devices();
                tray_manager.rebuild_camera_submenu(&new_devices);
                let count = new_devices.len();
                tray_manager.show_notification(&format!("Found {count} camera(s)"));
            } else if id == tray_manager.startup_id {
                let enabled = tray::toggle_startup();
                tray_manager.update_startup_label();
                if enabled {
                    tray_manager.show_notification("WSL-Cam-Bridge will launch on startup");
                } else {
                    tray_manager.show_notification("Startup launch disabled");
                }
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
