// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Web display support for browser-based GUI access to pods.
//!
//! Two modes:
//! - **noVNC (CE)**: Xvfb + x11vnc + websockify — simple VNC-over-WebSocket.
//! - **WebRTC (Premium)**: GStreamer pipeline — low-latency video + audio + input.
//!
//! All display services run inside the pod. The host only sets up port forwarding
//! and (for WebRTC) provides a signaling relay in the dashboard.

use crate::config::{WebDisplayConfig, WebDisplayType};

/// Generate apt-get install commands for the selected web display type.
pub fn generate_setup_commands(config: &WebDisplayConfig) -> Vec<String> {
    let apt_cleanup = "cd /etc/apt/sources.list.d && for f in *.list *.sources; do case \"$f\" in ubuntu*) ;; *) rm -f \"$f\" ;; esac; done 2>/dev/null; dpkg --configure -a 2>/dev/null; rm -rf /var/lib/apt/lists/* 2>/dev/null; apt-get update -qq";
    match config.display_type {
        WebDisplayType::None => Vec::new(),
        WebDisplayType::Novnc => {
            let mut cmds = vec![
                apt_cleanup.into(),
            ];
            if config.audio {
                cmds.push(concat!(
                    "DEBIAN_FRONTEND=noninteractive apt-get install -y ",
                    "xvfb x11vnc novnc websockify screen dbus-x11 numlockx ",
                    "pulseaudio socat ",
                    "gstreamer1.0-tools gstreamer1.0-plugins-base ",
                    "gstreamer1.0-plugins-good gstreamer1.0-plugins-bad"
                ).into());
            } else {
                cmds.push(
                    "DEBIAN_FRONTEND=noninteractive apt-get install -y xvfb x11vnc novnc websockify screen dbus-x11 numlockx".into()
                );
            }
            cmds
        }
        WebDisplayType::Webrtc => vec![
            apt_cleanup.into(),
            concat!(
                "DEBIAN_FRONTEND=noninteractive apt-get install -y -qq ",
                "xvfb xdotool screen dbus-x11 ",
                "gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good ",
                "gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-nice ",
                "gstreamer1.0-pulseaudio ",
                "> /dev/null 2>&1"
            ).into(),
        ],
    }
}

/// Generate the display service daemon and wrapper scripts.
///
/// Returns `(daemon_script, wrapper_script)`:
/// - **daemon**: background process managing Xvfb/x11vnc/websockify/audio/upload
///   with auto-restart on crash. Written to `envpod-display-services`.
/// - **wrapper**: lightweight entry point for user commands. Starts daemon if not
///   running, then `exec`s the user command immediately. The user always gets their
///   shell — even if display services fail to start. Written to `envpod-display-start`.
pub fn generate_supervisor_script(config: &WebDisplayConfig) -> String {
    // Returns only the wrapper script for backward compatibility.
    // The daemon script is returned by generate_services_daemon().
    match config.display_type {
        WebDisplayType::None => String::new(),
        WebDisplayType::Novnc => generate_novnc_wrapper(config),
        WebDisplayType::Webrtc => generate_webrtc_wrapper(config),
    }
}

/// Generate the background display services daemon script.
/// Written to `upper/usr/local/bin/envpod-display-services` during init.
pub fn generate_services_daemon(config: &WebDisplayConfig) -> String {
    match config.display_type {
        WebDisplayType::None => String::new(),
        WebDisplayType::Novnc => generate_novnc_daemon(config),
        WebDisplayType::Webrtc => generate_webrtc_daemon(config),
    }
}

/// Generate the noVNC services daemon script.
/// Runs as a background process managing all display services with auto-restart.
fn generate_novnc_daemon(config: &WebDisplayConfig) -> String {
    let resolution = &config.resolution;
    let audio_port = config.audio_port;

    let audio_block = if config.audio {
        format!(r#"
# --- Audio streaming (PulseAudio + Opus/WebM → WebSocket) ---
id pulse >/dev/null 2>&1 || useradd --system --no-create-home -s /usr/sbin/nologin pulse 2>/dev/null
getent group pulse-access >/dev/null 2>&1 || groupadd --system pulse-access 2>/dev/null
usermod -aG pulse-access root 2>/dev/null
mkdir -p /var/run/pulse /var/lib/pulse
chown pulse:pulse /var/run/pulse /var/lib/pulse 2>/dev/null
mkdir -p /etc/pulse/system.pa.d
cat > /etc/pulse/system.pa.d/envpod.pa << 'PACONF'
load-module module-native-protocol-unix auth-anonymous=1
PACONF
pulseaudio --system --daemonize --no-cpu-limit --log-target=file:/tmp/pulseaudio.log
sleep 1
export PULSE_SERVER=unix:/var/run/pulse/native
pactl load-module module-null-sink sink_name=envpod format=s16le channels=2 rate=48000 sink_properties=device.description=envpod
pactl set-default-sink envpod
pactl load-module module-simple-protocol-tcp listen=127.0.0.1 port=4711 format=s16le rate=48000 channels=2 record=true source=envpod.monitor
pactl set-source-volume envpod.monitor 65536
(pactl subscribe 2>/dev/null | while read -r line; do
    case "$line" in *source*) pactl set-source-volume envpod.monitor 65536 2>/dev/null;; esac
done) &
(while true; do /usr/local/bin/envpod-audio-proxy.sh -l 5711; sleep 1; done) &
sleep 0.5
(while true; do websockify 0.0.0.0:{audio_port} localhost:5711; sleep 1; done) &
"#)
    } else {
        String::new()
    };

    let upload_block = if config.file_upload {
        "\n# --- File upload server ---\nmkdir -p /tmp/uploads\n(while true; do python3 /usr/local/bin/envpod-upload-server.py 2>/dev/null; sleep 1; done) &\n".to_string()
    } else {
        String::new()
    };

    format!(
        r#"#!/bin/bash
# envpod display services daemon (noVNC)
# Runs in background. Manages Xvfb, x11vnc, websockify, audio, uploads.
# Auto-restarts crashed services. User commands run independently.

exec >/tmp/envpod-display-services.log 2>&1

# Start a new process group so kill/trap don't affect user shells
set -m

export __EGL_VENDOR_LIBRARY_FILENAMES=""
export __GLX_VENDOR_LIBRARY_NAME=mesa
export DISPLAY=:99

echo $$ > /tmp/envpod-display.pid
trap "rm -f /tmp/envpod-display.pid; kill -- -$$ 2>/dev/null" EXIT

# Start D-Bus session bus (required by XFCE terminal, thunar, etc.)
if command -v dbus-daemon >/dev/null 2>&1; then
    dbus-daemon --session --address=unix:path=/tmp/envpod-dbus --nofork --nopidfile &
    echo "D-Bus session bus started"
fi

# Start Xvfb virtual display (auto-restart on crash)
(while true; do Xvfb :99 -screen 0 {resolution}x24 -ac -noreset 2>/dev/null; sleep 1; done) &

# Wait for Xvfb to be ready
for i in $(seq 1 20); do
    [ -e /tmp/.X11-unix/X99 ] && break
    sleep 0.25
done

if [ ! -e /tmp/.X11-unix/X99 ]; then
    echo "ERROR: Xvfb failed to start"
    exit 1
fi

# Sync NumLock state (default on — matches most host keyboards)
command -v numlockx >/dev/null 2>&1 && numlockx on 2>/dev/null
command -v xdotool >/dev/null 2>&1 && xdotool key --clearmodifiers Num_Lock 2>/dev/null

# Start x11vnc (auto-restart on crash)
(while true; do x11vnc -display :99 -forever -nopw -shared -noshm -rfbport 5900 -q; sleep 1; done) &
sleep 1

# Start websockify (auto-restart on crash)
(while true; do websockify --web /usr/share/novnc 0.0.0.0:6080 localhost:5900; sleep 1; done) &
{audio_block}{upload_block}
echo "Display services started"

# Keep daemon alive — wait for all children
wait
"#
    )
}

