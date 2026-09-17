use std::collections::HashMap;

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

        let _ = menu.append(&PredefinedMenuItem::separator());

        // Action items
        let copy_url_item = MenuItem::new("Copy WSL Stream URL", true, None);
        let copy_url_id = copy_url_item.id().0.clone();
        let _ = menu.append(&copy_url_item);

        let open_web_item = MenuItem::new("Open Web Preview", true, None);
        let open_web_id = open_web_item.id().0.clone();
        let _ = menu.append(&open_web_item);

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
            camera_item_ids,
        })
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
