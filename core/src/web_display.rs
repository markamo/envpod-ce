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
    let apt_cleanup = "cd /etc/apt/sources.list.d && for f in *.list *.sources; do case \"$f\" in ubuntu*) ;; *) rm -f \"$f\" ;; esac; done 2>/dev/null; dpkg --configure -a 2>/dev/null; apt-get update -qq";
    match config.display_type {
        WebDisplayType::None => Vec::new(),
        WebDisplayType::Novnc => {
            let mut cmds = vec![
                apt_cleanup.into(),
            ];
            if config.audio {
                cmds.push(concat!(
                    "DEBIAN_FRONTEND=noninteractive apt-get install -y ",
                    "xvfb x11vnc novnc websockify ",
                    "pulseaudio socat ",
                    "gstreamer1.0-tools gstreamer1.0-plugins-base ",
                    "gstreamer1.0-plugins-good gstreamer1.0-plugins-bad"
                ).into());
            } else {
                cmds.push(
                    "DEBIAN_FRONTEND=noninteractive apt-get install -y xvfb x11vnc novnc websockify".into()
                );
            }
            cmds
        }
        WebDisplayType::Webrtc => vec![
            apt_cleanup.into(),
            concat!(
                "DEBIAN_FRONTEND=noninteractive apt-get install -y -qq ",
                "xvfb xdotool ",
                "gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good ",
                "gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-nice ",
                "gstreamer1.0-pulseaudio ",
                "> /dev/null 2>&1"
            ).into(),
        ],
    }
}

/// Generate the supervisor shell script that starts display services,
/// then execs the user command (passed as arguments).
///
/// Written to `upper/usr/local/bin/envpod-display-start` during init.
pub fn generate_supervisor_script(config: &WebDisplayConfig) -> String {
    match config.display_type {
        WebDisplayType::None => String::new(),
        WebDisplayType::Novnc => generate_novnc_script(config),
        WebDisplayType::Webrtc => generate_webrtc_script(config),
    }
}

fn generate_novnc_script(config: &WebDisplayConfig) -> String {
    let resolution = &config.resolution;
    let audio_port = config.audio_port;

    let audio_block = if config.audio {
        format!(r#"
# --- Audio streaming (PulseAudio + Opus/WebM → WebSocket) ---

# Ensure pulse user/group exists (PulseAudio system mode requires it)
id pulse >/dev/null 2>&1 || useradd --system --no-create-home -s /usr/sbin/nologin pulse 2>/dev/null
getent group pulse-access >/dev/null 2>&1 || groupadd --system pulse-access 2>/dev/null
usermod -aG pulse-access root 2>/dev/null
mkdir -p /var/run/pulse /var/lib/pulse
chown pulse:pulse /var/run/pulse /var/lib/pulse 2>/dev/null

# Configure PulseAudio: allow anonymous access (pod is already isolated)
mkdir -p /etc/pulse/system.pa.d
cat > /etc/pulse/system.pa.d/envpod.pa << 'PACONF'
load-module module-native-protocol-unix auth-anonymous=1
PACONF

# Start PulseAudio in system mode (required for running as root)
pulseaudio --system --daemonize --no-cpu-limit --log-target=file:/tmp/pulseaudio.log
sleep 1

# Set PULSE_SERVER so all child processes (Chrome, Firefox, etc.) can find PulseAudio
export PULSE_SERVER=unix:/var/run/pulse/native

# Load virtual sink + raw TCP output via pactl
pactl load-module module-null-sink sink_name=envpod format=s16le channels=2 rate=48000 sink_properties=device.description=envpod
pactl set-default-sink envpod
pactl load-module module-simple-protocol-tcp listen=127.0.0.1 port=4711 format=s16le rate=48000 channels=2 record=true source=envpod.monitor

# Lock monitor source at 100% — only the speaker slider controls audio volume.
# The XFCE mixer shows both a speaker and mic slider; both independently
# affect the stream. The watchdog below re-locks the monitor instantly
# whenever the user touches the mic slider, making it a no-op.
pactl set-source-volume envpod.monitor 65536
(pactl subscribe 2>/dev/null | while read -r line; do
    case "$line" in *source*) pactl set-source-volume envpod.monitor 65536 2>/dev/null;; esac
done) &

# Start audio proxy (GStreamer: raw PCM → Opus/WebM)
/usr/local/bin/envpod-audio-proxy.sh -l 5711 &
AUDIO_PROXY_PID=$!
sleep 0.5

# Start websockify for audio WebSocket on port {audio_port}
websockify 0.0.0.0:{audio_port} localhost:5711 &
AUDIO_WS_PID=$!
"#)
    } else {
        "\nAUDIO_PROXY_PID=\nAUDIO_WS_PID=\n".to_string()
    };

    let audio_cleanup = if config.audio {
        "$AUDIO_WS_PID $AUDIO_PROXY_PID "
    } else {
        ""
    };

    format!(
        r#"#!/bin/bash
# envpod web display supervisor (noVNC)

# Prevent NVIDIA EGL/GBM from loading (causes Xvfb segfault on GPU hosts)
export __EGL_VENDOR_LIBRARY_FILENAMES=""
export __GLX_VENDOR_LIBRARY_NAME=mesa
export DISPLAY=:99

# Cleanup on exit
cleanup() {{
    kill {audio_cleanup}$WEBSOCKIFY_PID $X11VNC_PID $XVFB_PID 2>/dev/null || true
}}
trap cleanup EXIT

# Start Xvfb virtual display
Xvfb :99 -screen 0 {resolution}x24 -ac -noreset 2>/dev/null &
XVFB_PID=$!

# Wait for Xvfb to be ready (check for X socket)
for i in $(seq 1 20); do
    [ -e /tmp/.X11-unix/X99 ] && break
    sleep 0.25
done

# Start x11vnc connecting to the virtual display
x11vnc -display :99 -forever -nopw -shared -noshm -rfbport 5900 -q &
X11VNC_PID=$!
sleep 1

# Start websockify to bridge VNC to WebSocket
websockify --web /usr/share/novnc 0.0.0.0:6080 localhost:5900 &
WEBSOCKIFY_PID=$!
{audio_block}
# Execute the user command, redirecting its output to a log file
# so GUI app noise (Chrome, Firefox, etc.) doesn't flood the terminal.
exec "$@" >/tmp/envpod-display-app.log 2>&1
"#
    )
}

