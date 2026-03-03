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
    match config.display_type {
        WebDisplayType::None => Vec::new(),
        WebDisplayType::Novnc => vec![
            "cd /etc/apt/sources.list.d && for f in *.list *.sources; do case \"$f\" in ubuntu*) ;; *) rm -f \"$f\" ;; esac; done 2>/dev/null; dpkg --configure -a 2>/dev/null; apt-get update -qq".into(),
            "DEBIAN_FRONTEND=noninteractive apt-get install -y xvfb x11vnc novnc websockify".into(),
        ],
        WebDisplayType::Webrtc => vec![
            "cd /etc/apt/sources.list.d && for f in *.list *.sources; do case \"$f\" in ubuntu*) ;; *) rm -f \"$f\" ;; esac; done 2>/dev/null; dpkg --configure -a 2>/dev/null; apt-get update -qq".into(),
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
    format!(
        r#"#!/bin/bash
# envpod web display supervisor (noVNC)

# Prevent NVIDIA EGL/GBM from loading (causes Xvfb segfault on GPU hosts)
export __EGL_VENDOR_LIBRARY_FILENAMES=""
export __GLX_VENDOR_LIBRARY_NAME=mesa
export DISPLAY=:99

# Cleanup on exit
cleanup() {{
    kill $WEBSOCKIFY_PID $X11VNC_PID $XVFB_PID 2>/dev/null || true
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
    fn novnc_setup_commands() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Novnc,
            ..Default::default()
        };
        let cmds = generate_setup_commands(&config);
        assert_eq!(cmds.len(), 2);
        assert!(cmds[0].contains("apt-get update"));
        assert!(cmds[1].contains("xvfb"));
        assert!(cmds[1].contains("x11vnc"));
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
    fn novnc_script_contains_key_services() {
        let config = WebDisplayConfig {
            display_type: WebDisplayType::Novnc,
            resolution: "1920x1080".into(),
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
