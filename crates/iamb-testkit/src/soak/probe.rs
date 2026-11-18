//! Process samples (`docs/test-strategy.md` §5.1): memory, CPU time, open handles or file
//! descriptors, and threads of one process, read from outside it.
//!
//! | OS | Source | Private memory |
//! | --- | --- | --- |
//! | Linux (CI) | `/proc/<pid>/{status,stat,smaps_rollup,fd}` | `Private_Clean + Private_Dirty` of `smaps_rollup` (else `RssAnon`) |
//! | macOS | `ps`, `lsof` | not read (`phys_footprint` needs the lab's `footprint` tool); the rules use RSS |
//! | Windows | PowerShell `Get-Process` | `PrivateMemorySize64` (private bytes) |
//!
//! Linux CPU time is in clock ticks of `USER_HZ`, which is 100 on every Linux the project
//! supports.

use std::process::Command;

use serde::{Deserialize, Serialize};

/// One process at one moment. `None`: not available on this OS or the process is gone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessSample {
    /// The process id.
    pub pid: u32,
    /// The process existed when sampled.
    pub alive: bool,
    /// Resident set size, KiB.
    pub rss_kb: Option<u64>,
    /// Private memory, KiB (see the module docs).
    pub private_kb: Option<u64>,
    /// User plus system CPU time, ms.
    pub cpu_ms: Option<u64>,
    /// Open file descriptors (Unix) or handles (Windows).
    pub handles: Option<u64>,
    /// Threads.
    pub threads: Option<u64>,
}

impl ProcessSample {
    /// The memory the leak rules use: private, else resident, KiB.
    pub fn memory_kb(&self) -> Option<u64> {
        self.private_kb.or(self.rss_kb)
    }
}

/// Samples `pid`.
pub fn sample(pid: u32) -> ProcessSample {
    if cfg!(target_os = "linux") {
        linux(pid)
    } else if cfg!(windows) {
        windows(pid)
    } else {
        ps(pid)
    }
}

fn kb_field(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .find_map(|l| l.strip_prefix(key))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
}

/// Parses `/proc/<pid>/stat`: user and system time in ticks.
fn stat_ticks(stat: &str) -> Option<u64> {
    // The command name may hold spaces and parentheses: the fields start after the last ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // Field 3 (state) is fields[0]; utime is field 14, stime field 15.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

fn linux(pid: u32) -> ProcessSample {
    let dir = std::path::PathBuf::from(format!("/proc/{pid}"));
    let Ok(status) = std::fs::read_to_string(dir.join("status")) else {
        return ProcessSample {
            pid,
            ..ProcessSample::default()
        };
    };
    let private = std::fs::read_to_string(dir.join("smaps_rollup"))
        .ok()
        .and_then(|s| Some(kb_field(&s, "Private_Clean:")? + kb_field(&s, "Private_Dirty:")?))
        .or_else(|| kb_field(&status, "RssAnon:"));
    let cpu_ms = std::fs::read_to_string(dir.join("stat"))
        .ok()
        .and_then(|s| stat_ticks(&s))
        .map(|ticks| ticks * 10);
    let handles = std::fs::read_dir(dir.join("fd"))
        .ok()
        .map(|d| u64::try_from(d.count()).unwrap_or(0));
    ProcessSample {
        pid,
        alive: true,
        rss_kb: kb_field(&status, "VmRSS:"),
        private_kb: private,
        cpu_ms,
        handles,
        threads: kb_field(&status, "Threads:"),
    }
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parses `ps` CPU time: `[[dd-]hh:]mm:ss[.cc]`, into ms.
fn ps_time_ms(text: &str) -> Option<u64> {
    let text = text.trim();
    let (days, rest) = match text.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, text),
    };
    let mut total_ms = days * 86_400_000;
    let parts: Vec<&str> = rest.split(':').collect();
    let (secs, units) = parts.split_last()?;
    let (whole, frac) = secs.split_once('.').unwrap_or((secs, "0"));
    total_ms += whole.parse::<u64>().ok()? * 1000;
    let frac: String = frac.chars().chain("000".chars()).take(3).collect();
    total_ms += frac.parse::<u64>().ok()?;
    for (i, unit) in units.iter().rev().enumerate() {
        let factor = if i == 0 { 60_000 } else { 3_600_000 };
        total_ms += unit.parse::<u64>().ok()? * factor;
    }
    Some(total_ms)
}

