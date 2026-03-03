<!-- type-delay 0.03 -->
# Demo 1: "Install to First Pod in 60 Seconds"

**Platform:** LinkedIn / X teaser
**Length:** ~60s
**Audience:** Developers curious about envpod, first impression
**Setup:** Clean Ubuntu 24.04 terminal, dark theme, large font (16pt+)

---

## Script

**[0:00]** Open browser → envpod.dev. Show hero: "Docker isolates. Envpod governs."

**[0:08]** Click "Copy" on the install one-liner. Switch to terminal.

**[0:12]** Paste and run:
<!-- no-exec -->
![Script](assets/01-script.svg)

```bash
curl -fsSL https://envpod.dev/install.sh | sh
```
*Narration: "Single binary, 5 megs, no dependencies. x86 and ARM64."*

**[0:20]** Show available presets:
<!-- no-exec -->
![Script](assets/02-script.svg)

```bash
envpod presets
```
*Narration: "18 built-in presets — coding agents, browsers, frameworks, environments."*

**[0:26]** Create a pod with a preset:
<!-- no-exec -->
![Script](assets/03-script.svg)

```bash
sudo envpod init hello --preset devbox
```
*Narration: "One command. Pick a preset. Pod created."*

**[0:32]** Run the agent — it writes a file:
<!-- no-exec -->
<!-- type-delay 0.02 -->
![Script](assets/04-script.svg)

```bash
sudo envpod run hello -- bash -c "echo 'the agent wrote this' > /home/agent/hello.txt && echo 'done'"
```
*Narration: "The agent thinks it wrote to your filesystem. It didn't."*

**[0:40]** Show the diff:
<!-- no-exec -->
![Script](assets/05-script.svg)

```bash
sudo envpod diff hello
```
<!-- pause 2 -->
*Narration: "Every change goes to a copy-on-write overlay. You review before anything touches the host."*

**[0:48]** Commit or rollback:
<!-- no-exec -->
<!-- type-delay 0.02 -->
![Script](assets/06-script.svg)

```bash
sudo envpod commit hello
# or: sudo envpod rollback hello
```
*Narration: "Commit what you want. Roll back the rest. That's governance."*

**[0:54]** Show the audit log:
<!-- no-exec -->
![Script](assets/07-script.svg)

```bash
sudo envpod audit hello
```
<!-- pause 2 -->
*Narration: "Every action logged. Append-only. Free and open source."*

**[0:58]** End card: `github.com/markamo/envpod-ce`

---

## Commands (copy-paste)

<!-- no-exec -->
<!-- type-delay 0.02 -->
![Commands (copy-paste)](assets/08-commands-copy-paste.svg)

```bash
curl -fsSL https://envpod.dev/install.sh | sh
envpod presets
sudo envpod init hello --preset devbox
sudo envpod run hello -- bash -c "echo 'the agent wrote this' > /home/agent/hello.txt && echo 'done'"
sudo envpod diff hello
sudo envpod commit hello
sudo envpod audit hello
```


<p align="center"><sub>Made with <a href="https://github.com/markamo/md2cast">md2cast</a></sub></p>
