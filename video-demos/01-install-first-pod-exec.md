<!-- type-delay 0.03 -->

# Install to First Pod in 60 Seconds

> Docker isolates. Envpod governs.

---

## Install

<!-- no-exec -->
```bash
curl -fsSL https://envpod.dev/install.sh | sh
```

```
envpod v0.1.0 installed to /usr/local/bin/envpod
```

## Show Presets

<!-- exec -->
```bash
envpod presets
```

---

## Create a Pod

<!-- exec -->
```bash
sudo envpod init hello --preset devbox
```

## Run a Command Inside

The agent thinks it wrote to your filesystem. It didn't.

<!-- exec -->
```bash
sudo envpod run hello -- bash -c "echo 'the agent wrote this' > /home/agent/hello.txt && echo 'wrote /home/agent/hello.txt inside pod'"
```

---

## Review Changes

Every change goes to a copy-on-write overlay. You review before anything touches the host.

<!-- exec -->
```bash
sudo envpod diff hello
```

<!-- pause 2 -->

## Commit

Commit what you want. Roll back the rest. That's governance.

<!-- exec -->
```bash
sudo envpod commit hello
```

---

## Audit Trail

Every action logged. Append-only. Free and open source.

<!-- exec -->
```bash
sudo envpod audit hello
```

<!-- pause 2 -->

## Cleanup

<!-- exec -->
```bash
sudo envpod destroy hello
```

> github.com/markamo/envpod-ce
