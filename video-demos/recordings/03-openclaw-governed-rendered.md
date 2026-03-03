<!-- type-delay 0.03 -->
# Demo 3: "OpenClaw — Messaging Agent, Finally Governed"

**Platform:** YouTube / LinkedIn feature deep-dive
**Length:** ~2.5 min
**Audience:** Developers running autonomous agents (not just coding agents)
**Setup:** Ubuntu 24.04, terminal

---

## Script

**[0:00]** *Narration: "OpenClaw connects to WhatsApp, Telegram, Discord — and talks to LLMs on your behalf. That's a lot of power. Let's govern it."*

**[0:12]** Show the interactive wizard:
<!-- no-exec -->
![Script](assets/01-script.svg)

```bash
sudo envpod init openclaw
```
*The wizard shows 18 presets in 4 categories. Select "openclaw".*

*Narration: "Don't know config files? The interactive wizard lets you pick a preset, customize CPU and memory, and go."*

**[0:30]** Customize in the wizard:
<!-- output -->
![Script](assets/02-script.svg)

```
  CPU cores [2.0]: 2
  Memory [1GB]: 2GB
  Need GPU? [y/N]: n

  ✓ Created pod 'openclaw' (openclaw preset, 2 cores, 2GB)
```
<!-- pause 2 -->
*Narration: "Setup installs Node.js 22 and OpenClaw — all inside the overlay. Takes a minute."*

**[0:50]** Store credentials:
<!-- no-exec -->
<!-- type-delay 0.02 -->
![Script](assets/03-script.svg)

```bash
sudo envpod vault openclaw set ANTHROPIC_API_KEY
sudo envpod vault openclaw set OPENAI_API_KEY
```
*Narration: "Each key encrypted separately. The agent gets them at runtime — never sees them in config files or env dumps."*

**[1:05]** Launch:
<!-- no-exec -->
![Script](assets/04-script.svg)

```bash
sudo envpod run openclaw -- openclaw
```
*Show OpenClaw starting, connecting to messaging platforms.*

**[1:25]** Second terminal — live audit:
<!-- no-exec -->
![Script](assets/05-script.svg)

```bash
sudo envpod audit openclaw
```
<!-- pause 2 -->
*Narration: "Every API call, every message sent, every LLM invocation — logged."*

**[1:40]** Show the diff (files OpenClaw created/modified):
<!-- no-exec -->
![Script](assets/06-script.svg)

```bash
sudo envpod diff openclaw
```
<!-- pause 2 -->

**[1:55]** Show snapshots — checkpoint the state:
<!-- no-exec -->
<!-- type-delay 0.02 -->
![Script](assets/07-script.svg)

```bash
sudo envpod snapshot openclaw create -n "after-setup"
sudo envpod snapshot openclaw ls
```
<!-- pause 2 -->
*Narration: "Snapshot the overlay at any point. Restore later if something breaks."*

**[2:10]** Freeze and resume:
<!-- no-exec -->
<!-- type-delay 0.02 -->
![Script](assets/08-script.svg)

```bash
sudo envpod freeze openclaw
# ... inspect ...
sudo envpod resume openclaw
```
*Narration: "Instant freeze. Every process in the pod suspended. Inspect, then resume or kill."*

**[2:30]** End card.

---

## Commands (copy-paste)

<!-- no-exec -->
<!-- type-delay 0.02 -->
![Commands (copy-paste)](assets/09-commands-copy-paste.svg)

```bash
sudo envpod init openclaw
# (select openclaw from wizard, customize resources)
sudo envpod vault openclaw set ANTHROPIC_API_KEY
sudo envpod vault openclaw set OPENAI_API_KEY
sudo envpod run openclaw -- openclaw
sudo envpod audit openclaw
sudo envpod diff openclaw
sudo envpod snapshot openclaw create -n "after-setup"
sudo envpod snapshot openclaw ls
sudo envpod freeze openclaw
sudo envpod resume openclaw
```


<p align="center"><sub>Made with <a href="https://github.com/markamo/md2cast">md2cast</a></sub></p>
