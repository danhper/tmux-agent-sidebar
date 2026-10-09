use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::process::Command;

use crate::tmux::AgentType;

#[derive(Debug, Clone)]
pub(crate) struct ProcessInfo {
    pub(crate) comm: String,
    pub(crate) args: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcessSnapshot {
    pub(crate) children_of: HashMap<u32, Vec<u32>>,
    pub(crate) info_by_pid: HashMap<u32, ProcessInfo>,
}

impl ProcessSnapshot {
    pub(crate) fn scan() -> Option<Self> {
        let output = Command::new("ps")
            .args(["-eo", "pid=,ppid=,comm=,args="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(Self::from_ps_output(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    pub(crate) fn from_ps_output(ps_output: &str) -> Self {
        let mut children_of: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut info_by_pid: HashMap<u32, ProcessInfo> = HashMap::new();

        for line in ps_output.lines() {
            let mut parts = line.split_whitespace();
            let Some(pid_str) = parts.next() else {
                continue;
            };
            let Some(ppid_str) = parts.next() else {
                continue;
            };
            let Ok(pid) = pid_str.parse::<u32>() else {
                continue;
            };
            let Ok(ppid) = ppid_str.parse::<u32>() else {
                continue;
            };
            let Some(comm) = parts.next() else {
                continue;
            };

            children_of.entry(ppid).or_default().push(pid);
            info_by_pid.insert(
                pid,
                ProcessInfo {
                    comm: comm.to_string(),
                    args: parts.collect::<Vec<_>>().join(" "),
                },
            );
        }

        Self {
            children_of,
            info_by_pid,
        }
    }

    pub(crate) fn descendants(&self, seed_pids: &[u32]) -> HashSet<u32> {
        let mut seen = HashSet::new();
        let mut queue: VecDeque<u32> = seed_pids.iter().copied().collect();

        while let Some(pid) = queue.pop_front() {
            if !seen.insert(pid) {
                continue;
            }
            if let Some(children) = self.children_of.get(&pid) {
                for &child in children {
                    if !seen.contains(&child) {
                        queue.push_back(child);
                    }
                }
            }
        }

        seen
    }

    pub(crate) fn tree_has_agent(&self, seed_pids: &[u32], agent: &AgentType) -> bool {
        self.descendants(seed_pids).into_iter().any(|pid| {
            self.info_by_pid
                .get(&pid)
                .map(|info| process_is_agent(info, agent))
                .unwrap_or(false)
        })
    }

    pub(crate) fn tree_has_claude_remote_control(&self, seed_pids: &[u32]) -> bool {
        let mut queue: VecDeque<u32> = seed_pids.iter().copied().collect();
        let mut seen = HashSet::new();
        while let Some(pid) = queue.pop_front() {
            if !seen.insert(pid) {
                continue;
            }
            if let Some(info) = self.info_by_pid.get(&pid)
                && process_is_agent(info, &AgentType::Claude)
            {
                let mut args = info.args.split_whitespace();
                args.next();
                if matches!(args.next(), Some("rc" | "remote-control")) {
                    return true;
                }
                // A real interactive Claude client owns this branch. Its
                // tool-spawned children do not make the pane an RC launcher.
                continue;
            }
            if let Some(children) = self.children_of.get(&pid) {
                queue.extend(children.iter().copied());
            }
        }
        false
    }

    pub(crate) fn command_lines_for_tree(&self, seed_pids: &[u32]) -> Vec<String> {
        self.descendants(seed_pids)
            .into_iter()
            .filter_map(|pid| self.info_by_pid.get(&pid))
            .map(|info| {
                if info.args.is_empty() {
                    info.comm.clone()
                } else {
                    info.args.trim().to_string()
                }
            })
            .collect()
    }
}

pub(crate) fn command_basename(command: &str) -> &str {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command)
}

/// Whether `info` is a live process of `agent`, checking every executable
/// name the agent is known to run under (see [`AgentType::process_names`]).
pub(crate) fn process_is_agent(info: &ProcessInfo, agent: &AgentType) -> bool {
    agent
        .process_names()
        .iter()
        .any(|name| process_matches_agent(info, name))
}

pub(crate) fn process_matches_agent(info: &ProcessInfo, agent_name: &str) -> bool {
    if command_basename(&info.comm) == agent_name {
        return true;
    }

    let Some(command) = info.args.split_whitespace().next() else {
        return false;
    };
    command_basename(command.trim_matches('"')) == agent_name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_control_launcher_is_detected_through_shell_wrappers() {
        for subcommand in ["rc", "remote-control"] {
            let snapshot = ProcessSnapshot::from_ps_output(&format!(
                "100 1 fish fish\n101 100 claude /opt/homebrew/bin/claude {subcommand}\n102 101 claude claude --print --sdk-url https://example.test\n"
            ));
            assert!(snapshot.tree_has_claude_remote_control(&[100]));
            assert!(!snapshot.tree_has_claude_remote_control(&[102]));
        }
        let normal = ProcessSnapshot::from_ps_output(
            "100 1 fish fish\n101 100 claude /opt/homebrew/bin/claude --resume thread\n102 101 claude claude rc\n",
        );
        assert!(!normal.tree_has_claude_remote_control(&[100]));
    }

    #[test]
    fn descendants_walks_process_tree() {
        let snapshot = ProcessSnapshot {
            children_of: HashMap::from([(1, vec![2, 3]), (2, vec![4]), (4, vec![5])]),
            info_by_pid: HashMap::new(),
        };
        let seen = snapshot.descendants(&[1]);
        assert!(seen.contains(&1));
        assert!(seen.contains(&2));
        assert!(seen.contains(&3));
        assert!(seen.contains(&4));
        assert!(seen.contains(&5));
    }

    #[test]
    fn parse_ps_processes_preserves_spaced_args() {
        let snapshot = ProcessSnapshot::from_ps_output(
            "100 1 codex /Applications/Codex App/bin/codex --full-auto\n101 100 sh sh -c wrapper\n",
        );

        assert_eq!(snapshot.children_of.get(&1).cloned(), Some(vec![100]));
        let info = snapshot.info_by_pid.get(&100).expect("process info");
        assert_eq!(info.comm, "codex");
        assert_eq!(info.args, "/Applications/Codex App/bin/codex --full-auto");
    }

    #[test]
    fn tree_has_agent_matches_descendant_process_name() {
        let snapshot = ProcessSnapshot::from_ps_output(
            "100 1 fish fish -c opencode\n101 100 opencode opencode\n",
        );

        assert!(snapshot.tree_has_agent(&[100], &AgentType::OpenCode));
        assert!(!snapshot.tree_has_agent(&[100], &AgentType::Codex));
    }

    #[test]
    fn tree_has_agent_matches_cursor_cli_by_argv0_not_comm() {
        // Real `ps` line from cursor-agent 2026.08.25: the wrapper script
        // re-execs the bundled Node binary, so `comm` is the thread name
        // while argv[0] keeps the `cursor-agent` name.
        let snapshot = ProcessSnapshot::from_ps_output(
            "100 1 fish fish\n101 100 MainThread /bin/cursor-agent --use-system-ca /opt/cursor-agent/index.js --trust\n",
        );
        assert!(snapshot.tree_has_agent(&[100], &AgentType::Cursor));
        assert!(!snapshot.tree_has_agent(&[100], &AgentType::Claude));

        // Current installer names the binary plain `agent`.
        let snapshot = ProcessSnapshot::from_ps_output(
            "100 1 zsh zsh\n101 100 MainThread /home/u/.local/bin/agent --use-system-ca /x/index.js\n",
        );
        assert!(snapshot.tree_has_agent(&[100], &AgentType::Cursor));
    }

    #[test]
    fn process_matches_agent_requires_command_name_match() {
        assert!(process_matches_agent(
            &ProcessInfo {
                comm: "claude".to_string(),
                args: "/opt/homebrew/bin/claude --flag".to_string(),
            },
            "claude",
        ));
        assert!(process_matches_agent(
            &ProcessInfo {
                comm: "node".to_string(),
                args: "/usr/local/bin/opencode".to_string(),
            },
            "opencode",
        ));
        assert!(!process_matches_agent(
            &ProcessInfo {
                comm: "not-opencode".to_string(),
                args: "/usr/local/bin/not-opencode".to_string(),
            },
            "opencode",
        ));
    }
}
