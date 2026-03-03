# Recording Tips

## Terminal Setup
- **Font**: 16pt+ JetBrains Mono or similar monospace
- **Theme**: Dark (match the envpod.dev website aesthetic)
- **Window size**: 1920x1080, terminal filling most of the screen
- **Shell prompt**: Keep it short — `$ ` or `user@host:~$ `

## Recording
- **Typing speed**: Moderate. Don't rush commands. Paste from a script if needed.
- **Narration**: Record voiceover separately for cleaner audio (or use captions)
- **Cuts**: Cut wait times (setup, npm install, etc.) with a "fast forward" visual
- **Pause**: Leave 1-2s after each command output before moving to the next

## Key Features to Highlight

Each demo should show at least one of these differentiators:
- **Presets**: `envpod presets` (18 built-in) and `--preset` flag
- **Interactive wizard**: `sudo envpod init <name>` with no flags — shows categorized picker
- **Diff/commit/rollback**: The core governance loop
- **Cloning**: `envpod clone` — 130ms from base, 10x faster than init
- **Snapshots**: `envpod snapshot create/restore` — overlay checkpoints
- **Dashboard**: `envpod dashboard` — browser-based fleet management
- **Security audit**: `envpod audit --security` — static config analysis
- **ARM64**: Mention cross-platform — same binary on x86 and Raspberry Pi / Jetson

## End Card
- `envpod.dev`
- `github.com/markamo/envpod-ce`
- "Free. Source Available. BSL 1.1."

## Thumbnail Ideas
- **Demo 1**: Terminal with `envpod presets` output + "60 seconds" overlay
- **Demo 2**: Split screen — Claude Code output left, `envpod audit` right
- **Demo 3**: Interactive wizard picking OpenClaw + messaging platform icons
- **Demo 4**: Chrome window floating inside a glowing green pod outline
- **Demo 5**: Dashboard fleet view screenshot with pods in different states

## Suggested Recording Order
1. Demo 1 (60s teaser) — quickest to record, good warm-up
2. Demo 4 (Chrome + Wayland) — most visual, grab attention
3. Demo 2 (Claude Code) — main audience, longest
4. Demo 3 (OpenClaw) — shows interactive wizard
5. Demo 5 (Dashboard) — requires all pods set up, do last