fn ps(pid: u32) -> ProcessSample {
    let id = pid.to_string();
    let Some(line) = run("ps", &["-o", "rss=,time=", "-p", &id]) else {
        return ProcessSample {
            pid,
            ..ProcessSample::default()
        };
    };
    let mut fields = line.split_whitespace();
    let rss_kb = fields.next().and_then(|n| n.parse().ok());
    let cpu_ms = fields.next().and_then(ps_time_ms);
    let threads = run("ps", &["-M", "-p", &id])
        .map(|t| u64::try_from(t.lines().count().saturating_sub(1)).unwrap_or(0));
    let handles = run("lsof", &["-n", "-P", "-p", &id])
        .map(|t| u64::try_from(t.lines().count().saturating_sub(1)).unwrap_or(0));
    ProcessSample {
        pid,
        alive: rss_kb.is_some(),
        rss_kb,
        private_kb: None,
        cpu_ms,
        handles,
        threads,
    }
}

fn windows(pid: u32) -> ProcessSample {
    let script = format!(
        "$p = Get-Process -Id {pid} -ErrorAction Stop; \"$($p.WorkingSet64) $($p.PrivateMemorySize64) $([int64]$p.TotalProcessorTime.TotalMilliseconds) $($p.HandleCount) $($p.Threads.Count)\""
    );
    let Some(line) = run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    ) else {
        return ProcessSample {
            pid,
            ..ProcessSample::default()
        };
    };
    let n: Vec<u64> = line
        .split_whitespace()
        .filter_map(|f| f.parse().ok())
        .collect();
    if n.len() != 5 {
        return ProcessSample {
            pid,
            ..ProcessSample::default()
        };
    }
    ProcessSample {
        pid,
        alive: true,
        rss_kb: Some(n[0] / 1024),
        private_kb: Some(n[1] / 1024),
        cpu_ms: Some(n[2]),
        handles: Some(n[3]),
        threads: Some(n[4]),
    }
}

/// Kills `pid` at once, without letting it clean up (`kill -9`, `taskkill /F`).
pub fn hard_kill(pid: u32) -> Result<(), String> {
    let id = pid.to_string();
    let status = if cfg!(windows) {
        Command::new("taskkill").args(["/F", "/PID", &id]).status()
    } else {
        Command::new("kill").args(["-9", &id]).status()
    };
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("kill of {pid} failed: {s}")),
        Err(e) => Err(format!("cannot run kill: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_is_parsed_after_the_command_name() {
        let stat = "4242 (iamb (soak) x) S 1 2 3 4 5 6 7 8 9 10 250 50 0 0 20 0 9 0";
        assert_eq!(stat_ticks(stat), Some(300));
        assert_eq!(stat_ticks("garbage"), None);
    }

    #[test]
    fn ps_times_parse() {
        assert_eq!(ps_time_ms("0:00.03"), Some(30));
        assert_eq!(ps_time_ms("1:02.5"), Some(62_500));
        assert_eq!(ps_time_ms("01:00:00"), Some(3_600_000));
        assert_eq!(ps_time_ms("2-00:00:01"), Some(172_801_000));
        assert_eq!(ps_time_ms("x"), None);
    }

    #[test]
    fn this_process_can_be_sampled() {
        let s = sample(std::process::id());
        if cfg!(target_os = "linux") {
            assert!(s.alive);
            assert!(s.rss_kb.is_some_and(|kb| kb > 0));
            assert!(s.private_kb.is_some());
            assert!(s.cpu_ms.is_some());
            assert!(s.handles.is_some_and(|n| n >= 3));
            assert!(s.threads.is_some_and(|n| n >= 1));
        }
        assert_eq!(s.pid, std::process::id());
    }
}
