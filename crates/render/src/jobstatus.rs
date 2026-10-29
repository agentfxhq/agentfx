//! Optional job status files, e.g. for a render dashboard that lists running jobs.
//!
//! Off by default. With `--jobs-dir DIR` (or `AGENTFX_JOBS_DIR=DIR`), `agentfx-render render|run`
//! writes `DIR/<host>-<pid>-<start>.json` (atomic tmp + rename, at most once per second
//! from a background thread): progress, ETA from the fps of the last ~10 s, output
//! folder, caller, peak RSS, and at the end the exit status / error. Jobs shorter than
//! ~1 s or with fewer than 2 frames (one-frame IRs) leave no file. Caller: `$AGENTFX_CALLER`,
//! else derived from the parent process chain (an agent CLI, an HTTP wrapper, cron, ssh
//! or manual) plus the git worktree name. Files older than 7 days in `DIR` are pruned
//! when a job starts. Process details come from `/proc` and are left empty elsewhere.
//!
//! The render loop only bumps an atomic counter (`frame_done`); everything else runs on
//! the writer thread.
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static FRAMES_DONE: AtomicU64 = AtomicU64::new(0);
/// 0 = not known yet (still preparing)
static FRAMES_TOTAL: AtomicU64 = AtomicU64::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static JOB: OnceLock<Mutex<Job>> = OnceLock::new();

/// One frame of the current job was rendered (or repeated). Called from segment threads.
#[inline]
pub fn frame_done() {
    FRAMES_DONE.fetch_add(1, Ordering::Relaxed);
}

/// Total frames of the current job (sum of the segment lengths).
pub fn set_total(n: u64) {
    FRAMES_TOTAL.store(n, Ordering::Relaxed);
}

pub struct Job {
    path: PathBuf,
    base: Value,
    t0: Instant,
    started_unix: f64,
    /// (seconds since t0, frames done) of the last ~10 s
    window: VecDeque<(f64, u64)>,
    written: bool,
    finished: bool,
}

/// The job status directory: `--jobs-dir` when given, else `$AGENTFX_JOBS_DIR`; None (no
/// status files) when neither is set.
pub fn dir(flag: Option<String>) -> Option<PathBuf> {
    flag.or_else(|| agentfx_ir::env::var("AGENTFX_JOBS_DIR").ok())
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}

fn now_unix() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// RFC 3339 UTC, second precision (no chrono in this crate).
pub fn iso_utc(unix: f64) -> String {
    let s = unix.max(0.0) as i64;
    let (days, rem) = (s.div_euclid(86_400), s.rem_euclid(86_400));
    // civil_from_days (H. Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname").map(|s| s.trim().to_string()).unwrap_or_else(|_| "unknown".into())
}

/// Peak resident set size of this process in MB (VmHWM), cheap /proc read.
fn peak_rss_mb() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some((kb / 1024.0 * 10.0).round() / 10.0)
}

/// (ppid, comm) from /proc/<pid>/stat; comm may contain spaces and parentheses.
fn proc_parent(pid: u32) -> Option<(u32, String)> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (open, close) = (s.find('(')?, s.rfind(')')?);
    let comm = s[open + 1..close].to_string();
    let ppid = s[close + 1..].split_whitespace().nth(1)?.parse().ok()?;
    Some((ppid, comm))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ancestor {
    pub pid: u32,
    pub comm: String,
    pub cmdline: String,
}

/// Parent chain of this process, nearest first (stops before pid 1, at most 16).
fn ancestors() -> Vec<Ancestor> {
    let mut out = Vec::new();
    #[cfg(unix)]
    let mut pid = std::os::unix::process::parent_id();
    #[cfg(not(unix))]
    let mut pid = 0u32;
    while pid > 1 && out.len() < 16 {
        let Some((ppid, comm)) = proc_parent(pid) else { break };
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).map(|b| String::from_utf8_lossy(&b).replace('\0', " ").trim().to_string()).unwrap_or_default();
        out.push(Ancestor { pid, comm, cmdline });
        pid = ppid;
    }
    out
}

/// Short label of one ancestor (`reel-api` for a process running `reel_api`, else comm).
pub fn label(a: &Ancestor) -> String {
    if a.cmdline.contains("reel_api") {
        return "reel-api".into();
    }
    let c = a.comm.trim();
    if c == "node" && a.cmdline.contains("claude") {
        return "claude".into();
    }
    if c.starts_with("tmux") {
        return "tmux".into();
    }
    c.to_string()
}

