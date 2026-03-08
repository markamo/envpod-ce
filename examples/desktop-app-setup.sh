#!/bin/bash
# desktop-app-setup.sh — Install and configure common desktop apps for envpod
#
# Creates wrapper scripts in /usr/local/bin/ that add --no-sandbox
# and desktop entries in /usr/share/applications/ for the XFCE menu.
# Needed because pods run as root — envpod's namespace isolation replaces
# each app's internal sandbox.
#
# Usage in pod.yaml:
#   setup_script: examples/desktop-app-setup.sh
#
# Or source specific functions:
#   setup:
#     - "bash /path/to/desktop-app-setup.sh chrome vscode gimp"
#
# Supported apps: chrome, firefox, vscode, gimp, libreoffice, slack,
#                 brave, blender, inkscape, obs, vlc, cursor, obsidian

set -e

install_chrome() {
    if command -v google-chrome >/dev/null 2>&1; then
        echo "Chrome already installed"
    else
        apt-get install -y wget gnupg
        wget -qO- https://dl.google.com/linux/linux_signing_key.pub | gpg --dearmor -o /usr/share/keyrings/google-chrome.gpg
        echo "deb [arch=amd64 signed-by=/usr/share/keyrings/google-chrome.gpg] http://dl.google.com/linux/chrome/deb/ stable main" > /etc/apt/sources.list.d/google-chrome.list
        apt-get update && apt-get install -y google-chrome-stable
    fi
    # Wrapper + desktop entry
    cat > /usr/local/bin/chrome << 'EOF'
#!/bin/bash
exec google-chrome --no-sandbox "$@"
EOF
    chmod +x /usr/local/bin/chrome
    cat > /usr/share/applications/chrome.desktop << 'EOF'
[Desktop Entry]
Name=Chrome
Exec=google-chrome --no-sandbox %U
Icon=google-chrome
Type=Application
Categories=Network;WebBrowser;
EOF
}

install_firefox() {
    if command -v firefox >/dev/null 2>&1 && ! readlink -f "$(which firefox)" | grep -q snap; then
        echo "Firefox (deb) already installed"
    else
        # Remove snap version if present (doesn't work in namespace pods)
        snap remove firefox 2>/dev/null || true
        # Install from Mozilla APT repo (deb, not snap)
        apt-get install -y wget gnupg
        wget -qO- https://packages.mozilla.org/apt/repo-signing-key.gpg | gpg --dearmor -o /usr/share/keyrings/packages.mozilla.gpg
        echo "deb [signed-by=/usr/share/keyrings/packages.mozilla.gpg] https://packages.mozilla.org/apt mozilla main" > /etc/apt/sources.list.d/mozilla.list
        # Prefer Mozilla repo over Ubuntu snap
        cat > /etc/apt/preferences.d/mozilla << 'PREF'
Package: firefox*
Pin: origin packages.mozilla.org
Pin-Priority: 1001
PREF
        apt-get update && apt-get install -y firefox
    fi
    # Wrapper + desktop entry
    cat > /usr/local/bin/firefox-pod << 'EOF'
#!/bin/bash
export MOZ_DISABLE_CONTENT_SANDBOX=1
export MOZ_DISABLE_GMP_SANDBOX=1
export MOZ_DISABLE_RDD_SANDBOX=1
exec firefox "$@"
EOF
    chmod +x /usr/local/bin/firefox-pod
    cat > /usr/share/applications/firefox-pod.desktop << 'EOF'
[Desktop Entry]
Name=Firefox
Exec=firefox-pod %U
Icon=firefox
Type=Application
Categories=Network;WebBrowser;
EOF
}

install_vscode() {
    if command -v code >/dev/null 2>&1; then
        echo "VS Code already installed"
    else
        apt-get install -y wget gpg apt-transport-https
        wget -qO- https://packages.microsoft.com/keys/microsoft.asc | gpg --dearmor -o /usr/share/keyrings/packages.microsoft.gpg
        echo "deb [arch=amd64 signed-by=/usr/share/keyrings/packages.microsoft.gpg] https://packages.microsoft.com/repos/code stable main" > /etc/apt/sources.list.d/vscode.list
        apt-get update && apt-get install -y code
    fi
    cat > /usr/local/bin/vscode << 'EOF'
#!/bin/bash
exec code --no-sandbox "$@"
EOF
    chmod +x /usr/local/bin/vscode
    cat > /usr/share/applications/vscode.desktop << 'EOF'
[Desktop Entry]
Name=VS Code
Exec=code --no-sandbox %F
Icon=vscode
Type=Application
Categories=Development;IDE;
EOF
}

install_gimp() {
    apt-get install -y --no-install-recommends gimp
    # GIMP works as-is (single-process GTK app, survives lock/unlock)
}

install_libreoffice() {
    apt-get install -y --no-install-recommends libreoffice
    # LibreOffice works as-is under root
}

