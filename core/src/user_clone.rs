use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::HostUserConfig;

const POD_UID: u32 = 60000;
const POD_GID: u32 = 60000;

pub struct ClonedUser {
    pub username: String,
    pub uid: u32,
    pub gid: u32,
    pub shell: PathBuf,
    pub home: PathBuf,
    pub host_home: PathBuf,
}

const DEFAULT_DOTFILES: &[&str] = &[
    ".bashrc",
    ".bash_profile",
    ".profile",
    ".zshrc",
    ".vimrc",
    ".gitconfig",
    ".tmux.conf",
    ".inputrc",
];

const DEFAULT_CONFIG_DIRS: &[&str] = &[
    "nvim",
];

const DEFAULT_EXCLUDES: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".config/gcloud",
    ".config/google-chrome",
    ".mozilla",
    ".password-store",
    ".kube",
    ".docker",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".gem/credentials",
];

const DEFAULT_DIRS: &[&str] = &[
    "Documents",
    "Desktop",
    "Downloads",
    "Pictures",
    "Videos",
    "Music",
    "Projects",
    "src",
    "workspace",
];

pub fn get_host_user() -> Result<ClonedUser> {
    let username = std::env::var("SUDO_USER")
        .unwrap_or_else(|_| std::env::var("USER").unwrap_or_else(|_| "agent".into()));

    let (shell, host_home) = parse_passwd_for_user(&username)?;

    Ok(ClonedUser {
        username,
        uid: POD_UID,
        gid: POD_GID,
        shell,
        home: host_home.clone(),
        host_home,
    })
}

fn parse_passwd_for_user(username: &str) -> Result<(PathBuf, PathBuf)> {
    let passwd = fs::read_to_string("/etc/passwd").context("read /etc/passwd")?;
    for line in passwd.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 7 && fields[0] == username {
            let shell = PathBuf::from(fields[6]);
            let home = PathBuf::from(fields[5]);
            return Ok((shell, home));
        }
    }
    anyhow::bail!("user '{}' not found in /etc/passwd", username)
}

pub fn setup_cloned_user(rootfs: &Path, config: &HostUserConfig) -> Result<ClonedUser> {
    let user = get_host_user()?;

    let passwd_path = rootfs.join("etc/passwd");
    let group_path = rootfs.join("etc/group");

    if passwd_path.exists() {
        let passwd = fs::read_to_string(&passwd_path).context("read rootfs passwd")?;
        let filtered: Vec<&str> = passwd
            .lines()
            .filter(|l| !l.starts_with("agent:") && !l.starts_with(&format!("{}:", user.username)))
            .collect();
        let mut new_passwd = filtered.join("\n");
        if !new_passwd.is_empty() {
            new_passwd.push('\n');
        }
        new_passwd.push_str(&format!(
            "{}:x:{}:{}:{}:{}:{}\n",
            user.username,
            POD_UID,
            POD_GID,
            user.username,
            user.home.display(),
            user.shell.display(),
        ));
        fs::write(&passwd_path, new_passwd).context("write rootfs passwd")?;
    }

    if group_path.exists() {
        let group = fs::read_to_string(&group_path).context("read rootfs group")?;
        let filtered: Vec<&str> = group
            .lines()
            .filter(|l| !l.starts_with("agent:") && !l.starts_with(&format!("{}:", user.username)))
            .collect();
        let mut new_group = filtered.join("\n");
        if !new_group.is_empty() {
            new_group.push('\n');
        }
        new_group.push_str(&format!("{}:x:{}:\n", user.username, POD_GID));
        fs::write(&group_path, new_group).context("write rootfs group")?;
    }

    let home_dir = rootfs.join(user.home.strip_prefix("/").unwrap_or(&user.home));
    fs::create_dir_all(&home_dir).context("create cloned user home")?;

    let excludes = effective_excludes(config);

    for dotfile in DEFAULT_DOTFILES {
        if is_excluded(dotfile, &excludes) {
            continue;
        }
        let src = user.host_home.join(dotfile);
        if src.exists() && src.is_file() {
            let dest = home_dir.join(dotfile);
            fs::copy(&src, &dest).ok();
        }
    }

    for extra in &config.include_dotfiles {
        if is_excluded(extra, &excludes) {
            continue;
        }
        let src = user.host_home.join(extra);
        if src.exists() {
            let dest = home_dir.join(extra);
            if src.is_file() {
                fs::copy(&src, &dest).ok();
            } else if src.is_dir() {
                copy_dir_recursive(&src, &dest).ok();
            }
        }
    }

    let config_src = user.host_home.join(".config");
    if config_src.is_dir() {
        let config_dest = home_dir.join(".config");
        fs::create_dir_all(&config_dest).ok();
        for dir_name in DEFAULT_CONFIG_DIRS {
            let full_path = format!(".config/{}", dir_name);
            if is_excluded(&full_path, &excludes) {
                continue;
            }
            let src = config_src.join(dir_name);
            if src.is_dir() {
                let dest = config_dest.join(dir_name);
                copy_dir_recursive(&src, &dest).ok();
            }
        }
    }

    chown_recursive(&home_dir, POD_UID, POD_GID);

    Ok(user)
}

