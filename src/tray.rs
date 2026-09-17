use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::camera::CameraDeviceInfo;

pub struct TrayManager {
    #[allow(dead_code)]
    pub tray_icon: TrayIcon,
    pub pause_item: MenuItem,
    pub pause_id: String,
    pub quit_id: String,
    pub open_web_id: String,
    pub copy_url_id: String,
    pub res_720p_id: String,
    pub res_1080p_id: String,
    pub res_480p_id: String,
    pub refresh_cameras_id: String,
    pub startup_item: MenuItem,
    pub startup_id: String,
    pub camera_submenu: Submenu,
    pub camera_item_ids: HashMap<String, u32>,
}

impl TrayManager {
    pub fn new(
        port: u16,
        devices: &[CameraDeviceInfo],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let menu = Menu::new();

        // Status header
        let title_item = MenuItem::new("WSL-Cam-Bridge: Active", false, None);
        let _ = menu.append(&title_item);

        let status_item = MenuItem::new(
            format!("Streaming on http://localhost:{port}/video"),
            false,
            None,
        );
        let _ = menu.append(&status_item);

        let _ = menu.append(&PredefinedMenuItem::separator());

        // Pause / Resume toggle
        let pause_item = MenuItem::new("Pause Stream", true, None);
        let pause_id = pause_item.id().0.clone();
        let _ = menu.append(&pause_item);

        let _ = menu.append(&PredefinedMenuItem::separator());

        // Resolution Submenu
        let res_submenu = Submenu::new("Resolution", true);
        let res_720p = MenuItem::new("1280 x 720 (HD - Default)", true, None);
        let res_1080p = MenuItem::new("1920 x 1080 (FHD)", true, None);
        let res_480p = MenuItem::new("640 x 480 (VGA - Fast)", true, None);

        let res_720p_id = res_720p.id().0.clone();
        let res_1080p_id = res_1080p.id().0.clone();
        let res_480p_id = res_480p.id().0.clone();

        let _ = res_submenu.append(&res_720p);
        let _ = res_submenu.append(&res_1080p);
        let _ = res_submenu.append(&res_480p);
        let _ = menu.append(&res_submenu);

        // Cameras Submenu
        let cam_submenu = Submenu::new("Select Camera", true);
        let mut camera_item_ids = HashMap::new();

        if devices.is_empty() {
            let no_cam = MenuItem::new("No cameras detected", false, None);
            let _ = cam_submenu.append(&no_cam);
        } else {
            for dev in devices {
                let label = format!("[{}] {}", dev.index, dev.name);
                let item = MenuItem::new(label, true, None);
                camera_item_ids.insert(item.id().0.clone(), dev.index);
                let _ = cam_submenu.append(&item);
            }
        }
        let _ = menu.append(&cam_submenu);

        // Refresh Cameras
        let refresh_cameras_item = MenuItem::new("Refresh Cameras", true, None);
        let refresh_cameras_id = refresh_cameras_item.id().0.clone();
        let _ = menu.append(&refresh_cameras_item);

        let _ = menu.append(&PredefinedMenuItem::separator());

        // Action items
        let copy_url_item = MenuItem::new("Copy WSL Stream URL", true, None);
        let copy_url_id = copy_url_item.id().0.clone();
        let _ = menu.append(&copy_url_item);

        let open_web_item = MenuItem::new("Open Web Preview", true, None);
        let open_web_id = open_web_item.id().0.clone();
        let _ = menu.append(&open_web_item);

        let _ = menu.append(&PredefinedMenuItem::separator());

        // Launch on Startup toggle
        let has_startup = startup_shortcut_exists();
        let startup_label = if has_startup {
            "Launch on Startup (On)"
        } else {
            "Launch on Startup (Off)"
        };
        let startup_item = MenuItem::new(startup_label, true, None);
        let startup_id = startup_item.id().0.clone();
        let _ = menu.append(&startup_item);

        let _ = menu.append(&PredefinedMenuItem::separator());

        // Quit item
        let quit_item = MenuItem::new("Exit", true, None);
        let quit_id = quit_item.id().0.clone();
        let _ = menu.append(&quit_item);

        let icon = create_camera_icon();

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("WSL-Cam-Bridge (Running)")
            .with_icon(icon)
            .build()?;

        Ok(Self {
            tray_icon,
            pause_item,
            pause_id,
            quit_id,
            open_web_id,
            copy_url_id,
            res_720p_id,
            res_1080p_id,
            res_480p_id,
            refresh_cameras_id,
            startup_item,
            startup_id,
            camera_submenu: cam_submenu,
            camera_item_ids,
        })
    }