/// The caller kind from a parent chain: the nearest agent/service, else `ssh` if the
/// chain passes sshd, else `manual`. Returns (kind, index of the deciding ancestor).
pub fn classify(chain: &[Ancestor]) -> (String, Option<usize>) {
    const AGENTS: &[&str] = &["claude", "codex", "reel-api", "agy", "gemini", "cron", "systemd"];
    for (i, a) in chain.iter().enumerate() {
        let l = label(a);
        if let Some(k) = AGENTS.iter().find(|k| l == **k) {
            // a user systemd unit as nearest known ancestor: name it as such
            let k = if *k == "systemd" { "service" } else { k };
            return (k.to_string(), Some(i));
        }
    }
    if chain.iter().any(|a| a.comm.starts_with("sshd")) {
        return ("ssh".into(), None);
    }
    ("manual".into(), None)
}

/// Name of the git worktree containing `dir` (its top-level directory name). Only
/// directories strictly below `home` count (a stray `.git` in /tmp or ~ is not a project).
pub fn worktree_name(dir: &Path, home: &Path) -> Option<String> {
    let mut d = Some(dir);
    while let Some(p) = d {
        if p == home || !p.starts_with(home) {
            return None;
        }
        if p.join(".git").exists() {
            return p.file_name().map(|n| n.to_string_lossy().into_owned());
        }
        d = p.parent();
    }
    None
}

fn derive_caller(cwd: &Path, chain: &[Ancestor]) -> String {
    let (kind, idx) = classify(chain);
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/nonexistent".into()));
    let place = worktree_name(cwd, &home)
        .or_else(|| idx.and_then(|i| std::fs::read_link(format!("/proc/{}/cwd", chain[i].pid)).ok()).and_then(|p| worktree_name(&p, &home)))
        .or_else(|| cwd.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    if place.is_empty() { kind } else { format!("{kind}:{place}") }
}

fn quote(a: &str) -> String {
    if !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c)) {
        a.to_string()
    } else {
        format!("'{}'", a.replace('\'', "'\\''"))
    }
}

fn absolute(cwd: &Path, p: &Path) -> PathBuf {
    let p = if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };
    // normalize `.` and `..` without touching the file system (the output may not exist yet)
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

impl Job {
    /// Build the job record (no file is written yet).
    pub fn new(dir: &Path, args: &[String], input: &Path, out: &Path, encoder: &str, segments: usize) -> Job {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let host = hostname();
        let pid = std::process::id();
        let started_unix = now_unix();
        let id = format!("{host}-{pid}-{}", started_unix as u64);
        let chain = ancestors();
        let caller = match agentfx_ir::env::var("AGENTFX_CALLER") {
            Ok(c) if !c.trim().is_empty() => c.trim().to_string(),
            _ => derive_caller(&cwd, &chain),
        };
        let output = absolute(&cwd, out);
        let base = json!({
            "v": 1,
            "id": id,
            "host": host,
            "pid": pid,
            "command": args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" "),
            "cwd": cwd,
            "input": absolute(&cwd, input),
            "output": output,
            "output_dir": output.parent().map(Path::to_path_buf).unwrap_or_default(),
            "caller": caller,
            "caller_chain": chain.iter().map(label).collect::<Vec<_>>().join("<"),
            "encoder": encoder,
            "segments": segments,
            "version": option_env!("AGENTFX_GIT_HASH").unwrap_or("dev"),
            "started_at": iso_utc(started_unix),
            "started_unix": started_unix,
        });
        Job { path: dir.join(format!("{id}.json")), base, t0: Instant::now(), started_unix, window: VecDeque::new(), written: false, finished: false }
    }

    /// The status record now. `exit`: None while running, Some(Ok) / Some(Err(msg)) at the end.
    pub fn snapshot(&mut self, done: u64, total: u64, exit: Option<Result<(), &str>>) -> Value {
        let el = self.t0.elapsed().as_secs_f64();
        self.window.push_back((el, done));
        while self.window.len() > 2 && el - self.window[0].0 > 10.0 {
            self.window.pop_front();
        }
        let (t_a, f_a) = self.window[0];
        let fps = if el - t_a > 0.5 { (done - f_a.min(done)) as f64 / (el - t_a) } else { 0.0 };
        let avg_fps = if el > 0.0 { done as f64 / el } else { 0.0 };
        let total_v = (total > 0).then_some(total);
        let pct = total_v.map(|t| (done.min(t) as f64 / t as f64 * 1000.0).round() / 10.0);
        let eta = match (total_v, exit) {
            (Some(t), None) if done < t && fps > 0.0 => Some(((t - done) as f64 / fps).round()),
            (Some(_), None) if done > 0 => Some(0.0),
            _ => None,
        };
        let (status, phase) = match exit {
            None if total == 0 => ("running", "preparing"),
            None if done >= total => ("running", "finalizing"),
            None => ("running", "rendering"),
            Some(Ok(())) => ("done", "done"),
            Some(Err(_)) => ("failed", "failed"),
        };
        let now = now_unix();
        let mut v = self.base.clone();
        let r2 = |x: f64| (x * 100.0).round() / 100.0;
        v["status"] = json!(status);
        v["phase"] = json!(phase);
        v["updated_at"] = json!(iso_utc(now));
        v["updated_unix"] = json!(now);
        v["frames_done"] = json!(done);
        v["frames_total"] = json!(total_v);
        v["percent"] = json!(pct);
        v["elapsed_s"] = json!(r2(el));
        v["eta_s"] = json!(eta);
        v["fps"] = json!(r2(if exit.is_some() { avg_fps } else { fps }));
        v["avg_fps"] = json!(r2(avg_fps));
        v["peak_rss_mb"] = json!(peak_rss_mb());
        if let Some(e) = exit {
            v["finished_at"] = json!(iso_utc(now));
            v["duration_s"] = json!(r2(now - self.started_unix));
            v["exit_code"] = json!(if e.is_ok() { 0 } else { 1 });
            v["error"] = match e {
                Ok(()) => Value::Null,
                Err(m) => json!(m.chars().take(2000).collect::<String>()),
            };
        }
        v
    }

    fn write(&mut self, v: &Value) {
        let tmp = self.path.with_extension("json.tmp");
        let ok = std::fs::create_dir_all(self.path.parent().unwrap_or(Path::new(".")))
            .and_then(|_| std::fs::write(&tmp, serde_json::to_vec_pretty(v).unwrap_or_default()))
            .and_then(|_| std::fs::rename(&tmp, &self.path));
        if ok.is_ok() {
            self.written = true;
        }
    }

    /// Periodic write: skipped for the first second and for one-frame jobs.
    fn tick(&mut self, done: u64, total: u64) {
        let v = self.snapshot(done, total, None);
        if self.finished || self.t0.elapsed() < Duration::from_millis(900) || total == 1 {
            return;
        }
        self.write(&v);
    }

    fn finish(&mut self, done: u64, total: u64, r: Result<(), &str>) {
        let v = self.snapshot(done, total, Some(r));
        if self.written || total >= 2 {
            self.write(&v);
        }
        self.finished = true;
    }
}

