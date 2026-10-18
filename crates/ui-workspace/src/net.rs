//! Network quiet-start check (ADR-0029, spec "Quiet start"): count this process's internet sockets.
//!
//! All outbound HTTP lives in `ps-llm` and is constructed lazily (AI queue start, "Test connection"); the
//! shell has no network code at all. This module lets tests and `--measure-idle-memory` runs verify that
//! no TCP/UDP socket exists after start-up and idling. Local IPC (Unix sockets: Wayland, D-Bus) is not
//! counted.

/// The process's open IPv4/IPv6 TCP and UDP sockets, or `None` where the platform does not tell us.
pub fn inet_sockets(pid: Option<u32>) -> Option<Vec<String>> {
    #[cfg(target_os = "linux")]
    {
        let proc = match pid {
            Some(pid) => format!("/proc/{pid}"),
            None => "/proc/self".to_owned(),
        };
        // Inodes of every socket this process holds.
        let mut inodes = std::collections::HashSet::new();
        for entry in std::fs::read_dir(format!("{proc}/fd")).ok()?.flatten() {
            if let Ok(target) = std::fs::read_link(entry.path())
                && let Some(inode) = target
                    .to_str()
                    .and_then(|t| t.strip_prefix("socket:["))
                    .and_then(|t| t.strip_suffix(']'))
            {
                inodes.insert(inode.to_owned());
            }
        }
        let mut found = Vec::new();
        for table in ["tcp", "tcp6", "udp", "udp6"] {
            let Ok(text) = std::fs::read_to_string(format!("{proc}/net/{table}")) else {
                continue;
            };
            for line in text.lines().skip(1) {
                let cols: Vec<&str> = line.split_whitespace().collect();
                // sl local rem st tx:rx tr:when retrnsmt uid timeout inode
                if let Some(inode) = cols.get(9)
                    && inodes.contains(*inode)
                {
                    found.push(format!(
                        "{table} {} -> {}",
                        cols.get(1).unwrap_or(&"?"),
                        cols.get(2).unwrap_or(&"?")
                    ));
                }
            }
        }
        Some(found)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use ::core::prelude::v1::test;

    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn sees_a_socket_this_process_opens() {
        let before = inet_sockets(None).unwrap().len();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let during = inet_sockets(None).unwrap().len();
        drop(listener);
        assert!(during > before, "{before} -> {during}");
    }
}
