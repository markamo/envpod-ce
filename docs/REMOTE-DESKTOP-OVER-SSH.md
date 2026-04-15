# Remote Desktop Over SSH

> Copyright 2026 Xtellix Inc. · Business Source License 1.1

When envpod is running on a remote server, VM, or cloud host and you want to use the pod's desktop from your own laptop, the default path is **SSH local port forwarding** to the host's loopback display ports.

This keeps the desktop private to your SSH session. You do not need to expose the pod directly to the internet, and you do not need a reverse SSH tunnel.

## Use This When

- The pod is running on a remote server you can SSH into
- You want browser access from your local machine only
- You do not want to publish the desktop publicly

## Do Not Use This For

- `envpod ssh-proxy`

  `ssh-proxy` is for IDE and shell access only. It is a `ProxyCommand` bridge into the pod, not a browser transport. See [IDE.md](IDE.md).

- `remote.enabled`

  The remote control API is for governance and command execution, not desktop/browser streaming. See [REMOTE-CONTROL.md](REMOTE-CONTROL.md).

- `ssh -R`

  Reverse tunnels are usually the wrong direction here. The desktop service lives on the **server host**, so the normal pattern is `ssh -L` from your laptop to the server.

## Recommended Pattern: noVNC Over SSH

### 1. Start the desktop on the server

```bash
sudo envpod init my-desktop -c examples/desktop.yaml
sudo envpod setup my-desktop
sudo envpod start my-desktop
```

The host exposes the noVNC service on `127.0.0.1:6080` by default.

### 2. Forward the port to your laptop

Run this from your **local machine**, not inside the pod:

```bash
ssh -N -L 6080:127.0.0.1:6080 user@your-server
```

### 3. Open the desktop locally

Open:

```text
http://localhost:6080/vnc.html
```

That browser tab is reaching the pod desktop through your SSH session.

## Audio and File Upload

If the pod enables noVNC audio or drag-and-drop upload, forward those ports too:

```bash
ssh -N \
  -L 6080:127.0.0.1:6080 \
  -L 6081:127.0.0.1:6081 \
  -L 5080:127.0.0.1:5080 \
  user@your-server
```

Open the same desktop URL:

```text
http://localhost:6080/vnc.html
```

Port map:

| Local Port | Remote Port | Purpose |
|-----------|-------------|---------|
| `6080` | `127.0.0.1:6080` | noVNC HTML + WebSocket |
| `6081` | `127.0.0.1:6081` | Audio stream |
| `5080` | `127.0.0.1:5080` | File upload helper |

## WebRTC Desktops

If the pod uses `web_display.type: webrtc`, forward the WebRTC and UI ports instead:

```bash
ssh -N \
  -L 8443:127.0.0.1:8443 \
  -L 8444:127.0.0.1:8444 \
  -L 8445:127.0.0.1:8445 \
  -L 8446:127.0.0.1:8446 \
  user@your-server
```

Then open one of:

```text
http://localhost:8443/desktop/
http://localhost:8446/
```

Use this only when the pod is configured for WebRTC. Port details are in [WEB-DISPLAY.md](WEB-DISPLAY.md).

## Multiple Pods or Port Collisions

If you already use local port `6080`, bind a different local port:

```bash
ssh -N -L 16080:127.0.0.1:6080 user@your-server
```

Then open:

```text
http://localhost:16080/vnc.html
```

The left side of `-L` is your laptop port. The right side stays the envpod host port.

## Bastion / Jump Host

If the server is only reachable through a bastion:

```bash
ssh -J user@bastion.example.com -N \
  -L 6080:127.0.0.1:6080 \
  user@envpod-server.internal
```

Then open:

```text
http://localhost:6080/vnc.html
```

## When To Use `publish` Instead

Use [PUBLISH.md](PUBLISH.md) when:

- You need access without an SSH session
- You want to share the desktop with someone else
- You need a public URL

Example:

```bash
sudo envpod publish my-desktop -d
```

That is the internet-facing workflow. SSH local forwarding is the private server-admin workflow.

## Troubleshooting

### Browser does not connect

Verify the pod is started on the server:

```bash
sudo envpod start my-desktop
```

Then check the host-local endpoint:

```bash
curl -I http://127.0.0.1:6080/vnc.html
```

### Desktop works but audio or upload is missing

You probably forwarded `6080` only. Add `6081` for audio and `5080` for upload.

### I used `envpod ssh-proxy` and nothing opened

That is expected. `ssh-proxy` is not a desktop transport. Use `ssh -L` to the host display port instead.

### I tried `ssh -R`

Use `ssh -L` unless you specifically need the **server** to initiate a connection back to your workstation. For normal laptop-to-server desktop access, local forwarding is the correct direction.

## Related Docs

- [WEB-DISPLAY.md](WEB-DISPLAY.md)
- [PUBLISH.md](PUBLISH.md)
- [IDE.md](IDE.md)
- [REMOTE-CONTROL.md](REMOTE-CONTROL.md)