/// Delete status files (and stray temp files) older than `max_age` in `dir`.
pub fn prune(dir: &Path, max_age: Duration) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let ours = p.extension().is_some_and(|x| x == "json" || x == "tmp");
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > max_age);
        if ours && old && e.file_type().is_ok_and(|t| t.is_file()) {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// Start status reporting for this process into `dir` (no-op when `dir` is None).
pub fn start(dir: Option<PathBuf>, args: &[String], input: &Path, out: &Path, encoder: &str, segments: usize) {
    let Some(dir) = dir else { return };
    if ACTIVE.swap(true, Ordering::SeqCst) {
        return;
    }
    let job = Job::new(&dir, args, input, out, encoder, segments);
    if JOB.set(Mutex::new(job)).is_err() {
        return;
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // try_lock: never block inside a panic
        if let Some(Ok(mut j)) = JOB.get().map(|m| m.try_lock()) {
            j.finish(FRAMES_DONE.load(Ordering::Relaxed), FRAMES_TOTAL.load(Ordering::Relaxed), Err(&format!("panic: {info}")));
        }
        prev(info);
    }));
    let _ = std::thread::Builder::new().name("reel-status".into()).spawn(move || {
        prune(&dir, Duration::from_secs(7 * 86_400));
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let Some(m) = JOB.get() else { return };
            let Ok(mut j) = m.lock() else { return };
            if j.finished {
                return;
            }
            j.tick(FRAMES_DONE.load(Ordering::Relaxed), FRAMES_TOTAL.load(Ordering::Relaxed));
        }
    });
}