/// Generate the noVNC wrapper script.
/// Starts daemon if needed, then exec's the user command immediately.
fn generate_novnc_wrapper(config: &WebDisplayConfig) -> String {
    let _config = config;
    r#"#!/bin/bash
# envpod display wrapper — starts services daemon if needed, then runs user command.

export DISPLAY=:99
export __EGL_VENDOR_LIBRARY_FILENAMES=""
export __GLX_VENDOR_LIBRARY_NAME=mesa
export DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/envpod-dbus

# Start display services daemon if not already running
if [ ! -f /tmp/envpod-display.pid ] || ! kill -0 "$(cat /tmp/envpod-display.pid)" 2>/dev/null; then
    setsid /usr/local/bin/envpod-display-services &
    disown
    # Wait for Xvfb to come up (max 5s)
    for i in $(seq 1 20); do
        [ -e /tmp/.X11-unix/X99 ] && break
        sleep 0.25
    done
fi

# Drop privileges if requested
if [ -n "$ENVPOD_RUN_USER" ]; then
    chmod 1777 /tmp/.X11-unix 2>/dev/null
    usermod -aG pulse-access "$ENVPOD_RUN_USER" 2>/dev/null
    # Resolve uid/gid from /etc/passwd (runuser uses PAM which fails in namespaces)
    EP_UID=$(id -u "$ENVPOD_RUN_USER" 2>/dev/null)
    EP_GID=$(id -g "$ENVPOD_RUN_USER" 2>/dev/null)
    EP_HOME=$(getent passwd "$ENVPOD_RUN_USER" 2>/dev/null | cut -d: -f6)
    EP_HOME="${EP_HOME:-/home/$ENVPOD_RUN_USER}"
    export HOME="$EP_HOME"
    exec setpriv --reuid="$EP_UID" --regid="$EP_GID" --init-groups "$@"
else
    exec "$@"
fi
"#.to_string()
}

/// Generate the WebRTC services daemon script.
fn generate_webrtc_daemon(config: &WebDisplayConfig) -> String {
    let resolution = &config.resolution;
    let codec_pipeline = match config.codec.as_str() {
        "h264" => "x264enc tune=zerolatency speed-preset=ultrafast ! video/x-h264,profile=baseline ! rtph264pay",
        _ => "vp8enc deadline=1 target-bitrate=2000000 ! rtpvp8pay",
    };
    let audio_pipeline = if config.audio {
        "\n# Start audio capture pipeline\n(while true; do gst-launch-1.0 -q pulsesrc ! opusenc ! rtpopuspay ! webrtcbin name=audio-send; sleep 1; done) &"
    } else {
        ""
    };

    format!(
        r#"#!/bin/bash
# envpod display services daemon (WebRTC/GStreamer)

exec >/tmp/envpod-display-services.log 2>&1

# Start a new process group so kill/trap don't affect user shells
set -m

export DISPLAY=:99

echo $$ > /tmp/envpod-display.pid
trap "rm -f /tmp/envpod-display.pid; kill -- -$$ 2>/dev/null" EXIT

# Start D-Bus session bus
if command -v dbus-daemon >/dev/null 2>&1; then
    dbus-daemon --session --address=unix:path=/tmp/envpod-dbus --nofork --nopidfile &
fi

# Start Xvfb (auto-restart on crash)
(while true; do Xvfb :99 -screen 0 {resolution}x24 -ac +extension GLX +render -noreset; sleep 1; done) &
for i in $(seq 1 20); do
    [ -e /tmp/.X11-unix/X99 ] && break
    sleep 0.25
done

if [ ! -e /tmp/.X11-unix/X99 ]; then
    echo "ERROR: Xvfb failed to start"
    exit 1
fi

# Start video capture pipeline (auto-restart on crash)
(while true; do gst-launch-1.0 -q ximagesrc use-damage=0 ! videoconvert ! {codec_pipeline} ! webrtcbin name=video-send; sleep 1; done) &
{audio_pipeline}

# Start xdotool input relay
INPUTPIPE=/tmp/envpod-input
mkfifo "$INPUTPIPE" 2>/dev/null || true
(while read -r cmd < "$INPUTPIPE"; do eval "$cmd"; done) &

echo "Display services started"
wait
"#
    )
}

