use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Serialize;
use tiny_http::{Header, Response, Server, StatusCode};

use crate::camera::FrameState;

#[derive(Serialize)]
struct StatusResponse {
    camera: String,
    width: u32,
    height: u32,
    fps: f32,
    frame_count: u64,
    status: String,
}

pub struct HttpServer {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl HttpServer {
    pub fn start(port: u16, frame_state: Arc<RwLock<FrameState>>) -> Result<Self, String> {
        let addr = format!("0.0.0.0:{port}");
        let server = Server::http(&addr).map_err(|e| format!("Failed to bind {addr}: {e}"))?;
        println!("[Server] HTTP server listening on http://{addr}");

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);

        let handle = thread::spawn(move || {
            let server = Arc::new(server);

            while running_clone.load(Ordering::SeqCst) {
                match server.recv_timeout(Duration::from_millis(200)) {
                    Ok(Some(request)) => {
                        let state = Arc::clone(&frame_state);
                        let path = request.url().to_string();

                        if path == "/video" || path == "/stream.mjpg" {
                            // MJPEG Stream requires persistent connection
                            thread::spawn(move || {
                                handle_mjpeg_stream(request, state);
                            });
                        } else if path == "/snapshot" || path == "/frame.jpg" {
                            handle_snapshot(request, state);
                        } else if path == "/status" {
                            handle_status(request, state);
                        } else if path == "/" || path == "/index.html" {
                            handle_index(request, port);
                        } else {
                            let resp = Response::from_string("Not Found")
                                .with_status_code(StatusCode(404));
                            let _ = request.respond(resp);
                        }
                    }
                    Ok(None) => {
                        // Timeout, continue loop
                    }
                    Err(e) => {
                        eprintln!("[Server] Error receiving request: {e}");
                    }
                }
            }
            println!("[Server] Server worker loop terminated.");
        });

        Ok(Self {
            running,
            handle: Some(handle),
        })
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_mjpeg_stream(request: tiny_http::Request, state: Arc<RwLock<FrameState>>) {
    // Upgrade to raw writer stream
    let mut writer = request.into_writer();
    let init_headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n"
    );

    if writer.write_all(init_headers.as_bytes()).is_err() {
        return;
    }

    let mut last_frame_id = 0u64;