fn generate_webrtc_script(config: &WebDisplayConfig) -> String {
    let resolution = &config.resolution;
    let codec_pipeline = match config.codec.as_str() {
        "h264" => "x264enc tune=zerolatency speed-preset=ultrafast ! video/x-h264,profile=baseline ! rtph264pay",
        _ => "vp8enc deadline=1 target-bitrate=2000000 ! rtpvp8pay",
    };
    let audio_pipeline = if config.audio {
        "\n# Start audio capture pipeline\ngst-launch-1.0 -q pulsesrc ! opusenc ! rtpopuspay ! webrtcbin name=audio-send &\nAUDIO_PID=$!"
    } else {
        "\nAUDIO_PID="
    };
    let audio_cleanup = if config.audio { "$AUDIO_PID " } else { "" };

    format!(
        r#"#!/bin/bash
# envpod web display supervisor (WebRTC/GStreamer)
set -e

# Start Xvfb virtual display
Xvfb :99 -screen 0 {resolution}x24 -ac +extension GLX +render -noreset &
XVFB_PID=$!
sleep 0.5

export DISPLAY=:99

# Start video capture pipeline
gst-launch-1.0 -q ximagesrc use-damage=0 ! videoconvert ! {codec_pipeline} ! webrtcbin name=video-send &
VIDEO_PID=$!
{audio_pipeline}

# Start xdotool input relay (reads commands from a named pipe)
INPUTPIPE=/tmp/envpod-input
mkfifo "$INPUTPIPE" 2>/dev/null || true
(while read -r cmd < "$INPUTPIPE"; do eval "$cmd"; done) &
INPUT_PID=$!

# Cleanup on exit
cleanup() {{
    kill {audio_cleanup}$VIDEO_PID $INPUT_PID $XVFB_PID 2>/dev/null || true
    rm -f "$INPUTPIPE"
}}
trap cleanup EXIT

# Execute the user command, redirecting its output to a log file
exec "$@" >/tmp/envpod-display-app.log 2>&1
"#
    )
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
        const bar = document.getElementById('noVNC_control_bar_anchor');
        if (!bar) { console.warn('[envpod-audio] control bar not found'); return; }

        const btn = document.createElement('img');
        btn.id = 'envpod_audio_btn';
        btn.alt = 'Audio';
        btn.title = 'Enable audio';
        btn.style.cssText = 'width:24px;height:24px;padding:4px;cursor:pointer;opacity:0.5;';
        btn.src = 'data:image/svg+xml,' + encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z"/></svg>');
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

        bar.insertBefore(btn, bar.firstChild);
    },

    updateBtn(on) {
        if (!this.btn) return;
        this.btn.style.opacity = on ? '1' : '0.5';
        this.btn.title = on ? 'Disable audio' : 'Enable audio';
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
        let script = generate_supervisor_script(&config);
        assert!(script.contains("Xvfb :99"));
        assert!(script.contains("1920x1080"));
        assert!(script.contains("x11vnc"));
        assert!(script.contains("websockify"));
        assert!(script.contains("0.0.0.0:6080"));
        assert!(script.contains("DISPLAY=:99"));
        assert!(script.contains("exec \"$@\""));
        assert!(!script.contains("pulseaudio"));
        assert!(!script.contains("audio-proxy"));
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
        let script = generate_supervisor_script(&config);
        assert!(script.contains("pulseaudio"));
        assert!(script.contains("envpod-audio-proxy"));
        assert!(script.contains("0.0.0.0:6081"));
        assert!(script.contains("module-null-sink"));
        assert!(script.contains("module-simple-protocol-tcp"));
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
    fn webrtc_script_contains_gstreamer() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Webrtc,
            codec: "vp8".into(),
            audio: true,
            ..Default::default()
        };
        let script = generate_supervisor_script(&config);
        assert!(script.contains("ximagesrc"));
        assert!(script.contains("vp8enc"));
        assert!(script.contains("pulsesrc"));
        assert!(script.contains("xdotool"));
    }

    #[test]
    fn webrtc_h264_codec() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Webrtc,
            codec: "h264".into(),
            audio: false,
            ..Default::default()
        };
        let script = generate_supervisor_script(&config);
        assert!(script.contains("x264enc"));
        assert!(!script.contains("pulsesrc"));
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