install_slack() {
    if command -v slack >/dev/null 2>&1; then
        echo "Slack already installed"
    else
        apt-get install -y wget
        wget -qO /tmp/slack.deb "https://downloads.slack-edge.com/desktop-releases/linux/x64/4.41.105/slack-desktop-4.41.105-amd64.deb"
        apt-get install -y /tmp/slack.deb
        rm -f /tmp/slack.deb
    fi
    cat > /usr/local/bin/slack-pod << 'EOF'
#!/bin/bash
exec slack --no-sandbox "$@"
EOF
    chmod +x /usr/local/bin/slack-pod
    cat > /usr/share/applications/slack-pod.desktop << 'EOF'
[Desktop Entry]
Name=Slack
Exec=slack --no-sandbox %U
Icon=slack
Type=Application
Categories=Network;Chat;
EOF
}

install_brave() {
    if command -v brave-browser >/dev/null 2>&1; then
        echo "Brave already installed"
    else
        apt-get install -y curl gnupg
        curl -fsSLo /usr/share/keyrings/brave-browser-archive-keyring.gpg https://brave-browser-apt-release.s3.brave.com/brave-browser-archive-keyring.gpg
        echo "deb [signed-by=/usr/share/keyrings/brave-browser-archive-keyring.gpg] https://brave-browser-apt-release.s3.brave.com/ stable main" > /etc/apt/sources.list.d/brave-browser-release.list
        apt-get update && apt-get install -y brave-browser
    fi
    cat > /usr/local/bin/brave << 'EOF'
#!/bin/bash
exec brave-browser --no-sandbox "$@"
EOF
    chmod +x /usr/local/bin/brave
    cat > /usr/share/applications/brave-pod.desktop << 'EOF'
[Desktop Entry]
Name=Brave
Exec=brave-browser --no-sandbox %U
Icon=brave-browser
Type=Application
Categories=Network;WebBrowser;
EOF
}

install_blender() {
    apt-get install -y --no-install-recommends blender
    # Blender works as-is (uses GPU via passthrough if devices.gpu: true)
}

install_inkscape() {
    apt-get install -y --no-install-recommends inkscape
}

install_obs() {
    apt-get install -y --no-install-recommends obs-studio
}

install_vlc() {
    apt-get install -y --no-install-recommends vlc
    # VLC refuses to run as root by default
    cat > /usr/local/bin/vlc-pod << 'EOF'
#!/bin/bash
exec /usr/bin/vlc --no-qt-privacy-ask "$@"
EOF
    chmod +x /usr/local/bin/vlc-pod
}

install_cursor() {
    if command -v cursor >/dev/null 2>&1; then
        echo "Cursor already installed"
    else
        apt-get install -y wget
        wget -qO /tmp/cursor.appimage "https://downloader.cursor.sh/linux/appImage/x64"
        chmod +x /tmp/cursor.appimage
        /tmp/cursor.appimage --appimage-extract
        mv squashfs-root /opt/cursor
        ln -sf /opt/cursor/cursor /usr/local/bin/cursor-bin
        rm -f /tmp/cursor.appimage
    fi
    cat > /usr/local/bin/cursor << 'EOF'
#!/bin/bash
exec /opt/cursor/cursor --no-sandbox "$@"
EOF
    chmod +x /usr/local/bin/cursor
    cat > /usr/share/applications/cursor.desktop << 'EOF'
[Desktop Entry]
Name=Cursor
Exec=/opt/cursor/cursor --no-sandbox %F
Icon=cursor
Type=Application
Categories=Development;IDE;
EOF
}

install_obsidian() {
    if command -v obsidian >/dev/null 2>&1; then
        echo "Obsidian already installed"
    else
        apt-get install -y wget
        wget -qO /tmp/obsidian.deb "https://github.com/obsidianmd/obsidian-releases/releases/download/v1.7.7/obsidian_1.7.7_amd64.deb"
        apt-get install -y /tmp/obsidian.deb
        rm -f /tmp/obsidian.deb
    fi
    cat > /usr/local/bin/obsidian-pod << 'EOF'
#!/bin/bash
exec obsidian --no-sandbox "$@"
EOF
    chmod +x /usr/local/bin/obsidian-pod
    cat > /usr/share/applications/obsidian-pod.desktop << 'EOF'
[Desktop Entry]
Name=Obsidian
Exec=obsidian --no-sandbox %U
Icon=obsidian
Type=Application
Categories=Office;
EOF
}

# Ensure desktop entries directory exists
mkdir -p /usr/share/applications

# If called with arguments, install those apps
# If called with no arguments, print usage and exit
if [ $# -gt 0 ]; then
    for app in "$@"; do
        echo "==> Installing $app..."
        install_"$app"
    done
else
    echo "Usage: $0 <app1> [app2] ..."
    echo "Available: chrome firefox vscode gimp libreoffice slack brave blender inkscape obs vlc cursor obsidian"
    exit 0
fi
