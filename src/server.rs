use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
    paused: bool,
}

pub struct HttpServer {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl HttpServer {
    pub fn start(
        port: u16,
        frame_state: Arc<RwLock<FrameState>>,
        paused_flag: Arc<AtomicBool>,
        target_width: Arc<AtomicU32>,
        target_height: Arc<AtomicU32>,
    ) -> Result<Self, String> {
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
                        } else if path == "/api/pause" {
                            handle_api_pause(request, &paused_flag);
                        } else if path.starts_with("/api/resolution") {
                            handle_api_resolution(request, &path, &target_width, &target_height);
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

fn handle_api_pause(request: tiny_http::Request, paused_flag: &Arc<AtomicBool>) {
    let current = paused_flag.load(Ordering::SeqCst);
    paused_flag.store(!current, Ordering::SeqCst);
    let new_state = !current;

    let json = format!(r#"{{"paused":{new_state}}}"#);
    let mut response = Response::from_string(json).with_header(
        Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
    );
    response.add_header(
        Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap(),
    );
    let _ = request.respond(response);

    if new_state {
        println!("[API] Stream paused via web dashboard.");
    } else {
        println!("[API] Stream resumed via web dashboard.");
    }
}

fn handle_api_resolution(
    request: tiny_http::Request,
    path: &str,
    target_width: &Arc<AtomicU32>,
    target_height: &Arc<AtomicU32>,
) {
    // Parse ?w=1280&h=720 from the URL
    let mut w: Option<u32> = None;
    let mut h: Option<u32> = None;

    if let Some(query) = path.split('?').nth(1) {
        for param in query.split('&') {
            let mut kv = param.splitn(2, '=');
            if let (Some(key), Some(val)) = (kv.next(), kv.next()) {
                match key {
                    "w" => w = val.parse().ok(),
                    "h" => h = val.parse().ok(),
                    _ => {}
                }
            }
        }
    }

    if let (Some(width), Some(height)) = (w, h) {
        target_width.store(width, Ordering::SeqCst);
        target_height.store(height, Ordering::SeqCst);
        println!("[API] Resolution set to {width}x{height} via web dashboard.");

        let json = format!(r#"{{"width":{width},"height":{height}}}"#);
        let mut response = Response::from_string(json).with_header(
            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
        );
        response.add_header(
            Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap(),
        );
        let _ = request.respond(response);
    } else {
        let mut response =
            Response::from_string(r#"{"error":"Missing w and h parameters"}"#)
                .with_status_code(StatusCode(400))
                .with_header(
                    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
                );
        response.add_header(
            Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap(),
        );
        let _ = request.respond(response);
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
                status: if guard.paused {
                    "paused".to_string()
                } else if guard.frame_count > 0 {
                    "streaming".to_string()
                } else {
                    "idle".to_string()
                },
                paused: guard.paused,
            }
        } else {
            StatusResponse {
                camera: "Unknown".to_string(),
                width: 0,
                height: 0,
                fps: 0.0,
                frame_count: 0,
                status: "error".to_string(),
                paused: false,
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
      --red: #ef4444;
    }}
    * {{ box-sizing: border-box; margin: 0; padding: 0; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }}
    body {{ background: var(--bg); color: var(--text); padding: 24px; min-height: 100vh; }}
    .container {{ max-width: 1000px; margin: 0 auto; display: flex; flex-direction: column; gap: 24px; }}
    header {{ display: flex; justify-content: space-between; align-items: center; border-bottom: 1px solid var(--border); padding-bottom: 16px; }}
    .title-group {{ display: flex; align-items: center; gap: 12px; }}
    .badge {{ display: inline-flex; align-items: center; gap: 6px; padding: 4px 10px; border-radius: 9999px; font-size: 0.85rem; font-weight: 600; background: rgba(34,197,94,0.15); color: var(--green); }}
    .badge.paused {{ background: rgba(239,68,68,0.15); color: var(--red); }}
    .dot {{ width: 8px; height: 8px; border-radius: 50%; background: var(--green); animation: pulse 2s infinite; }}
    .dot.paused {{ background: var(--red); animation: none; }}
    @keyframes pulse {{ 0%, 100% {{ opacity: 1; }} 50% {{ opacity: 0.4; }} }}
    .card {{ background: var(--card); border: 1px solid var(--border); border-radius: 12px; padding: 20px; box-shadow: 0 4px 20px rgba(0,0,0,0.3); }}
    .video-container {{ width: 100%; border-radius: 8px; overflow: hidden; background: #000; position: relative; aspect-ratio: 16/9; display: flex; justify-content: center; align-items: center; }}
    .video-container img {{ width: 100%; height: 100%; object-fit: contain; }}
    .stats-row {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 16px; margin-top: 16px; }}
    .stat-box {{ background: rgba(15,23,42,0.6); padding: 14px; border-radius: 8px; border: 1px solid var(--border); }}
    .stat-label {{ font-size: 0.8rem; color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.05em; }}
    .stat-val {{ font-size: 1.25rem; font-weight: 700; margin-top: 4px; color: var(--accent); }}
    .controls {{ display: flex; flex-wrap: wrap; gap: 10px; margin-top: 16px; }}
    .ctrl-btn {{ background: var(--border); border: 1px solid rgba(255,255,255,0.1); color: var(--text); padding: 8px 16px; border-radius: 8px; cursor: pointer; font-size: 0.9rem; font-weight: 500; transition: background 0.15s, color 0.15s; }}
    .ctrl-btn:hover {{ background: var(--accent); color: #000; }}
    .ctrl-btn.active {{ background: var(--accent); color: #000; }}
    .ctrl-btn.pause {{ background: rgba(239,68,68,0.2); border-color: var(--red); color: var(--red); }}
    .ctrl-btn.pause:hover {{ background: var(--red); color: #fff; }}
    .ctrl-btn.resume {{ background: rgba(34,197,94,0.2); border-color: var(--green); color: var(--green); }}
    .ctrl-btn.resume:hover {{ background: var(--green); color: #fff; }}
    .code-block {{ background: #020617; border: 1px solid var(--border); border-radius: 8px; padding: 14px; position: relative; margin-top: 12px; }}
    pre {{ overflow-x: auto; font-family: "Cascadia Code", "Fira Code", monospace; font-size: 0.9rem; color: #e2e8f0; }}
    .copy-btn {{ position: absolute; right: 10px; top: 10px; background: var(--border); border: none; color: var(--text); padding: 6px 12px; border-radius: 6px; cursor: pointer; font-size: 0.8rem; }}
    .copy-btn:hover {{ background: var(--accent); color: #000; }}
    .hint {{ color: var(--text-muted); font-size: 0.9rem; line-height: 1.5; margin-top: 8px; }}
    .section-title {{ font-size: 1rem; font-weight: 600; margin-bottom: 10px; color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.05em; }}
  </style>
</head>
<body>
  <div class="container">
    <header>
      <div class="title-group">
        <h2>WSL-Cam-Bridge</h2>
        <span class="badge" id="status-badge"><span class="dot" id="status-dot"></span> <span id="status-text">LIVE</span></span>
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
      <div class="section-title">Controls</div>
      <div class="controls">
        <button class="ctrl-btn pause" id="btn-pause" onclick="togglePause()">Pause Stream</button>
        <button class="ctrl-btn" onclick="setResolution(640, 480)">480p</button>
        <button class="ctrl-btn active" onclick="setResolution(1280, 720)">720p</button>
        <button class="ctrl-btn" onclick="setResolution(1920, 1080)">1080p</button>
      </div>
    </div>

    <details class="card">
      <summary style="cursor: pointer; font-size: 1.15rem; font-weight: 600; display: flex; justify-content: space-between; align-items: center; user-select: none;">
        <span>Connecting from WSL2</span>
        <span style="font-size: 0.85rem; color: var(--text-muted); font-weight: normal;">Click to expand</span>
      </summary>

      <div style="margin-top: 16px;">
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
    </details>
  </div>

  <script>
    let isPaused = false;
    let currentRes = '720p';

    function updateStats() {{
      fetch('/status')
        .then(res => res.json())
        .then(data => {{
          document.getElementById('stat-cam').textContent = data.camera || 'Active';
          document.getElementById('stat-res').textContent = (data.width && data.height) ? `${{data.width}} x ${{data.height}}` : '--';
          document.getElementById('stat-fps').textContent = (data.fps || 0) + ' FPS';
          document.getElementById('stat-frames').textContent = data.frame_count || 0;

          isPaused = data.paused || false;
          updatePauseButton();
          updateStatusBadge();
        }})
        .catch(() => {{}});
    }}

    function updatePauseButton() {{
      const btn = document.getElementById('btn-pause');
      if (isPaused) {{
        btn.textContent = 'Resume Stream';
        btn.className = 'ctrl-btn resume';
      }} else {{
        btn.textContent = 'Pause Stream';
        btn.className = 'ctrl-btn pause';
      }}
    }}

    function updateStatusBadge() {{
      const badge = document.getElementById('status-badge');
      const dot = document.getElementById('status-dot');
      const text = document.getElementById('status-text');
      if (isPaused) {{
        badge.className = 'badge paused';
        dot.className = 'dot paused';
        text.textContent = 'PAUSED';
      }} else {{
        badge.className = 'badge';
        dot.className = 'dot';
        text.textContent = 'LIVE';
      }}
    }}

    function togglePause() {{
      fetch('/api/pause', {{ method: 'POST' }})
        .then(res => res.json())
        .then(data => {{
          isPaused = data.paused;
          updatePauseButton();
          updateStatusBadge();
          if (!isPaused) {{
            // Reconnect stream after resuming
            const img = document.getElementById('live-stream');
            img.src = '/video?' + Date.now();
          }}
        }})
        .catch(() => {{}});
    }}

    function setResolution(w, h) {{
      fetch(`/api/resolution?w=${{w}}&h=${{h}}`, {{ method: 'POST' }})
        .then(res => res.json())
        .then(() => {{
          // Highlight active button
          const buttons = document.querySelectorAll('.controls .ctrl-btn:not(#btn-pause)');
          buttons.forEach(btn => btn.classList.remove('active'));
          const label = h <= 480 ? '480p' : h <= 720 ? '720p' : '1080p';
          buttons.forEach(btn => {{
            if (btn.textContent === label) btn.classList.add('active');
          }});
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
