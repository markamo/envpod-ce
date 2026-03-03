/// Interactive init wizard — categorized preset selection + resource customization.
///
/// Only runs when stdin is a terminal. Returns a parsed `PodConfig` (or None
/// for the "custom" option which falls through to default config).

use std::io::{self, BufRead, Write};

use anyhow::{Context, Result};
use envpod_core::config::PodConfig;

use crate::presets;

/// Run the interactive wizard. Returns `Some(config)` for a preset selection,
/// or `None` if the user picks "custom" (blank config).
pub fn run_interactive() -> Result<Option<PodConfig>> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();

    // ── Print categorized preset list ──
    eprintln!();
    eprintln!("  Select a preset (or 'custom' for blank config):");
    eprintln!();

    let categories = presets::categories();
    let all_presets = presets::list();
    let total = all_presets.len();
    let mut n = 0usize;

    for (category, items) in &categories {
        eprint!("  \x1b[1m {category}\x1b[0m\n");
        for preset in items {
            n += 1;
            eprintln!("  {n:>3}  {:<15} {}", preset.name, preset.description);
        }
        eprintln!();
    }

    let custom_n = total + 1;
    eprintln!("  {custom_n:>3}  {:<15} {}", "custom", "Start from blank config");
    eprintln!();

    // ── Read selection ──
    let choice = loop {
        eprint!("  > ");
        io::stderr().flush()?;

        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            // EOF
            anyhow::bail!("no selection (stdin closed)");
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Accept number or preset name
        if let Ok(num) = trimmed.parse::<usize>() {
            if num == custom_n {
                return Ok(None);
            }
            if num >= 1 && num <= total {
                break num - 1; // index into all_presets
            }
            eprintln!("  Invalid selection. Enter 1-{custom_n}.");
        } else if trimmed.eq_ignore_ascii_case("custom") {
            return Ok(None);
        } else if let Some(idx) = all_presets.iter().position(|p| p.name.eq_ignore_ascii_case(trimmed)) {
            break idx;
        } else {
            eprintln!("  Unknown preset '{trimmed}'. Enter a number or preset name.");
        }
    };

    let preset = &all_presets[choice];
    let mut config: PodConfig = serde_yaml::from_str(preset.yaml)
        .with_context(|| format!("parse preset '{}'", preset.name))?;

    eprintln!();
    eprintln!("  Customize resources:");

    // ── CPU cores ──
    let cores_display = config.processor.cores.map_or("2.0".to_string(), |c| format!("{c}"));
    eprint!("    CPU cores [{cores_display}]: ");
    io::stderr().flush()?;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let trimmed = line.trim();
    if !trimmed.is_empty() {
        if let Ok(cores) = trimmed.parse::<f64>() {
            if cores > 0.0 {
                config.processor.cores = Some(cores);
            }
        }
    }

    // ── Memory ──
    let mem_display = config.processor.memory.as_deref().unwrap_or("2GB");
    eprint!("    Memory [{mem_display}]: ");
    io::stderr().flush()?;
    line.clear();
    reader.read_line(&mut line)?;
    let trimmed = line.trim();
    if !trimmed.is_empty() {
        config.processor.memory = Some(trimmed.to_string());
    }

    // ── GPU ──
    let gpu_default = config.devices.gpu;
    let gpu_hint = if gpu_default { "Y/n" } else { "y/N" };
    eprint!("    Need GPU? [{gpu_hint}]: ");
    io::stderr().flush()?;
    line.clear();
    reader.read_line(&mut line)?;
    let trimmed = line.trim().to_lowercase();
    let want_gpu = if trimmed.is_empty() {
        gpu_default
    } else {
        trimmed.starts_with('y')
    };
    config.devices.gpu = want_gpu;

    eprintln!();
    let final_cores = config.processor.cores.map_or("2.0".to_string(), |c| format!("{c}"));
    let final_mem = config.processor.memory.as_deref().unwrap_or("2GB");
    eprintln!(
        "  \x1b[32m\u{2713}\x1b[0m Selected preset '{}' ({} cores, {}{})",
        preset.name,
        final_cores,
        final_mem,
        if want_gpu { ", GPU" } else { "" },
    );
    eprintln!();

    Ok(Some(config))
}
