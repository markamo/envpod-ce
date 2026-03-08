// Copyright 2026 Mark Amo-Boateng / Xtellix Inc.
// SPDX-License-Identifier: BSL-1.1

//! Desktop environment setup for pods.
//!
//! Generates apt-get commands to install a minimal desktop environment
//! based on the `devices.desktop_env` config option. Pairs with
//! `web_display` (noVNC/WebRTC) or `devices.display` (host passthrough).

use crate::config::DesktopEnv;

/// Generate setup commands to install the selected desktop environment.
/// Returns empty vec for `DesktopEnv::None`.
pub fn generate_setup_commands(env: DesktopEnv) -> Vec<String> {
    match env {
        DesktopEnv::None => Vec::new(),
        DesktopEnv::Xfce => vec![
            "cd /etc/apt/sources.list.d && for f in *.list *.sources; do case \"$f\" in ubuntu*) ;; *) rm -f \"$f\" ;; esac; done 2>/dev/null; dpkg --configure -a 2>/dev/null; apt-get update -qq".into(),
            "DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends xfce4 xfce4-terminal dbus-x11".into(),
        ],
        DesktopEnv::Openbox => vec![
            "cd /etc/apt/sources.list.d && for f in *.list *.sources; do case \"$f\" in ubuntu*) ;; *) rm -f \"$f\" ;; esac; done 2>/dev/null; dpkg --configure -a 2>/dev/null; apt-get update -qq".into(),
            "DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends openbox tint2 xterm".into(),
        ],
        DesktopEnv::Sway => vec![
            "cd /etc/apt/sources.list.d && for f in *.list *.sources; do case \"$f\" in ubuntu*) ;; *) rm -f \"$f\" ;; esac; done 2>/dev/null; dpkg --configure -a 2>/dev/null; apt-get update -qq".into(),
            "DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends sway foot".into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_returns_empty() {
        assert!(generate_setup_commands(DesktopEnv::None).is_empty());
    }

    #[test]
    fn xfce_installs_xfce4() {
        let cmds = generate_setup_commands(DesktopEnv::Xfce);
        assert_eq!(cmds.len(), 2);
        assert!(cmds[0].contains("apt-get update"));
        assert!(cmds[1].contains("xfce4"));
        assert!(cmds[1].contains("xfce4-terminal"));
        assert!(cmds[1].contains("dbus-x11"));
        assert!(cmds[1].contains("--no-install-recommends"));
    }

    #[test]
    fn openbox_installs_openbox() {
        let cmds = generate_setup_commands(DesktopEnv::Openbox);
        assert_eq!(cmds.len(), 2);
        assert!(cmds[1].contains("openbox"));
        assert!(cmds[1].contains("tint2"));
        assert!(cmds[1].contains("xterm"));
    }

    #[test]
    fn sway_installs_sway() {
        let cmds = generate_setup_commands(DesktopEnv::Sway);
        assert_eq!(cmds.len(), 2);
        assert!(cmds[1].contains("sway"));
        assert!(cmds[1].contains("foot"));
    }
}