pub fn user_dir_mounts(
    user: &ClonedUser,
    config: &HostUserConfig,
) -> Vec<(PathBuf, PathBuf, bool)> {
    let dirs: Vec<&str> = if config.dirs.is_empty() {
        DEFAULT_DIRS.iter().copied().collect()
    } else {
        config.dirs.iter().map(|s| s.as_str()).collect()
    };

    let excludes = effective_excludes(config);
    let mut mounts = Vec::new();

    for dir in dirs {
        if is_excluded(dir, &excludes) {
            continue;
        }
        let host_path = user.host_home.join(dir);
        if host_path.exists() && host_path.is_dir() {
            let pod_path = user.home.join(dir);
            mounts.push((host_path, pod_path, true));
        }
    }

    mounts
}

fn effective_excludes(config: &HostUserConfig) -> Vec<String> {
    if config.exclude.is_empty() {
        DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect()
    } else {
        config.exclude.clone()
    }
}

fn is_excluded(path: &str, excludes: &[String]) -> bool {
    let path_normalized = path.trim_start_matches('/');
    for exc in excludes {
        let exc_normalized = exc.trim_start_matches('/');
        if path_normalized == exc_normalized || path_normalized.starts_with(&format!("{}/", exc_normalized)) {
            return true;
        }
    }
    false
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            fs::copy(&src_path, &dest_path)?;
        }
    }
    Ok(())
}

fn chown_recursive(path: &Path, uid: u32, gid: u32) {
    let nuid = Some(nix::unistd::Uid::from_raw(uid));
    let ngid = Some(nix::unistd::Gid::from_raw(gid));
    let _ = nix::unistd::chown(path, nuid, ngid);
    if path.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                chown_recursive(&entry.path(), uid, gid);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_passwd_line() {
        let result = parse_passwd_for_user("root");
        assert!(result.is_ok());
        let (shell, home) = result.unwrap();
        assert_eq!(home, PathBuf::from("/root"));
        assert!(!shell.as_os_str().is_empty());
    }

    #[test]
    fn parse_passwd_nonexistent_user() {
        let result = parse_passwd_for_user("envpod_nonexistent_user_12345");
        assert!(result.is_err());
    }

    #[test]
    fn default_excludes_applied() {
        let config = HostUserConfig::default();
        let excludes = effective_excludes(&config);
        assert!(excludes.contains(&".ssh".to_string()));
        assert!(excludes.contains(&".gnupg".to_string()));
        assert!(excludes.contains(&".aws".to_string()));
    }

    #[test]
    fn custom_excludes_override_defaults() {
        let config = HostUserConfig {
            exclude: vec![".mydir".into()],
            ..Default::default()
        };
        let excludes = effective_excludes(&config);
        assert_eq!(excludes, vec![".mydir"]);
        assert!(!excludes.contains(&".ssh".to_string()));
    }

    #[test]
    fn exclude_filtering() {
        let excludes = vec![
            ".ssh".to_string(),
            ".config/gcloud".to_string(),
        ];
        assert!(is_excluded(".ssh", &excludes));
        assert!(is_excluded(".ssh/id_rsa", &excludes));
        assert!(is_excluded(".config/gcloud", &excludes));
        assert!(is_excluded(".config/gcloud/credentials", &excludes));
        assert!(!is_excluded(".bashrc", &excludes));
        assert!(!is_excluded(".config/nvim", &excludes));
    }

    #[test]
    fn exclude_filtering_with_leading_slash() {
        let excludes = vec![".ssh".to_string()];
        assert!(is_excluded("/.ssh", &excludes));
        assert!(is_excluded(".ssh", &excludes));
    }

    #[test]
    fn dotfile_list_generation() {
        let config = HostUserConfig {
            clone_host: true,
            exclude: vec![".bashrc".into()],
            ..Default::default()
        };
        let excludes = effective_excludes(&config);
        let dotfiles: Vec<&&str> = DEFAULT_DOTFILES
            .iter()
            .filter(|d| !is_excluded(d, &excludes))
            .collect();
        assert!(!dotfiles.contains(&&".bashrc"));
        assert!(dotfiles.contains(&&".profile"));
        assert!(dotfiles.contains(&&".gitconfig"));
    }

    #[test]
    fn user_dir_mounts_uses_defaults_when_empty() {
        let user = ClonedUser {
            username: "testuser".into(),
            uid: POD_UID,
            gid: POD_GID,
            shell: PathBuf::from("/bin/bash"),
            home: PathBuf::from("/home/testuser"),
            host_home: PathBuf::from("/home/testuser"),
        };
        let config = HostUserConfig::default();
        let mounts = user_dir_mounts(&user, &config);
        for (host, pod, ro) in &mounts {
            assert!(host.starts_with("/home/testuser"));
            assert!(pod.starts_with("/home/testuser"));
            assert!(*ro);
        }
    }

    #[test]
    fn user_dir_mounts_uses_custom_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let host_home = tmp.path();
        fs::create_dir_all(host_home.join("MyDocs")).unwrap();

        let user = ClonedUser {
            username: "testuser".into(),
            uid: POD_UID,
            gid: POD_GID,
            shell: PathBuf::from("/bin/bash"),
            home: PathBuf::from("/home/testuser"),
            host_home: host_home.to_path_buf(),
        };
        let config = HostUserConfig {
            clone_host: true,
            dirs: vec!["MyDocs".into()],
            ..Default::default()
        };
        let mounts = user_dir_mounts(&user, &config);
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].0, host_home.join("MyDocs"));
        assert_eq!(mounts[0].1, PathBuf::from("/home/testuser/MyDocs"));
        assert!(mounts[0].2);
    }
}