/// Generate the WebRTC wrapper script.
fn generate_webrtc_wrapper(config: &WebDisplayConfig) -> String {
    let _config = config;
    r#"#!/bin/bash
# envpod display wrapper (WebRTC)

export DISPLAY=:99
export DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/envpod-dbus

# Start display services daemon if not already running
if [ ! -f /tmp/envpod-display.pid ] || ! kill -0 "$(cat /tmp/envpod-display.pid)" 2>/dev/null; then
    setsid /usr/local/bin/envpod-display-services &
    disown
    for i in $(seq 1 20); do
        [ -e /tmp/.X11-unix/X99 ] && break
        sleep 0.25
    done
fi

if [ -n "$ENVPOD_RUN_USER" ]; then
    chmod 1777 /tmp/.X11-unix 2>/dev/null
    EP_UID=$(id -u "$ENVPOD_RUN_USER" 2>/dev/null)
    EP_GID=$(id -g "$ENVPOD_RUN_USER" 2>/dev/null)
    EP_HOME=$(getent passwd "$ENVPOD_RUN_USER" 2>/dev/null | cut -d: -f6)
    EP_HOME="${EP_HOME:-/home/$ENVPOD_RUN_USER}"
    export HOME="$EP_HOME"
    exec setpriv --reuid="$EP_UID" --regid="$EP_GID" --init-groups "$@"
else
    exec "$@"
fi
"#.to_string()
}

/// Returns files to inject into the pod overlay for audio support.
/// Each tuple is (path_inside_pod, content, executable).
pub fn audio_overlay_files(config: &WebDisplayConfig) -> Vec<(&'static str, String, bool)> {
    if config.display_type != WebDisplayType::Novnc || !config.audio {
        return Vec::new();
    }
    vec![
        ("/usr/local/bin/envpod-audio-proxy.sh", AUDIO_PROXY_SCRIPT.to_string(), true),
        ("/usr/share/novnc/audio-plugin.js", generate_audio_plugin_js(config), false),
    ]
}

/// Generate audio-plugin.js with the correct default port baked in.
fn generate_audio_plugin_js(config: &WebDisplayConfig) -> String {
    AUDIO_PLUGIN_JS.replace("__ENVPOD_AUDIO_PORT__", &config.audio_port.to_string())
}

/// Returns files to inject into the pod overlay for file upload support.
/// Each tuple is (path_inside_pod, content, executable).
pub fn upload_overlay_files(config: &WebDisplayConfig) -> Vec<(&'static str, String, bool)> {
    if config.display_type != WebDisplayType::Novnc || !config.file_upload {
        return Vec::new();
    }
    vec![
        ("/usr/local/bin/envpod-upload-server.py", generate_upload_server_py(config), true),
        ("/usr/share/novnc/upload-plugin.js", generate_upload_plugin_js(config), false),
    ]
}

/// Returns files to inject into the pod overlay for clipboard sync.
/// Each tuple is (path_inside_pod, content, executable).
pub fn clipboard_overlay_files(config: &WebDisplayConfig) -> Vec<(&'static str, String, bool)> {
    if config.display_type != WebDisplayType::Novnc {
        return Vec::new();
    }
    vec![
        ("/usr/share/novnc/clipboard-plugin.js", CLIPBOARD_PLUGIN_JS.to_string(), false),
    ]
}

/// Generate upload-server.py with the correct port baked in.
fn generate_upload_server_py(config: &WebDisplayConfig) -> String {
    UPLOAD_SERVER_SCRIPT.replace("__ENVPOD_UPLOAD_PORT__", &config.upload_port.to_string())
}

/// Generate upload-plugin.js with the correct port baked in.
fn generate_upload_plugin_js(config: &WebDisplayConfig) -> String {
    UPLOAD_PLUGIN_JS.replace("__ENVPOD_UPLOAD_PORT__", &config.upload_port.to_string())
}

/// Embedded audio-proxy.sh — GStreamer pipeline that encodes PulseAudio raw PCM
/// to Opus/WebM for streaming via WebSocket. Inspired by noVNC-audio-plugin.
const AUDIO_PROXY_SCRIPT: &str = r##"#!/bin/sh
# Audio proxy: raw PCM from PulseAudio → Opus/WebM via GStreamer
# Inspired by noVNC-audio-plugin by Mehrzad Asri

readonly SCRIPT="$0"
readonly PULSE_PORT='4711'
readonly PULSE_FORMAT='s16le'
readonly PULSE_SAMPLE_RATE='48000'
readonly PULSE_CHANNELS='2'
readonly TCP_BIND='127.0.0.1'

error() { echo "$1" >&2; exit 1; }

proto_ready() { echo "READY"; }
proto_error() { echo "ERR:$1"; exit 1; }

opus_proxy() {
    local pulse_port="$1" pulse_format="$2" pulse_sample_rate="$3" pulse_channels="$4" bitrate="$5"
    proto_ready
    exec gst-launch-1.0 -q webmmux name=mux streamable=true min-cluster-duration=50000000 ! fdsink fd=1 \
        tcpclientsrc port="${pulse_port}" ! rawaudioparse use-sink-caps=false format=pcm pcm-format="${pulse_format}" sample-rate="${pulse_sample_rate}" num-channels="${pulse_channels}" \
        ! audioconvert ! audioresample ! opusenc audio-type=restricted-lowdelay bitrate="${bitrate}" bitrate-type=0 complexity=0 frame-size=10 ! mux.audio_0
}