    loop {
        let (jpeg_data, frame_id) = {
            if let Ok(guard) = state.read() {
                (Arc::clone(&guard.jpeg), guard.frame_count)
            } else {
                thread::sleep(Duration::from_millis(15));
                continue;
            }
        };

        // Only send when there is a new frame or buffer is available
        if frame_id != last_frame_id && !jpeg_data.is_empty() {
            last_frame_id = frame_id;

            let part_header = format!(
                "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                jpeg_data.len()
            );

            if writer.write_all(part_header.as_bytes()).is_err() {
                break; // Client disconnected
            }

            if writer.write_all(&jpeg_data).is_err() {
                break;
            }

            if writer.write_all(b"\r\n").is_err() {
                break;
            }

            let _ = writer.flush();
        }

        // Limit loop frequency to avoid CPU burn
        thread::sleep(Duration::from_millis(10));
    }
}

fn handle_snapshot(request: tiny_http::Request, state: Arc<RwLock<FrameState>>) {
    let jpeg_data = {
        if let Ok(guard) = state.read() {
            Arc::clone(&guard.jpeg)
        } else {
            let resp = Response::from_string("Unavailable").with_status_code(StatusCode(503));
            let _ = request.respond(resp);
            return;
        }
    };

    if jpeg_data.is_empty() {
        let resp = Response::from_string("Camera Warming Up").with_status_code(StatusCode(503));
        let _ = request.respond(resp);
        return;
    }

    let mut response = Response::from_data((*jpeg_data).clone()).with_header(
        Header::from_bytes(&b"Content-Type"[..], &b"image/jpeg"[..]).unwrap(),
    );
    response.add_header(
        Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap(),
    );
    let _ = request.respond(response);
}

fn handle_status(request: tiny_http::Request, state: Arc<RwLock<FrameState>>) {
    let status_data = {
        if let Ok(guard) = state.read() {
            StatusResponse {
                camera: guard.camera_name.clone(),
                width: guard.width,
                height: guard.height,
                fps: (guard.fps * 10.0).round() / 10.0,
                frame_count: guard.frame_count,
                status: if guard.frame_count > 0 {
                    "streaming".to_string()
                } else {
                    "idle".to_string()
                },
            }
        } else {
            StatusResponse {
                camera: "Unknown".to_string(),
                width: 0,
                height: 0,
                fps: 0.0,
                frame_count: 0,
                status: "error".to_string(),
            }
        }
    };

    let json = serde_json::to_string(&status_data).unwrap_or_else(|_| "{}".to_string());
    let mut response = Response::from_string(json).with_header(
        Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
    );
    response.add_header(
        Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap(),
    );
    let _ = request.respond(response);
}

fn handle_index(request: tiny_http::Request, port: u16) {
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>WSL-Cam-Bridge Dashboard</title>
  <style>
    :root {{
      --bg: #0f172a;
      --card: #1e293b;
      --border: #334155;
      --accent: #38bdf8;
      --accent-hover: #0ea5e9;
      --text: #f8fafc;
      --text-muted: #94a3b8;
      --green: #22c55e;
    }}
    * {{ box-sizing: border-box; margin: 0; padding: 0; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }}
    body {{ background: var(--bg); color: var(--text); padding: 24px; min-height: 100vh; }}
    .container {{ max-width: 1000px; margin: 0 auto; display: flex; flex-direction: column; gap: 24px; }}
    header {{ display: flex; justify-content: space-between; align-items: center; border-bottom: 1px solid var(--border); padding-bottom: 16px; }}
    .title-group {{ display: flex; align-items: center; gap: 12px; }}
    .badge {{ display: inline-flex; align-items: center; gap: 6px; padding: 4px 10px; border-radius: 9999px; font-size: 0.85rem; font-weight: 600; background: rgba(34,197,94,0.15); color: var(--green); }}
    .dot {{ width: 8px; height: 8px; border-radius: 50%; background: var(--green); animation: pulse 2s infinite; }}
    @keyframes pulse {{ 0%, 100% {{ opacity: 1; }} 50% {{ opacity: 0.4; }} }}
    .card {{ background: var(--card); border: 1px solid var(--border); border-radius: 12px; padding: 20px; box-shadow: 0 4px 20px rgba(0,0,0,0.3); }}
    .video-container {{ width: 100%; border-radius: 8px; overflow: hidden; background: #000; position: relative; aspect-ratio: 16/9; display: flex; justify-content: center; align-items: center; }}
    .video-container img {{ width: 100%; height: 100%; object-fit: contain; }}
    .stats-row {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 16px; margin-top: 16px; }}
    .stat-box {{ background: rgba(15,23,42,0.6); padding: 14px; border-radius: 8px; border: 1px solid var(--border); }}
    .stat-label {{ font-size: 0.8rem; color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.05em; }}
    .stat-val {{ font-size: 1.25rem; font-weight: 700; margin-top: 4px; color: var(--accent); }}
    .code-block {{ background: #020617; border: 1px solid var(--border); border-radius: 8px; padding: 14px; position: relative; margin-top: 12px; }}
    pre {{ overflow-x: auto; font-family: "Cascadia Code", "Fira Code", monospace; font-size: 0.9rem; color: #e2e8f0; }}
    .copy-btn {{ position: absolute; right: 10px; top: 10px; background: var(--border); border: none; color: var(--text); padding: 6px 12px; border-radius: 6px; cursor: pointer; font-size: 0.8rem; }}
    .copy-btn:hover {{ background: var(--accent); color: #000; }}
    .hint {{ color: var(--text-muted); font-size: 0.9rem; line-height: 1.5; margin-top: 8px; }}
  </style>
</head>
<body>
  <div class="container">
    <header>
      <div class="title-group">
        <h2>🎥 WSL-Cam-Bridge</h2>
        <span class="badge"><span class="dot"></span> LIVE</span>
      </div>
      <div style="color: var(--text-muted); font-size: 0.9rem;">
        Listening on port: <strong>{port}</strong>
      </div>
    </header>

    <div class="card">
      <div class="video-container">
        <img id="live-stream" src="/video" alt="Webcam Stream Loading..." />
      </div>
      <div class="stats-row">
        <div class="stat-box">
          <div class="stat-label">Camera</div>
          <div class="stat-val" id="stat-cam">Detecting...</div>
        </div>
        <div class="stat-box">
          <div class="stat-label">Resolution</div>
          <div class="stat-val" id="stat-res">-- x --</div>
        </div>
        <div class="stat-box">
          <div class="stat-label">Frame Rate</div>
          <div class="stat-val" id="stat-fps">-- FPS</div>
        </div>
        <div class="stat-box">
          <div class="stat-label">Frames Streamed</div>
          <div class="stat-val" id="stat-frames">0</div>
        </div>
      </div>
    </div>

    <div class="card">
      <h3>🚀 Connecting from WSL2</h3>
      <p class="hint">
        Inside WSL2, access this stream using <code>localhost:{port}</code> (with Windows 11 Mirrored Networking) or your Windows host IP.
      </p>

      <div style="margin-top: 16px;">
        <strong>1. Python & OpenCV (Zero Kernel Setup)</strong>
        <div class="code-block">
          <button class="copy-btn" onclick="copyCode('py-code')">Copy</button>
          <pre id="py-code">import cv2

# Open stream from Windows Host
cap = cv2.VideoCapture("http://localhost:{port}/video")

while True:
    ret, frame = cap.read()
    if not ret:
        continue
    cv2.imshow("WSL2 Camera Feed", frame)
    if cv2.waitKey(1) & 0xFF == ord('q'):
        break

cap.release()
cv2.destroyAllWindows()</pre>
        </div>
      </div>

      <div style="margin-top: 16px;">
        <strong>2. Map to a real /dev/video0 (For ROS & Legacy Linux Apps)</strong>
        <div class="code-block">
          <button class="copy-btn" onclick="copyCode('sh-code')">Copy</button>
          <pre id="sh-code">sudo apt install -y v4l2loopback-dkms ffmpeg
sudo modprobe v4l2loopback video_nr=0 card_label="Virtual_Cam"
ffmpeg -re -i "http://localhost:{port}/video" -f v4l2 /dev/video0</pre>
        </div>
      </div>
    </div>
  </div>

  <script>
    function updateStats() {{
      fetch('/status')
        .then(res => res.json())
        .then(data => {{
          document.getElementById('stat-cam').textContent = data.camera || 'Active';
          document.getElementById('stat-res').textContent = (data.width && data.height) ? `${{data.width}} x ${{data.height}}` : '--';
          document.getElementById('stat-fps').textContent = (data.fps || 0) + ' FPS';
          document.getElementById('stat-frames').textContent = data.frame_count || 0;
        }})
        .catch(() => {{}});
    }}
    setInterval(updateStats, 1000);
    updateStats();

    function copyCode(id) {{
      const text = document.getElementById(id).textContent;
      navigator.clipboard.writeText(text).then(() => {{
        alert('Copied snippet to clipboard!');
      }});
    }}
  </script>
</body>
</html>"#
    );

    let response = Response::from_string(html).with_header(
        Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
    );
    let _ = request.respond(response);
}
