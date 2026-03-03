/// Built-in pod presets — curated configs for common agents and environments.
///
/// Each preset maps to a YAML file in `examples/` embedded at compile time.
/// Grouped by category for the interactive wizard.

pub struct Preset {
    pub name: &'static str,
    pub description: &'static str,
    pub category: &'static str,
    pub yaml: &'static str,
}

const PRESETS: &[Preset] = &[
    // ── Coding Agents ──
    Preset {
        name: "claude-code",
        description: "Anthropic Claude Code CLI",
        category: "Coding Agents",
        yaml: include_str!("../../examples/claude-code.yaml"),
    },
    Preset {
        name: "codex",
        description: "OpenAI Codex CLI",
        category: "Coding Agents",
        yaml: include_str!("../../examples/codex.yaml"),
    },
    Preset {
        name: "gemini-cli",
        description: "Google Gemini CLI",
        category: "Coding Agents",
        yaml: include_str!("../../examples/gemini-cli.yaml"),
    },
    Preset {
        name: "opencode",
        description: "OpenCode terminal agent",
        category: "Coding Agents",
        yaml: include_str!("../../examples/opencode.yaml"),
    },
    Preset {
        name: "aider",
        description: "Aider AI pair programmer",
        category: "Coding Agents",
        yaml: include_str!("../../examples/aider.yaml"),
    },
    Preset {
        name: "swe-agent",
        description: "SWE-agent autonomous coder",
        category: "Coding Agents",
        yaml: include_str!("../../examples/swe-agent.yaml"),
    },
    // ── Frameworks ──
    Preset {
        name: "langgraph",
        description: "LangGraph workflows",
        category: "Frameworks",
        yaml: include_str!("../../examples/langgraph.yaml"),
    },
    Preset {
        name: "google-adk",
        description: "Google Agent Development Kit",
        category: "Frameworks",
        yaml: include_str!("../../examples/google-adk.yaml"),
    },
    Preset {
        name: "openclaw",
        description: "OpenClaw messaging assistant",
        category: "Frameworks",
        yaml: include_str!("../../examples/openclaw.yaml"),
    },
    // ── Browser Agents ──
    Preset {
        name: "browser-use",
        description: "Browser-use web automation",
        category: "Browser Agents",
        yaml: include_str!("../../examples/browser-use.yaml"),
    },
    Preset {
        name: "playwright",
        description: "Playwright browser automation",
        category: "Browser Agents",
        yaml: include_str!("../../examples/playwright.yaml"),
    },
    Preset {
        name: "browser",
        description: "Headless Chrome sandbox",
        category: "Browser Agents",
        yaml: include_str!("../../examples/browser.yaml"),
    },
    // ── Environments ──
    Preset {
        name: "devbox",
        description: "General dev sandbox",
        category: "Environments",
        yaml: include_str!("../../examples/devbox.yaml"),
    },
    Preset {
        name: "python-env",
        description: "Python environment",
        category: "Environments",
        yaml: include_str!("../../examples/python-env.yaml"),
    },
    Preset {
        name: "nodejs",
        description: "Node.js environment",
        category: "Environments",
        yaml: include_str!("../../examples/nodejs.yaml"),
    },
    Preset {
        name: "web-display",
        description: "noVNC desktop",
        category: "Environments",
        yaml: include_str!("../../examples/web-display-novnc.yaml"),
    },
    Preset {
        name: "desktop",
        description: "XFCE desktop via noVNC",
        category: "Environments",
        yaml: include_str!("../../examples/desktop.yaml"),
    },
    Preset {
        name: "vscode",
        description: "VS Code in the browser",
        category: "Environments",
        yaml: include_str!("../../examples/vscode.yaml"),
    },
];

/// Look up a preset by name (case-insensitive).
pub fn get(name: &str) -> Option<&'static Preset> {
    let lower = name.to_lowercase();
    PRESETS.iter().find(|p| p.name == lower)
}

/// Return all presets in display order.
pub fn list() -> &'static [Preset] {
    PRESETS
}

/// Category order for display.
const CATEGORY_ORDER: &[&str] = &[
    "Coding Agents",
    "Frameworks",
    "Browser Agents",
    "Environments",
];

/// Return presets grouped by category in display order.
pub fn categories() -> Vec<(&'static str, Vec<&'static Preset>)> {
    CATEGORY_ORDER
        .iter()
        .map(|&cat| {
            let items: Vec<&Preset> = PRESETS.iter().filter(|p| p.category == cat).collect();
            (cat, items)
        })
        .filter(|(_, items)| !items.is_empty())
        .collect()
}

/// Format a table of all presets for `envpod presets`.
pub fn format_table() -> String {
    let mut out = String::new();
    let mut n = 0usize;
    for (category, presets) in categories() {
        if n > 0 {
            out.push('\n');
        }
        out.push_str(&format!("  \x1b[1m{category}\x1b[0m\n"));
        for preset in presets {
            n += 1;
            out.push_str(&format!(
                "  {n:>3}  {:<15} {}\n",
                preset.name, preset.description
            ));
        }
    }
    out
}