proxy() {
    local pulse_port="$1" pulse_format="$2" pulse_sample_rate="$3" pulse_channels="$4"
    local codec='opus' bitrate='96000'

    local line
    while IFS= read -r line; do
        [ -z "${line}" ] && break
        case "${line}" in *':'*) ;; *) proto_error 'bad handshake' ;; esac
        local opt val
        opt="$(echo "${line}" | cut -d ':' -f 1)"
        val="$(echo "${line}" | cut -d ':' -f 2-)"
        case "${opt}" in
            'CD') codec="${val}" ;; 'BR') bitrate="${val}" ;; 'SR') ;; *) proto_error "invalid option ${opt}" ;;
        esac
    done

    case "${codec}" in
        'opus') opus_proxy "${pulse_port}" "${pulse_format}" "${pulse_sample_rate}" "${pulse_channels}" "${bitrate}" ;;
        *) proto_error "unsupported codec ${codec} (only opus supported)" ;;
    esac
}

server() {
    local pulse_port="${PULSE_PORT}" pulse_format="${PULSE_FORMAT}"
    local pulse_sample_rate="${PULSE_SAMPLE_RATE}" pulse_channels="${PULSE_CHANNELS}"
    local tcp_port="" tcp_bind="${TCP_BIND}"

    while getopts 'p:l:b:f:r:c:h' opt; do
        case "${opt}" in
            'p') pulse_port="${OPTARG}" ;; 'l') tcp_port="${OPTARG}" ;;
            'b') tcp_bind="${OPTARG}" ;; 'f') pulse_format="${OPTARG}" ;;
            'r') pulse_sample_rate="${OPTARG}" ;; 'c') pulse_channels="${OPTARG}" ;;
            'h') echo "Usage: $0 -l <port>"; exit 0 ;; *) exit 1 ;;
        esac
    done

    [ -z "${tcp_port}" ] && error "Usage: $0 -l <port>"
    local proxy_cmd="${SCRIPT} proxy ${pulse_port} ${pulse_format} ${pulse_sample_rate} ${pulse_channels}"
    exec socat tcp-listen:"${tcp_port}",bind="${tcp_bind}",nodelay,reuseaddr,fork exec:"${proxy_cmd}",nofork
}

command -v socat >/dev/null 2>&1 || error 'socat not found'
command -v gst-launch-1.0 >/dev/null 2>&1 || error 'gst-launch-1.0 not found'

if [ "$1" = 'proxy' ]; then shift; proxy "$@"; else server "$@"; fi
"##;

/// Embedded audio-plugin.js — browser-side MediaSource player for noVNC.
/// Connects via WebSocket, receives Opus/WebM, plays via MSE.
/// Inspired by noVNC-audio-plugin.
///
/// __ENVPOD_AUDIO_PORT__ is replaced with the actual port at generation time.
const AUDIO_PLUGIN_JS: &str = r##"/**
 * envpod audio plugin for noVNC
 * Opus/WebM audio streaming via WebSocket + MediaSource API
 * Inspired by noVNC-audio-plugin by Mehrzad Asri
 */

class MediaSourcePlayer {
    static #BUFFER_MIN_REMAIN = 30;
    static #DRIFT_CHECK_INTERVAL = 5000;
    static #DRIFT_MAX_TOLERANCE = 1.0;

    mediaSource;
    sourceBuffer;
    #directFeed = true;
    #dataQueue = [];
    #attachedEl;
    #driftCheckTimer;

