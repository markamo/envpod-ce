# Device Passthrough

envpod pods run with a minimal `/dev` tree by default — only essential pseudo-devices are exposed. Hardware devices must be explicitly opted in via `pod.yaml`.

## How It Works

envpod replaces the host `/dev` with a clean tmpfs containing only what the pod needs:

1. Mount tmpfs on `/dev` (5MB, mode 0755)
2. Bind-mount essential pseudo-devices (null, zero, full, random, urandom, tty)
3. Set up devpts for PTY/terminal support
4. Create standard symlinks (stdin, stdout, stderr, fd)
5. Bind-mount opted-in hardware devices (GPU, audio, extra)
6. Mount pod-private `/dev/shm`

Devices not explicitly enabled are invisible inside the pod. GPU info in `/proc` and `/sys` is also masked when GPU access is denied — agents cannot fingerprint host hardware.

## First-Class Devices

These have dedicated support with auto-detection and protocol selection.

### GPU

```yaml
devices:
  gpu: true
```

Bind-mounts NVIDIA and DRI device nodes:

| Device | Purpose |
|---|---|
| `/dev/nvidia0`–`nvidia3` | NVIDIA GPU compute |
| `/dev/nvidiactl` | NVIDIA control |
| `/dev/nvidia-modeset` | Display mode setting |
| `/dev/nvidia-uvm` | Unified virtual memory |
| `/dev/nvidia-uvm-tools` | UVM debugging tools |
| `/dev/dri/card0`–`card1` | DRM graphics cards |
| `/dev/dri/renderD128`–`renderD129` | DRM render nodes |

When `gpu: false` (default), envpod also masks `/proc/driver/nvidia`, `/sys/module/nvidia`, `/sys/class/drm`, and `/sys/bus/pci/drivers/nvidia` with empty read-only tmpfs.

**Use cases:** AI/ML inference, model training, 3D rendering, video encoding (NVENC), game streaming.

### Display

```yaml
devices:
  display: true
  display_protocol: auto    # auto | x11 | wayland
```

Mounts the host display socket into the pod:

| Protocol | Socket | When |
|---|---|---|
| Wayland | `/tmp/wayland-0` | `WAYLAND_DISPLAY` set on host |
| X11 | `/tmp/.X11-unix/X0` | `DISPLAY` set on host |
| Auto | whichever exists | default |

For browser-based display (no host display needed), use `web_display` instead:

```yaml
web_display:
  display_type: novnc       # novnc | webrtc
  resolution: "1920x1080"
  audio: true
  audio_port: 6081
```

**Use cases:** GUI applications, IDEs, browsers, desktop environments, VDI.

### Audio

```yaml
devices:
  audio: true
  audio_protocol: auto      # auto | pipewire | pulseaudio
```

Bind-mounts ALSA sound device nodes (`/dev/snd/*`) and audio server sockets:

| Protocol | Socket | When |
|---|---|---|
| PipeWire | `/tmp/pipewire-0` | PipeWire running on host |
| PulseAudio | `/tmp/pulse-native` | PulseAudio running on host |
| Auto | whichever exists | default |

ALSA devices mounted:

| Device | Purpose |
|---|---|
| `/dev/snd/controlC0`–`C1` | Mixer control |
| `/dev/snd/pcmC0D0p` | PCM playback |
| `/dev/snd/pcmC0D0c` | PCM capture (microphone) |
| `/dev/snd/seq` | MIDI sequencer |
| `/dev/snd/timer` | Timer |

**Use cases:** Media playback, voice AI, call centers, music production, game audio, telehealth.

## Extra Devices

For any device not covered by first-class support, use the `extra` field:

```yaml
devices:
  extra:
    - "/dev/fuse"
    - "/dev/kvm"
```

This bind-mounts the specified device nodes from the host into the pod. Devices that don't exist on the host are silently skipped.

### Common Extra Devices

| Device | Path | Use case |
|---|---|---|
| **Camera** | `/dev/video0` | Webcam, video capture, computer vision |
| **KVM** | `/dev/kvm` | Nested virtualization, Android emulation |
| **FUSE** | `/dev/fuse` | Userspace filesystems (sshfs, s3fs) |
| **USB Serial** | `/dev/ttyUSB0` | IoT devices, Arduino, lab equipment |
| **Serial** | `/dev/ttyACM0` | GPS receivers, modems, microcontrollers |
| **USB Generic** | `/dev/bus/usb/001/002` | USB passthrough (specific device) |
| **Tape** | `/dev/st0` | Backup drives |
| **Loop** | `/dev/loop0` | Disk image mounting |
| **Printer** | `/dev/usb/lp0` | USB printing |
| **Scanner** | `/dev/sg0` | SCSI scanner devices |
| **SDR** | `/dev/swradio0` | Software defined radio |
| **TPM** | `/dev/tpm0` | Hardware security module |
| **Sensors** | `/dev/iio:device0` | Industrial IoT, environmental sensors |
| **Infiniband** | `/dev/infiniband/uverbs0` | HPC networking |
| **VFIO** | `/dev/vfio/vfio` | PCIe device passthrough |

