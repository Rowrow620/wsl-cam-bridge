#!/usr/bin/env bash
# ==============================================================================
# setup_v4l2.sh
# Creates a virtual /dev/video0 device in WSL2 using v4l2loopback and pipes the
# wsl-cam-bridge stream into it using ffmpeg.
# ==============================================================================

set -e

PORT=8080

echo "=== WSL2 Virtual Webcam Setup (/dev/video0) ==="

# 1. Check prerequisites
if ! command -v ffmpeg &> /dev/null; then
    echo "[*] Installing ffmpeg and v4l-utils..."
    sudo apt update && sudo apt install -y ffmpeg v4l-utils v4l2loopback-dkms
fi

# 2. Detect Host IP
HOST_IP="localhost"
if ! curl -s --connect-timeout 1 "http://localhost:${PORT}/status" > /dev/null; then
    HOST_IP=$(ip route | grep default | awk '{print $3}')
    if ! curl -s --connect-timeout 1 "http://${HOST_IP}:${PORT}/status" > /dev/null; then
        echo "[!] Cannot reach wsl-cam-bridge on localhost or default gateway (${HOST_IP})."
        echo "    Ensure wsl-cam-bridge is running on Windows."
        read -p "Enter Windows Host IP manually: " HOST_IP
    fi
fi

STREAM_URL="http://${HOST_IP}:${PORT}/video"
echo "[+] Detected stream at: ${STREAM_URL}"

# 3. Load v4l2loopback module
if [ ! -e /dev/video0 ]; then
    echo "[*] Loading v4l2loopback kernel module as /dev/video0..."
    sudo modprobe v4l2loopback video_nr=0 card_label="WSL_Virtual_Cam" exclusive_caps=1 || {
        echo "[!] Failed to modprobe v4l2loopback."
        echo "    Note: If your WSL2 kernel does not have module loading enabled,"
        echo "    you can use direct stream access in Python/OpenCV instead (wsl_client.py)!"
        exit 1
    }
fi

echo "[✓] /dev/video0 is ready!"
echo "[+] Piping stream to /dev/video0 with FFmpeg (Press Ctrl+C to stop)..."

ffmpeg -loglevel warning -re -i "${STREAM_URL}" -f v4l2 /dev/video0