    #onPlayCallback = (event) => {
        const elem = event.target;
        if (this.sourceBuffer.buffered.length > 0) {
            elem.currentTime = this.sourceBuffer.buffered.end(0);
        }
        elem.playbackRate = 1.003;
    };

    constructor(mime) {
        this.mediaSource = new MediaSource();
        this.mediaSource.addEventListener('sourceopen', () => {
            this.sourceBuffer = this.mediaSource.addSourceBuffer(mime);
            this.sourceBuffer.mode = 'sequence';
            this.sourceBuffer.addEventListener('updateend', () => {
                if (this.sourceBuffer.updating) return;
                if (this.#dataQueue.length == 0) { this.#directFeed = true; return; }
                const data = this.#dataQueue[0];
                try {
                    this.sourceBuffer.appendBuffer(data);
                    this.#dataQueue.shift();
                } catch (err) {
                    if (err.name == 'QuotaExceededError') {
                        this.#emptyBuffer();
                        if (!this.sourceBuffer.updating) {
                            this.sourceBuffer.appendBuffer(data);
                            this.#dataQueue.shift();
                        }
                    } else throw err;
                }
            });
        }, { once: true });
    }

    async attach(element) {
        if (this.#attachedEl) throw new Error('Already attached');
        element.src = URL.createObjectURL(this.mediaSource);
        this.#attachedEl = element;
        return new Promise((resolve) => {
            this.mediaSource.addEventListener('sourceopen', () => {
                element.addEventListener('play', this.#onPlayCallback);
                this.#driftCheckTimer = setInterval(() => this.#checkDrift(), MediaSourcePlayer.#DRIFT_CHECK_INTERVAL);
                resolve();
            }, { once: true });
        });
    }

    async detach() {
        if (this.#attachedEl) {
            this.#attachedEl.removeEventListener('play', this.#onPlayCallback);
            this.#attachedEl.playbackRate = 1;
            await this.#attachedEl.pause();
            this.#attachedEl.removeAttribute('src');
            this.#attachedEl = null;
        }
        if (this.#driftCheckTimer) { clearInterval(this.#driftCheckTimer); this.#driftCheckTimer = null; }
    }

    feed(data) {
        if (!this.#attachedEl) throw new Error('Not attached');
        if (this.mediaSource.readyState != 'open') throw new Error('Bad MediaSource state');
        if (this.#directFeed) {
            try { this.sourceBuffer.appendBuffer(data); }
            catch (err) {
                if (err.name == 'QuotaExceededError') {
                    this.#emptyBuffer();
                    if (this.sourceBuffer.updating) { this.#directFeed = false; this.#dataQueue.push(data); }
                    else this.sourceBuffer.appendBuffer(data);
                }
            }
            if (this.sourceBuffer.updating) this.#directFeed = false;
        } else {
            this.#dataQueue.push(data);
        }
    }

    #emptyBuffer() {
        const end = this.sourceBuffer.buffered.end(0);
        const removeEnd = end - MediaSourcePlayer.#BUFFER_MIN_REMAIN;
        this.sourceBuffer.remove(0, removeEnd <= 0 ? 1 : removeEnd);
    }

    #checkDrift() {
        if (this.#attachedEl.paused || this.sourceBuffer.buffered.length == 0) return;
        const drift = this.sourceBuffer.buffered.end(0) - this.#attachedEl.currentTime;
        if (drift > MediaSourcePlayer.#DRIFT_MAX_TOLERANCE) {
            this.#attachedEl.currentTime = this.sourceBuffer.buffered.end(0);
        }
    }
}

const AudioProxy = {
    handshake(socket, codec = 'opus', bitrate = 96000) {
        const enc = new TextEncoder(), dec = new TextDecoder();
        socket.send(enc.encode(`CD:${codec}\nBR:${bitrate}\n\n`));
        return new Promise((resolve, reject) => {
            socket.addEventListener('message', (msg) => {
                const resp = dec.decode(msg.data).trim();
                if (resp == 'READY') resolve();
                else reject(new Error(resp.startsWith('ERR:') ? resp.substring(4) : 'Protocol error'));
            }, { once: true });
        });
    }
};

const EnvpodAudio = {
    msp: null, ws: null, audioEl: null, enabled: false, btn: null,
    audioPort: __ENVPOD_AUDIO_PORT__,

    init() {
        this.audioEl = document.createElement('audio');
        this.audioEl.id = 'envpod_audio';
        document.body.appendChild(this.audioEl);
        this.addControls();
        console.log('[envpod-audio] initialized, audio port:', this.audioPort);
    },

    addControls() {
        // Insert audio button into the noVNC side panel, before the disconnect button.
        // Uses the same noVNC_button class so it matches the existing panel style.
        const disconnectBtn = document.getElementById('noVNC_disconnect_button');
        const container = disconnectBtn ? disconnectBtn.parentElement : document.getElementById('noVNC_control_bar');
        if (!container) { console.warn('[envpod-audio] control bar not found'); return; }

        const speakerOn = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z"/></svg>';
        const speakerOff = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M16.5 12c0-1.77-1.02-3.29-2.5-4.03v2.21l2.45 2.45c.03-.2.05-.41.05-.63zm2.5 0c0 .94-.2 1.82-.54 2.64l1.51 1.51A8.796 8.796 0 0021 12c0-4.28-2.99-7.86-7-8.77v2.06c2.89.86 5 3.54 5 6.71zM4.27 3L3 4.27 7.73 9H3v6h4l5 5v-6.73l4.25 4.25c-.67.52-1.42.93-2.25 1.18v2.06a8.99 8.99 0 003.69-1.81L19.73 21 21 19.73l-9-9L4.27 3zM12 4L9.91 6.09 12 8.18V4z"/></svg>';
        this.speakerOnSrc = 'data:image/svg+xml,' + encodeURIComponent(speakerOn);
        this.speakerOffSrc = 'data:image/svg+xml,' + encodeURIComponent(speakerOff);

        const btn = document.createElement('img');
        btn.id = 'envpod_audio_btn';
        btn.className = 'noVNC_button';
        btn.alt = 'Audio';
        btn.title = 'Enable audio [beta]';
        btn.src = this.speakerOffSrc;
        this.btn = btn;

        btn.addEventListener('click', async (e) => {
            e.preventDefault();
            e.stopPropagation();
            console.log('[envpod-audio] toggle clicked, enabled:', this.enabled);
            if (this.enabled) {
                await this.stop();
            } else {
                await this.start();
            }
        });

        if (disconnectBtn) {
            container.insertBefore(btn, disconnectBtn);
        } else {
            container.appendChild(btn);
        }
    },

    updateBtn(on) {
        if (!this.btn) return;
        this.btn.src = on ? this.speakerOnSrc : this.speakerOffSrc;
        this.btn.title = on ? 'Disable audio' : 'Enable audio [beta]';
        if (on) {
            this.btn.classList.add('noVNC_selected');
        } else {
            this.btn.classList.remove('noVNC_selected');
        }
    },

    async start() {
        if (this.msp) return;
        this.enabled = true;
        this.updateBtn(true);

        const wsProto = location.protocol === 'https:' ? 'wss:' : 'ws:';
        const wsHost = location.hostname;
        const url = `${wsProto}//${wsHost}:${this.audioPort}/`;
        console.log('[envpod-audio] connecting to', url);

        try {
            this.ws = new WebSocket(url);
        } catch (err) {
            console.error('[envpod-audio] WebSocket create failed:', err);
            await this.stop();
            return;
        }
        this.ws.binaryType = 'arraybuffer';

        this.ws.addEventListener('error', async (e) => {
            console.error('[envpod-audio] WebSocket error', e);
            await this.stop();
        });
        this.ws.addEventListener('close', async () => {
            if (this.msp) await this.stop();
        });

        this.ws.addEventListener('open', async () => {
            console.log('[envpod-audio] WebSocket connected');
            try {
                this.msp = new MediaSourcePlayer('audio/webm; codecs="opus"');
                await this.msp.attach(this.audioEl);
                await AudioProxy.handshake(this.ws, 'opus', 96000);
                console.log('[envpod-audio] handshake complete, streaming');
            } catch (err) {
                console.error('[envpod-audio] setup failed:', err);
                await this.stop();
                return;
            }

            this.ws.addEventListener('message', async (msg) => {
                try { this.msp.feed(msg.data); }
                catch (err) { console.error('[envpod-audio] feed error:', err); await this.stop(); }
            });

            // Browsers require user interaction before playing audio
            const playOnClick = async () => {
                try { await this.audioEl.play(); console.log('[envpod-audio] playback started'); }
                catch (e) { /* AbortError is fine */ }
            };
            document.body.addEventListener('click', playOnClick, { capture: true, once: true });
            try { await this.audioEl.play(); } catch (e) { /* will play on next click */ }
        });
    },

    async stop() {
        this.enabled = false;
        this.updateBtn(false);
        if (this.msp) { await this.msp.detach(); this.msp = null; }
        if (this.ws) { this.ws.close(); this.ws = null; }
        console.log('[envpod-audio] stopped');
    }
};

window.addEventListener('load', () => EnvpodAudio.init());
"##;

/// Embedded upload server — Python3 HTTP server that accepts file uploads.
/// __ENVPOD_UPLOAD_PORT__ is replaced with the actual port at generation time.
const UPLOAD_SERVER_SCRIPT: &str = r##"#!/usr/bin/env python3
"""envpod file upload server — saves files to /tmp/uploads/"""
import os, http.server, json, datetime

UPLOAD_DIR = '/tmp/uploads'
AUDIT_LOG = '/tmp/envpod-uploads.jsonl'
PORT = __ENVPOD_UPLOAD_PORT__
POD_NAME = os.environ.get('ENVPOD_POD_NAME', 'unknown')

os.makedirs(UPLOAD_DIR, exist_ok=True)

def audit(filename, size, success=True):
    entry = {
        'timestamp': datetime.datetime.utcnow().strftime('%Y-%m-%dT%H:%M:%S.%fZ'),
        'pod_name': POD_NAME,
        'action': 'file_upload',
        'detail': f'file={filename}, size={size}',
        'success': success,
    }
    try:
        with open(AUDIT_LOG, 'a') as f:
            f.write(json.dumps(entry) + '\n')
    except Exception:
        pass

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass

    def do_OPTIONS(self):
        self.send_response(200)
        self._cors()
        self.end_headers()

    def do_GET(self):
        files = []
        for f in sorted(os.listdir(UPLOAD_DIR)):
            p = os.path.join(UPLOAD_DIR, f)
            if os.path.isfile(p):
                files.append({'name': f, 'size': os.path.getsize(p)})
        self.send_response(200)
        self._cors()
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(json.dumps({'files': files, 'dir': UPLOAD_DIR}).encode())

    def do_POST(self):
        name = self.headers.get('X-Filename', 'upload')
        name = os.path.basename(name)
        length = int(self.headers.get('Content-Length', 0))
        if length == 0:
            self._err(400, 'empty body')
            return
        data = self.rfile.read(length)
        with open(os.path.join(UPLOAD_DIR, name), 'wb') as f:
            f.write(data)
        audit(name, len(data))
        self.send_response(200)
        self._cors()
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(json.dumps({'ok': True, 'file': name, 'size': len(data)}).encode())

    def _err(self, code, msg):
        self.send_response(code)
        self._cors()
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(json.dumps({'error': msg}).encode())

    def _cors(self):
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Methods', 'GET, POST, OPTIONS')
        self.send_header('Access-Control-Allow-Headers', '*')

http.server.HTTPServer(('0.0.0.0', PORT), H).serve_forever()
"##;

/// Embedded upload-plugin.js — browser-side upload button for noVNC panel.
/// Sends files via fetch() to the upload server. Shows status on button.
///
/// __ENVPOD_UPLOAD_PORT__ is replaced with the actual port at generation time.
const UPLOAD_PLUGIN_JS: &str = r##"/**
 * envpod upload plugin for noVNC
 * File upload via fetch() to Python upload server inside pod
 * Toast notifications for upload status
 */

const EnvpodUpload = {
    btn: null,
    uploadPort: __ENVPOD_UPLOAD_PORT__,
    toastContainer: null,

    init() {
        this.createToastContainer();
        this.addControls();
        console.log('[envpod-upload] initialized, upload port:', this.uploadPort);
    },

    createToastContainer() {
        const c = document.createElement('div');
        c.id = 'envpod_toast_container';
        c.style.cssText = 'position:fixed;top:12px;right:12px;z-index:99999;display:flex;flex-direction:column;gap:8px;pointer-events:none;';
        document.body.appendChild(c);
        this.toastContainer = c;
    },

    toast(msg, type) {
        const t = document.createElement('div');
        const bg = type === 'ok' ? '#1a7f37' : type === 'err' ? '#cf222e' : '#2d333b';
        t.style.cssText = 'pointer-events:auto;padding:10px 16px;border-radius:8px;color:#fff;font:13px/1.4 -apple-system,system-ui,sans-serif;box-shadow:0 4px 12px rgba(0,0,0,.4);max-width:340px;word-break:break-word;opacity:0;transition:opacity .2s;background:' + bg;
        t.textContent = msg;
        this.toastContainer.appendChild(t);
        requestAnimationFrame(() => { t.style.opacity = '1'; });
        const dur = type === 'ok' ? 4000 : type === 'err' ? 6000 : 2000;
        setTimeout(() => {
            t.style.opacity = '0';
            setTimeout(() => t.remove(), 300);
        }, dur);
        return t;
    },

    addControls() {
        const disconnectBtn = document.getElementById('noVNC_disconnect_button');
        const container = disconnectBtn ? disconnectBtn.parentElement : document.getElementById('noVNC_control_bar');
        if (!container) { console.warn('[envpod-upload] control bar not found'); return; }

        const uploadIcon = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M19.35 10.04C18.67 6.59 15.64 4 12 4 9.11 4 6.6 5.64 5.35 8.04 2.34 8.36 0 10.91 0 14c0 3.31 2.69 6 6 6h13c2.76 0 5-2.24 5-5 0-2.64-2.05-4.78-4.65-4.96zM14 13v4h-4v-4H7l5-5 5 5h-3z"/></svg>';
        const uploadSrc = 'data:image/svg+xml,' + encodeURIComponent(uploadIcon);

        const btn = document.createElement('img');
        btn.id = 'envpod_upload_btn';
        btn.className = 'noVNC_button';
        btn.alt = 'Upload';
        btn.title = 'Upload file to pod';
        btn.src = uploadSrc;
        this.btn = btn;

        const fileInput = document.createElement('input');
        fileInput.type = 'file';
        fileInput.multiple = true;
        fileInput.style.display = 'none';
        document.body.appendChild(fileInput);

        btn.addEventListener('click', (e) => {
            e.preventDefault();
            e.stopPropagation();
            fileInput.click();
        });

        fileInput.addEventListener('change', async () => {
            const files = fileInput.files;
            if (!files || files.length === 0) return;
            for (const file of files) {
                await this.upload(file);
            }
            fileInput.value = '';
        });

        const audioBtn = document.getElementById('envpod_audio_btn');
        const insertBefore = audioBtn || disconnectBtn;
        if (insertBefore) {
            container.insertBefore(btn, insertBefore);
        } else {
            container.appendChild(btn);
        }
    },

    async upload(file) {
        const url = `${location.protocol}//${location.hostname}:${this.uploadPort}/`;
        this.btn.classList.add('noVNC_selected');
        const progressToast = this.toast(`Uploading ${file.name} (${this.fmtSize(file.size)})...`, 'info');
        try {
            const resp = await fetch(url, {
                method: 'POST',
                headers: { 'X-Filename': file.name },
                body: file,
            });
            const data = await resp.json();
            progressToast.remove();
            if (data.ok) {
                this.toast(`\u2713 ${data.file} (${this.fmtSize(data.size)}) \u2192 /tmp/uploads/`, 'ok');
            } else {
                this.toast(`\u2717 Upload failed: ${file.name}`, 'err');
            }
        } catch (err) {
            console.error('[envpod-upload] failed:', err);
            progressToast.remove();
            this.toast(`\u2717 Upload failed \u2014 server not reachable`, 'err');
        }
        this.btn.classList.remove('noVNC_selected');
    },

    fmtSize(b) {
        if (b < 1024) return b + ' B';
        if (b < 1048576) return (b / 1024).toFixed(1) + ' KB';
        return (b / 1048576).toFixed(1) + ' MB';
    }
};

window.addEventListener('load', () => EnvpodUpload.init());
"##;

/// Embedded clipboard-plugin.js — bidirectional clipboard sync between host and pod.
///
/// Host→Pod: intercepts paste events on the noVNC canvas, reads browser clipboard,
/// sends text to VNC via noVNC's clipboardPasteFrom() API, then simulates Ctrl+V.
/// Pod→Host: listens for VNC clipboard events and writes to browser clipboard.
///
/// Notes:
/// - Clipboard API requires secure context (HTTPS) or localhost — localhost works.
/// - Falls back gracefully if clipboard permission is denied.
const CLIPBOARD_PLUGIN_JS: &str = r##"/**
 * envpod clipboard plugin for noVNC
 * Bidirectional clipboard sync: host browser <-> VNC session
 */
const EnvpodClipboard = {
    rfb: null,
    ready: false,

    init() {
        // Wait for noVNC to connect, then hook into the RFB object
        const check = setInterval(() => {
            // noVNC stores the RFB instance on the UI object or as a global
            const rfb = window.rfb || (window.UI && window.UI.rfb);
            if (rfb && rfb._rfbConnectionState === 'connected') {
                this.rfb = rfb;
                this.ready = true;
                clearInterval(check);
                this.attach();
                console.log('[envpod-clipboard] attached to RFB');
            }
        }, 500);
    },

    attach() {
        // Pod→Host: VNC server sends clipboard content
        this.rfb.addEventListener('clipboard', (e) => {
            if (e.detail && e.detail.text) {
                navigator.clipboard.writeText(e.detail.text).catch(() => {});
            }
        });

        // Host→Pod: intercept paste on the noVNC canvas
        const canvas = document.getElementById('noVNC_canvas') ||
                       document.querySelector('canvas');
        if (!canvas) return;

        // Listen on the document for paste events (works when canvas is focused)
        document.addEventListener('paste', async (e) => {
            if (!this.ready) return;
            let text = '';
            if (e.clipboardData) {
                text = e.clipboardData.getData('text/plain');
            }
            if (!text) {
                try { text = await navigator.clipboard.readText(); } catch (_) {}
            }
            if (text) {
                e.preventDefault();
                e.stopPropagation();
                this.rfb.clipboardPasteFrom(text);
                // Simulate Ctrl+V in the VNC session so the app receives the paste
                this.rfb.sendKey(0xffe3, 'ControlLeft', true);   // Ctrl down
                this.rfb.sendKey(0x0076, 'KeyV', true);          // v down
                this.rfb.sendKey(0x0076, 'KeyV', false);         // v up
                this.rfb.sendKey(0xffe3, 'ControlLeft', false);  // Ctrl up
            }
        });

        // Also handle Ctrl+V keydown as a trigger to read clipboard
        // (some browsers fire keydown before paste event)
        document.addEventListener('keydown', async (e) => {
            if (!this.ready) return;
            if ((e.ctrlKey || e.metaKey) && e.key === 'v') {
                try {
                    const text = await navigator.clipboard.readText();
                    if (text) {
                        this.rfb.clipboardPasteFrom(text);
                    }
                } catch (_) {
                    // Clipboard permission denied — fall through to normal paste
                }
            }
        });
    }
};

window.addEventListener('load', () => EnvpodClipboard.init());
"##;

/// Parse resolution string into (width, height). Returns None if invalid.
pub fn parse_resolution(res: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = res.split('x').collect();
    if parts.len() == 2 {
        let w = parts[0].parse().ok()?;
        let h = parts[1].parse().ok()?;
        Some((w, h))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn novnc_setup_commands_no_audio() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Novnc,
            audio: false,
            ..Default::default()
        };
        let cmds = generate_setup_commands(&config);
        assert_eq!(cmds.len(), 2);
        assert!(cmds[0].contains("apt-get update"));
        assert!(cmds[1].contains("xvfb"));
        assert!(cmds[1].contains("x11vnc"));
        assert!(cmds[1].contains("websockify"));
        assert!(!cmds[1].contains("pulseaudio"));
    }

    #[test]
    fn novnc_setup_commands_with_audio() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Novnc,
            audio: true,
            ..Default::default()
        };
        let cmds = generate_setup_commands(&config);
        assert_eq!(cmds.len(), 2);
        assert!(cmds[1].contains("pulseaudio"));
        assert!(cmds[1].contains("socat"));
        assert!(cmds[1].contains("gstreamer"));
        assert!(cmds[1].contains("websockify"));
    }

    #[test]
    fn webrtc_setup_commands() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Webrtc,
            ..Default::default()
        };
        let cmds = generate_setup_commands(&config);
        assert_eq!(cmds.len(), 2);
        assert!(cmds[1].contains("gstreamer"));
        assert!(cmds[1].contains("xdotool"));
    }

    #[test]
    fn none_setup_commands_empty() {
        let config = WebDisplayConfig::default();
        assert!(generate_setup_commands(&config).is_empty());
    }

    #[test]
    fn novnc_script_no_audio() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Novnc,
            resolution: "1920x1080".into(),
            audio: false,
            ..Default::default()
        };
        // Wrapper script: lightweight, starts daemon + exec's user command
        let wrapper = generate_supervisor_script(&config);
        assert!(wrapper.contains("DISPLAY=:99"));
        assert!(wrapper.contains("envpod-display-services"));
        assert!(wrapper.contains("exec \"$@\""));
        // Daemon script: manages display services
        let daemon = generate_services_daemon(&config);
        assert!(daemon.contains("Xvfb :99"));
        assert!(daemon.contains("1920x1080"));
        assert!(daemon.contains("x11vnc"));
        assert!(daemon.contains("websockify"));
        assert!(daemon.contains("0.0.0.0:6080"));
        assert!(!daemon.contains("pulseaudio"));
        assert!(!daemon.contains("audio-proxy"));
    }

    #[test]
    fn novnc_script_with_audio() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Novnc,
            resolution: "1920x1080".into(),
            audio: true,
            audio_port: 6081,
            ..Default::default()
        };
        let daemon = generate_services_daemon(&config);
        assert!(daemon.contains("pulseaudio"));
        assert!(daemon.contains("envpod-audio-proxy"));
        assert!(daemon.contains("0.0.0.0:6081"));
        assert!(daemon.contains("module-null-sink"));
        assert!(daemon.contains("module-simple-protocol-tcp"));
    }

    #[test]
    fn audio_overlay_files_when_enabled() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Novnc,
            audio: true,
            audio_port: 6081,
            ..Default::default()
        };
        let files = audio_overlay_files(&config);
        assert_eq!(files.len(), 2);
        assert!(files[0].0.contains("audio-proxy"));
        assert!(files[0].2); // executable
        assert!(files[1].0.contains("audio-plugin"));
        assert!(!files[1].2); // not executable
        assert!(files[1].1.contains("6081")); // port baked in
    }

    #[test]
    fn audio_overlay_files_when_disabled() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Novnc,
            audio: false,
            ..Default::default()
        };
        assert!(audio_overlay_files(&config).is_empty());
    }

    #[test]
    fn upload_overlay_files_when_enabled() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Novnc,
            file_upload: true,
            upload_port: 5080,
            ..Default::default()
        };
        let files = upload_overlay_files(&config);
        assert_eq!(files.len(), 2);
        assert!(files[0].0.contains("upload-server"));
        assert!(files[0].2); // executable
        assert!(files[1].0.contains("upload-plugin"));
        assert!(!files[1].2); // not executable
        assert!(files[1].1.contains("5080")); // port baked in
    }

    #[test]
    fn upload_overlay_files_when_disabled() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Novnc,
            file_upload: false,
            ..Default::default()
        };
        assert!(upload_overlay_files(&config).is_empty());
    }

    #[test]
    fn novnc_script_with_upload() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Novnc,
            file_upload: true,
            ..Default::default()
        };
        let daemon = generate_services_daemon(&config);
        assert!(daemon.contains("envpod-upload-server.py"));
        assert!(daemon.contains("/tmp/uploads"));
    }

    #[test]
    fn novnc_script_without_upload() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Novnc,
            file_upload: false,
            ..Default::default()
        };
        let daemon = generate_services_daemon(&config);
        assert!(!daemon.contains("envpod-upload-server.py"));
    }

    #[test]
    fn webrtc_script_contains_gstreamer() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Webrtc,
            codec: "vp8".into(),
            audio: true,
            ..Default::default()
        };
        let daemon = generate_services_daemon(&config);
        assert!(daemon.contains("ximagesrc"));
        assert!(daemon.contains("vp8enc"));
        assert!(daemon.contains("pulsesrc"));
        assert!(daemon.contains("xdotool"));
        // Wrapper should be lightweight
        let wrapper = generate_supervisor_script(&config);
        assert!(wrapper.contains("envpod-display-services"));
        assert!(wrapper.contains("exec \"$@\""));
    }

    #[test]
    fn webrtc_h264_codec() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Webrtc,
            codec: "h264".into(),
            audio: false,
            ..Default::default()
        };
        let daemon = generate_services_daemon(&config);
        assert!(daemon.contains("x264enc"));
        assert!(!daemon.contains("pulsesrc"));
    }

    #[test]
    fn none_script_empty() {
        let config = WebDisplayConfig::default();
        assert!(generate_supervisor_script(&config).is_empty());
    }

    #[test]
    fn parse_resolution_valid() {
        assert_eq!(parse_resolution("1280x720"), Some((1280, 720)));
        assert_eq!(parse_resolution("1920x1080"), Some((1920, 1080)));
    }

    #[test]
    fn parse_resolution_invalid() {
        assert_eq!(parse_resolution("invalid"), None);
        assert_eq!(parse_resolution("1280"), None);
        assert_eq!(parse_resolution("axb"), None);
    }
}