### Multiple Cameras

```yaml
devices:
  extra:
    - "/dev/video0"
    - "/dev/video1"
    - "/dev/video2"
```

### IoT Gateway

```yaml
devices:
  extra:
    - "/dev/ttyUSB0"     # Zigbee coordinator
    - "/dev/ttyACM0"     # Z-Wave stick
    - "/dev/ttyACM1"     # GPS receiver
```

## Example Configurations

### AI Agent with GPU

```yaml
name: ai-agent
devices:
  gpu: true

processor:
  cores: 4.0
  memory: "16GB"
```

### Desktop Environment with Display + Audio

```yaml
name: dev-desktop
devices:
  display: true
  audio: true
  gpu: true
  display_protocol: wayland
  audio_protocol: pipewire
  desktop_env: xfce
```

### Web-Based Desktop (VDI)

```yaml
name: remote-desktop
devices:
  gpu: true
  audio: true

web_display:
  display_type: novnc
  resolution: "1920x1080"
  audio: true
```

Access via browser: `http://<pod-ip>:6080/vnc.html`

### Computer Vision with Camera + GPU

```yaml
name: cv-pipeline
devices:
  gpu: true
  extra:
    - "/dev/video0"

processor:
  cores: 4.0
  memory: "8GB"
```

### IoT Hub

```yaml
name: iot-gateway
devices:
  extra:
    - "/dev/ttyUSB0"
    - "/dev/ttyACM0"
    - "/dev/ttyACM1"

network:
  mode: Monitored
  dns:
    mode: Allowlist
    allow:
      - mqtt.example.com
```

### Nested Virtualization

```yaml
name: android-emulator
devices:
  gpu: true
  display: true
  extra:
    - "/dev/kvm"

processor:
  cores: 4.0
  memory: "8GB"

security:
  shm_size: "256MB"
```

## Security Model

### Default-Deny

No hardware devices are exposed unless explicitly configured. A pod with no `devices` section sees only pseudo-devices (null, zero, random, etc.).

### GPU Info Masking

When `gpu: false`, envpod masks GPU information in `/proc` and `/sys` with empty read-only tmpfs. Agents cannot detect or fingerprint host GPU hardware.

### Minimal /dev

The pod `/dev` is a tmpfs — not a bind-mount of the host `/dev`. Only explicitly listed devices are bind-mounted in. The pod cannot see or access any device not in its configuration.

### Extra Device Risks

Devices in the `extra` field are bind-mounted directly. Consider the risk level:

| Risk | Devices | Mitigation |
|---|---|---|
| Low | `/dev/fuse`, `/dev/loop*` | Read-only filesystem access |
| Medium | `/dev/video*`, `/dev/snd/*` | Can capture audio/video from host |
| Medium | `/dev/ttyUSB*`, `/dev/ttyACM*` | Can communicate with connected hardware |
| High | `/dev/kvm` | Can create VMs (potential escape vector) |
| High | `/dev/vfio/*` | Direct PCIe access (DMA capable) |
| High | `/dev/mem`, `/dev/kmem` | Direct memory access (NEVER passthrough) |

**Never passthrough:** `/dev/mem`, `/dev/kmem`, `/dev/port` — these allow direct host memory access and bypass all isolation.

### Governance

All device access is governed by envpod:
- **Audit** — device mount events logged in audit trail
- **Network** — devices that generate network traffic are subject to DNS/network rules
- **Filesystem** — device-generated files tracked by COW filesystem
- **Policy** — OPA policies can gate device access per-agent (Premium)

## Discovering Host Devices

List available devices on the host:

```bash
# All video devices (cameras)
ls /dev/video*

# All serial/USB devices
ls /dev/ttyUSB* /dev/ttyACM*

# GPU devices
ls /dev/nvidia* /dev/dri/*

# Sound devices
ls /dev/snd/*

# All character devices
find /dev -type c | sort
```
