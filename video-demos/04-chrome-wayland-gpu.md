<!-- type-delay 0.03 -->
# Demo 4: "Chrome in a Pod — Wayland + GPU"

**Platform:** YouTube / X — most visually impressive demo
**Length:** ~90s
**Audience:** Anyone — visual wow factor
**Setup:** Ubuntu 24.04 with Wayland session (GNOME), PipeWire, Chrome installed on host

---

## Script

**[0:00]** *Narration: "A browser agent needs GPU, display, and audio. Here's how envpod does it without Docker."*

**[0:08]** Show the preset and config:
<!-- no-exec -->
```bash
envpod presets | grep browser
```
*Highlight: 3 browser presets — browser, browser-use, playwright.*

<!-- no-exec -->
```bash
cat examples/browser-wayland.yaml
```
*Highlight: `gpu: true`, `display: true`, `audio: true`, `display_protocol: wayland`, `audio_protocol: pipewire`, browser seccomp, 4GB RAM.*

**[0:22]** Init with the config:
<!-- no-exec -->
```bash
sudo envpod init browser -c examples/browser-wayland.yaml
```

**[0:30]** Launch Chrome inside the pod with display and audio forwarding:
<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
sudo envpod run browser -d -a -- google-chrome --no-sandbox --ozone-platform=wayland https://youtube.com
```
*Chrome window appears on the desktop. Navigate YouTube. Play a video — audio works.*

*Narration: "Full Wayland display forwarding. GPU-accelerated rendering. PipeWire audio. All inside a governed pod."*

**[0:55]** Show what's different from Docker:
<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
sudo envpod diff browser
sudo envpod audit browser
```
<!-- pause 2 -->
*Narration: "Every file Chrome wrote — cookies, cache, preferences — captured in the overlay. Audit log shows every network domain it contacted."*

**[1:10]** Show security comparison:
<!-- no-exec -->
```bash
sudo envpod audit --security -c examples/browser-wayland.yaml
```
<!-- pause 2 -->
*Narration: "Wayland + PipeWire: display is LOW risk, audio is MEDIUM. Compare that to X11 — CRITICAL, because X11 allows keylogging across windows."*

**[1:25]** Rollback everything:
<!-- no-exec -->
```bash
sudo envpod rollback browser
```
*Narration: "One command. All traces gone. No base image. No container OS. Just your host, governed."*

**[1:35]** End card.

---

## Commands (copy-paste)

<!-- no-exec -->
<!-- type-delay 0.02 -->
```bash
envpod presets | grep browser
cat examples/browser-wayland.yaml
sudo envpod init browser -c examples/browser-wayland.yaml
sudo envpod run browser -d -a -- google-chrome --no-sandbox --ozone-platform=wayland https://youtube.com
sudo envpod diff browser
sudo envpod audit browser
sudo envpod audit --security -c examples/browser-wayland.yaml
sudo envpod rollback browser
```