    pub fn rebuild_camera_submenu(&mut self, devices: &[CameraDeviceInfo]) {
        self.camera_item_ids.clear();

        while self.camera_submenu.remove_at(0).is_some() {}

        if devices.is_empty() {
            let no_cam = MenuItem::new("No cameras detected", false, None);
            let _ = self.camera_submenu.append(&no_cam);
        } else {
            for dev in devices {
                let label = format!("[{}] {}", dev.index, dev.name);
                let item = MenuItem::new(label, true, None);
                self.camera_item_ids.insert(item.id().0.clone(), dev.index);
                let _ = self.camera_submenu.append(&item);
            }
        }

        println!("[Tray] Camera list refreshed: {} device(s) found.", devices.len());
    }

    pub fn show_notification(&self, body: &str) {
        self.tray_icon.set_tooltip(Some(body)).ok();
        // Reset tooltip after a delay is not practical here, so we keep the tooltip
        // as a persistent status indicator until the next state change.
    }

    pub fn update_startup_label(&self) {
        let has_startup = startup_shortcut_exists();
        let label = if has_startup {
            "Launch on Startup (On)"
        } else {
            "Launch on Startup (Off)"
        };
        self.startup_item.set_text(label);
    }

}

fn create_camera_icon() -> Icon {
    let width = 32u32;
    let height = 32u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];

    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;

            let in_body = x >= 4 && x <= 27 && y >= 10 && y <= 25;
            let in_top = x >= 8 && x <= 14 && y >= 6 && y <= 9;

            let dx = x as f32 - 16.0;
            let dy = y as f32 - 18.0;
            let dist_sq = dx * dx + dy * dy;
            let in_lens_rim = dist_sq <= 5.5 * 5.5 && dist_sq >= 3.0 * 3.0;
            let in_lens_center = dist_sq < 3.0 * 3.0;

            if in_lens_center {
                // Bright cyan lens
                rgba[idx] = 56;
                rgba[idx + 1] = 189;
                rgba[idx + 2] = 248;
                rgba[idx + 3] = 255;
            } else if in_lens_rim {
                // Silver lens ring
                rgba[idx] = 226;
                rgba[idx + 1] = 232;
                rgba[idx + 2] = 240;
                rgba[idx + 3] = 255;
            } else if in_body || in_top {
                // Dark slate camera body
                rgba[idx] = 30;
                rgba[idx + 1] = 41;
                rgba[idx + 2] = 59;
                rgba[idx + 3] = 255;
            } else {
                // Transparent
                rgba[idx + 3] = 0;
            }
        }
    }

    Icon::from_rgba(rgba, width, height).expect("Failed to create tray icon from RGBA")
}

/// Returns the path to the Startup folder shortcut for this application.
fn startup_shortcut_path() -> Option<PathBuf> {
    let appdata = env::var("APPDATA").ok()?;
    let startup_dir = PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup");
    Some(startup_dir.join("wsl-cam-bridge.lnk"))
}

/// Check whether a startup shortcut or copy exists.
pub fn startup_shortcut_exists() -> bool {
    let lnk = startup_shortcut_path().map(|p| p.exists()).unwrap_or(false);
    let exe = env::var("APPDATA")
        .ok()
        .map(|a| {
            PathBuf::from(a)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup")
                .join("wsl-cam-bridge.exe")
                .exists()
        })
        .unwrap_or(false);
    lnk || exe
}

/// Toggle the startup shortcut: create it if missing, remove it if present.
/// Returns true if startup is now enabled, false if disabled.
pub fn toggle_startup() -> bool {
    let lnk_path = match startup_shortcut_path() {
        Some(p) => p,
        None => return false,
    };
    let exe_path = lnk_path.with_file_name("wsl-cam-bridge.exe");

    if lnk_path.exists() || exe_path.exists() {
        let _ = std::fs::remove_file(&lnk_path);
        let _ = std::fs::remove_file(&exe_path);
        println!("[Startup] Removed startup shortcut.");
        false
    } else {
        let current_exe = match env::current_exe() {
            Ok(p) => p,
            Err(_) => return false,
        };
        let work_dir = current_exe.parent().unwrap_or(&current_exe);
        let ps_cmd = format!(
            "$ws = New-Object -ComObject WScript.Shell; $s = $ws.CreateShortcut('{}'); $s.TargetPath = '{}'; $s.WorkingDirectory = '{}'; $s.Save()",
            lnk_path.display(),
            current_exe.display(),
            work_dir.display()
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
            .output();

        if lnk_path.exists() {
            println!("[Startup] Created shortcut: {}", lnk_path.display());
            true
        } else {
            // Fallback: copy executable directly
            match std::fs::copy(&current_exe, &exe_path) {
                Ok(_) => {
                    println!("[Startup] Copied executable to startup folder.");
                    true
                }
                Err(e) => {
                    eprintln!("[Startup] Failed to create startup entry: {e}");
                    false
                }
            }
        }
    }
}