/// Final status (done / failed + error). No-op when no job was started.
pub fn finish(r: Result<(), &str>) {
    if let Some(m) = JOB.get() {
        if let Ok(mut j) = m.lock() {
            j.finish(FRAMES_DONE.load(Ordering::Relaxed), FRAMES_TOTAL.load(Ordering::Relaxed), r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anc(comm: &str, cmd: &str) -> Ancestor {
        Ancestor { pid: 2, comm: comm.into(), cmdline: cmd.into() }
    }

    #[test]
    fn opt_in_dir() {
        assert_eq!(dir(Some("/x/jobs".into())), Some(PathBuf::from("/x/jobs")));
        assert_eq!(dir(Some("  ".into())), None);
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso_utc(0.0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400.0), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(1_793_178_245.0), "2026-10-28T09:04:05Z");
    }

    #[test]
    fn caller_kinds() {
        let chain = vec![anc("bash", "bash -c x"), anc("claude", "/home/u/.local/bin/claude"), anc("tmux: server", "tmux")];
        assert_eq!(classify(&chain), ("claude".into(), Some(1)));
        let api = vec![anc("nice", "nice -n 19 reel"), anc("reel", "agentfx render"), anc("python3", "/usr/bin/python3 /home/u/reel-api/reel_api.py")];
        assert_eq!(classify(&api).0, "reel-api");
        let ssh = vec![anc("bash", "bash"), anc("flock", "flock"), anc("sshd-session", "sshd-session: u@notty")];
        assert_eq!(classify(&ssh), ("ssh".into(), None));
        assert_eq!(classify(&[anc("bash", "-bash"), anc("tmux: server", "tmux")]).0, "manual");
        assert_eq!(label(&anc("tmux: server", "tmux new")), "tmux");
        assert_eq!(label(&anc("node", "node /x/claude-code/cli.js")), "claude");
    }

    #[test]
    fn worktree_and_paths() {
        let t = std::env::temp_dir().join(format!("reel-js-wt-{}", std::process::id()));
        std::fs::create_dir_all(t.join("my-wt/a/b")).unwrap();
        std::fs::write(t.join("my-wt/.git"), "gitdir: x").unwrap();
        assert_eq!(worktree_name(&t.join("my-wt/a/b"), &t).as_deref(), Some("my-wt"));
        // outside / at the home boundary nothing counts
        assert_eq!(worktree_name(&t.join("my-wt/a"), &t.join("my-wt")), None);
        assert_eq!(worktree_name(&t.join("my-wt/a"), Path::new("/nonexistent")), None);
        assert_eq!(absolute(Path::new("/x/y"), Path::new("../z/./o.mp4")), PathBuf::from("/x/z/o.mp4"));
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("--out"), "--out");
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn snapshot_progress_eta_and_atomic_file() {
        let d = std::env::temp_dir().join(format!("reel-js-{}", std::process::id()));
        let args: Vec<String> = ["agentfx-render", "render", "s.ir.json", "--out", "o/v.mp4"].iter().map(|s| s.to_string()).collect();
        let mut j = Job::new(&d, &args, Path::new("s.ir.json"), Path::new("o/v.mp4"), "X264", 1);
        assert!(j.base["output"].as_str().unwrap().ends_with("/o/v.mp4"));
        assert!(j.base["output_dir"].as_str().unwrap().ends_with("/o"));
        assert!(j.base["id"].as_str().unwrap().contains(&format!("-{}-", std::process::id())));
        let v = j.snapshot(0, 0, None);
        assert_eq!(v["phase"], "preparing");
        assert!(v["percent"].is_null() && v["frames_total"].is_null());
        // simulate 1 s of rendering at 50 fps
        j.window.clear();
        j.window.push_back((j.t0.elapsed().as_secs_f64() - 1.0, 0));
        let v = j.snapshot(50, 200, None);
        assert_eq!(v["status"], "running");
        assert_eq!(v["percent"], 25.0);
        let fps = v["fps"].as_f64().unwrap();
        assert!((40.0..60.0).contains(&fps), "fps {fps}");
        let eta = v["eta_s"].as_f64().unwrap();
        assert!((2.0..5.0).contains(&eta), "eta {eta}");
        assert_eq!(j.snapshot(200, 200, None)["phase"], "finalizing");
        // end states, atomic write leaves no temp file
        j.finish(200, 200, Err("boom"));
        let s: Value = serde_json::from_slice(&std::fs::read(&j.path).unwrap()).unwrap();
        assert_eq!(s["status"], "failed");
        assert_eq!(s["exit_code"], 1);
        assert_eq!(s["error"], "boom");
        assert!(!j.path.with_extension("json.tmp").exists());
        let v = j.snapshot(200, 200, Some(Ok(())));
        assert_eq!(v["status"], "done");
        assert!(v["eta_s"].is_null());
        // one-frame jobs that never wrote stay silent
        let mut k = Job::new(&d, &args, Path::new("a"), Path::new("b.png"), "X264", 1);
        k.path = d.join("one-frame.json");
        k.tick(1, 1);
        k.finish(1, 1, Ok(()));
        assert!(!k.path.exists());
        prune(&d, Duration::ZERO);
        assert!(!j.path.exists());
        let _ = std::fs::remove_dir(&d);
    }
}
