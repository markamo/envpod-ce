<!-- type-delay 0.03 -->
# Demo 2: "Claude Code — Governed"

**Platform:** YouTube / LinkedIn full walkthrough
**Length:** ~3 min
**Audience:** Developers using AI coding agents
**Setup:** Ubuntu 24.04, a small project directory (e.g. a Node.js app), terminal

---

## Script

**[0:00]** *Narration: "I run Claude Code every day. Here's the problem: it has full access to my filesystem, my API keys, my git credentials. If something goes wrong — good luck."*

**[0:15]** Show the preset (no config file needed):
<!-- no-exec -->
![Script](assets/01-script.svg)

```bash
envpod presets
```
*Highlight the claude-code preset. Narration: "18 presets built in. Claude Code is one of them — pre-configured DNS whitelist, resource limits, browser seccomp."*

**[0:30]** Init with preset:
<!-- no-exec -->
![Script](assets/02-script.svg)

```bash
sudo envpod init claude-code --preset claude-code
```
*Narration: "One command. Setup runs automatically — installs Claude CLI inside the overlay."*

**[0:50]** Store the API key in the vault:
<!-- no-exec -->
![Script](assets/03-script.svg)

```bash
sudo envpod vault claude-code set ANTHROPIC_API_KEY
# (pastes key, not echoed)
```
*Narration: "The key goes into an encrypted vault — ChaCha20-Poly1305. The agent gets it as an env var at runtime but it never touches disk in plaintext."*

**[1:05]** Run Claude Code:
<!-- no-exec -->
![Script](assets/04-script.svg)

```bash
sudo envpod run claude-code -- claude
```
*Show Claude Code starting up inside the pod. Give it a task — e.g. "add input validation to server.js".*

**[1:35]** While Claude works, open a second terminal. Show live monitoring:
<!-- no-exec -->
![Script](assets/05-script.svg)

```bash
sudo envpod audit claude-code
```
<!-- pause 2 -->
*Narration: "Real-time audit log. Every file write, every network call, every tool invocation."*

**[1:50]** Claude finishes. Review the diff:
<!-- no-exec -->
![Script](assets/06-script.svg)

```bash
sudo envpod diff claude-code
```
<!-- pause 2 -->
*Narration: "Here's everything Claude changed. Green = added, red = deleted. Nothing reached my real codebase yet."*

**[2:10]** Commit the good changes:
<!-- no-exec -->
![Script](assets/07-script.svg)

```bash
sudo envpod commit claude-code
```
*Narration: "Now it's on the host. If I didn't like it — rollback. Zero risk."*

**[2:25]** Show the security audit:
<!-- no-exec -->
![Script](assets/08-script.svg)

```bash
sudo envpod audit claude-code --security
```
<!-- pause 2 -->
*Narration: "Static analysis of the pod config. Tells you exactly what attack surface you're exposing."*

**[2:40]** Show cloning — spin up another instance instantly:
<!-- no-exec -->
<!-- type-delay 0.02 -->
![Script](assets/09-script.svg)

```bash
sudo envpod clone claude-code claude-code-2
sudo envpod run claude-code-2 -- echo "I'm a separate pod"
```
*Narration: "Clone in 130 milliseconds. Same setup, independent state. Scale your agents."*

**[2:50]** End card. *Narration: "Docker isolates. Envpod governs."*

---

## Commands (copy-paste)

<!-- no-exec -->
<!-- type-delay 0.02 -->
![Commands (copy-paste)](assets/10-commands-copy-paste.svg)

```bash
envpod presets
sudo envpod init claude-code --preset claude-code
sudo envpod vault claude-code set ANTHROPIC_API_KEY
sudo envpod run claude-code -- claude
sudo envpod audit claude-code
sudo envpod diff claude-code
sudo envpod commit claude-code
sudo envpod audit claude-code --security
sudo envpod clone claude-code claude-code-2
sudo envpod run claude-code-2 -- echo "I'm a separate pod"
```


<p align="center"><sub>Made with <a href="https://github.com/markamo/md2cast">md2cast</a></sub></p>
