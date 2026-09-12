use std::io::Read;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
mod screen_record {
    extern "C++Qt" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, is_running)]
        #[qproperty(u64, elapsed)]
        #[qproperty(bool, audio)]
        #[qproperty(bool, display)]
        #[qproperty(bool, replay, READ, WRITE = set_replay, NOTIFY = replay_changed)]
        #[qproperty(QString, monitor)]
        #[qproperty(u64, fps, READ, WRITE=set_fps, NOTIFY=fps_changed)]
        #[qproperty(u64, duration, READ, WRITE=set_duration, NOTIFY=duration_changed)]
        type ScreenRec = super::ScreenRecord;

        #[qinvokable]
        fn start(self: Pin<&mut Self>, path: &QString);

        #[qinvokable]
        fn stop(self: Pin<&mut Self>);

        #[qinvokable]
        fn clip(self: Pin<&mut Self>);

        #[qinvokable]
        fn pkill(self: Pin<&mut Self>);

        #[qsignal]
        fn started(self: Pin<&mut Self>);

        #[qsignal]
        fn finished(self: Pin<&mut Self>, path: QString);

        #[qsignal]
        fn clipped(self: Pin<&mut Self>, path: QString);

        #[qsignal]
        fn error(self: Pin<&mut Self>, message: QString);

        #[qsignal]
        fn replay_changed(self: Pin<&mut Self>);

        #[qsignal]
        fn duration_changed(self: Pin<&mut Self>);

        #[qsignal]
        fn fps_changed(self: Pin<&mut Self>);

        fn set_replay(self: Pin<&mut Self>, value: bool);
        fn set_duration(self: Pin<&mut Self>, value: u64);
        fn set_fps(self: Pin<&mut Self>, value: u64);

    }

    impl cxx_qt::Constructor<()> for ScreenRec {}
    impl cxx_qt::Threading for ScreenRec {}
}

pub struct ScreenRecord {
    pub is_running: bool,
    pub elapsed: u64,
    pub audio: bool,
    pub display: bool,
    pub replay: bool,
    pub fps: u64,
    pub monitor: QString,
    pub duration: u64,
}

impl Default for ScreenRecord {
    fn default() -> Self {
        Self {
            is_running: false,
            elapsed: 0,
            audio: true,
            display: true,
            replay: false,
            monitor: QString::from(""),
            fps: 60,
            duration: 30,
        }
    }
}

const RECORDER_BIN: &str = "gpu-screen-recorder";
/// Legacy binary from before the gpu-screen-recorder migration; only used to
/// clean up orphan daemons left running by older versions.
const LEGACY_BIN: &str = "wl-screenrec";

impl cxx_qt::Initialize for screen_record::ScreenRec {
    fn initialize(self: Pin<&mut Self>) {
        // Always reap daemons left behind by the pre-migration wl-screenrec backend.
        kill_legacy_daemon();
        if *self.replay() {
            let duration = *self.duration();
            let fps = *self.fps();
            let monitor = self.monitor().to_string();
            let audio = *self.audio();
            let qt_thread = self.qt_thread();
            spawn_replay_process(qt_thread, duration, fps, monitor, audio);
        } else {
            // crash fix: if previous program crashed with replay active, orphan pid remains.
            // When new instance starts with replay=false, clean it up instead of leaking.
            if let Some((pid, _dir)) = read_pid_file() {
                if is_pid_alive(pid) {
                    eprintln!("initialize: replay=false but orphan pid {pid} exists, cleaning");
                    let _ = Command::new("kill")
                        .arg("-INT")
                        .arg(pid.to_string())
                        .status();
                    for _ in 0..10 {
                        thread::sleep(Duration::from_millis(100));
                        if !is_pid_alive(pid) {
                            break;
                        }
                    }
                    if is_pid_alive(pid) {
                        let _ = Command::new("kill")
                            .arg("-KILL")
                            .arg(pid.to_string())
                            .status();
                    }
                }
                let _ = std::fs::remove_file(replay_pid_path());
            }
            // also do general stale file cleanup (legacy wl-screenrec temp files)
            cleanup_old_history_files(&replay_temp_dir());
        }
    }
}

/// Resolve the gsr capture target (`-w` value) from an explicit monitor name.
/// Empty means "first monitor found".
fn capture_target(monitor: &str) -> String {
    let m = monitor.trim();
    if m.is_empty() {
        "screen".to_string()
    } else {
        m.to_string()
    }
}

fn slurp_geometry(display: bool) -> Result<String, Box<dyn std::error::Error>> {
    let mut cmd = Command::new("slurp");
    if display {
        cmd.args(["-o", "-f", "%o"]);
    } else {
        cmd.args(["-f", "%wx%h+%x+%y"]);
    }
    let out = cmd.output()?;
    if !out.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

struct ActiveRecording {
    child: Arc<Mutex<Child>>,
    pid: u32,
    stop: Arc<AtomicBool>,
}
static ACTIVE: OnceLock<Mutex<Option<ActiveRecording>>> = OnceLock::new();
fn active() -> &'static Mutex<Option<ActiveRecording>> {
    ACTIVE.get_or_init(|| Mutex::new(None))
}

struct ReplayRecording {
    child: Option<Arc<Mutex<Child>>>,
    pid: u32,
    /// Output directory (`-o` in replay mode is a directory).
    dir: PathBuf,
}
static REPLAY: OnceLock<Mutex<Option<ReplayRecording>>> = OnceLock::new();
fn replay_active() -> &'static Mutex<Option<ReplayRecording>> {
    REPLAY.get_or_init(|| Mutex::new(None))
}
// Serialize clip(): a second clip while one finalizes would signal a dying pid.
static CLIP_BUSY: AtomicBool = AtomicBool::new(false);

// --- recording elapsed timer: generation counter kills stale timer threads ---
static TIMER_GEN: AtomicU64 = AtomicU64::new(0);
fn rec_timer_start(qt_thread: cxx_qt::CxxQtThread<screen_record::ScreenRec>) {
    let generation = TIMER_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let t0 = Instant::now();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        if TIMER_GEN.load(Ordering::SeqCst) != generation {
            break;
        }
        let secs = t0.elapsed().as_secs();
        let _ = qt_thread.queue(move |mut q| {
            q.as_mut().set_elapsed(secs);
        });
    });
}
fn rec_timer_stop() {
    TIMER_GEN.fetch_add(1, Ordering::SeqCst);
}

// --- crash-resilience helpers: pid file to survive program crash ---
fn replay_temp_dir() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}
fn replay_pid_path() -> PathBuf {
    replay_temp_dir().join("gsr-history.pid")
}
fn replay_log_path() -> PathBuf {
    replay_temp_dir().join("gsr-history.log")
}
/// Pid file used by the pre-migration wl-screenrec backend.
fn legacy_pid_path() -> PathBuf {
    replay_temp_dir().join("wl-screenrec-history.pid")
}
fn cmdline_contains(pid: u32, needle: &str) -> Option<bool> {
    let proc_path = format!("/proc/{pid}");
    if !Path::new(&proc_path).exists() {
        return Some(false);
    }
    let cmdline = std::fs::read_to_string(format!("{proc_path}/cmdline")).ok()?;
    if cmdline.is_empty() {
        // zombie -> treat as not alive
        return Some(false);
    }
    Some(cmdline.contains(needle))
}
fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    match cmdline_contains(pid, RECORDER_BIN) {
        Some(alive) => {
            if alive {
                return true;
            }
            // pid exists but is some other process (reuse) -> not ours.
            // Only fall back to kill -0 when cmdline was unreadable (None).
            if Path::new(&format!("/proc/{pid}")).exists() {
                return false;
            }
            false
        }
        None => {
            if !Path::new(&format!("/proc/{pid}")).exists() {
                return false;
            }
            // fallback kill -0
            Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    }
}
fn write_pid_file(pid: u32, dir: &Path, duration: u64, fps: u64) -> std::io::Result<()> {
    let pid_path = replay_pid_path();
    if let Some(parent) = pid_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // store pid, output dir, duration, fps for crash recovery & duration-mismatch detection
    std::fs::write(
        &pid_path,
        format!("{}\n{}\n{} {}\n", pid, dir.display(), duration, fps),
    )
}
fn read_pid_file() -> Option<(u32, PathBuf)> {
    let content = std::fs::read_to_string(replay_pid_path()).ok()?;
    let mut lines = content.lines();
    let pid_str = lines.next()?.trim();
    let pid: u32 = pid_str.parse().ok()?;
    let dir_str = lines.next().unwrap_or("").trim();
    let dir = if dir_str.is_empty() {
        PathBuf::from("")
    } else {
        PathBuf::from(dir_str)
    };
    Some((pid, dir))
}
fn read_pid_file_with_params() -> Option<(u32, PathBuf, u64, u64)> {
    let content = std::fs::read_to_string(replay_pid_path()).ok()?;
    let mut lines = content.lines();
    let pid_str = lines.next()?.trim();
    let pid: u32 = pid_str.parse().ok()?;
    let dir_str = lines.next().unwrap_or("").trim().to_string();
    let dir = if dir_str.is_empty() {
        PathBuf::from("")
    } else {
        PathBuf::from(dir_str)
    };
    let params_line = lines.next().unwrap_or("").trim().to_string();
    let mut parts = params_line.split_whitespace();
    let duration = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let fps = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Some((pid, dir, duration, fps))
}
fn clear_pid_file_if_matches(pid: u32) {
    if let Some((existing, _)) = read_pid_file() {
        if existing == pid {
            let _ = std::fs::remove_file(replay_pid_path());
        }
    }
}
fn cleanup_stale_pid_file() {
    if let Some((pid, _dir)) = read_pid_file() {
        if !is_pid_alive(pid) {
            let _ = std::fs::remove_file(replay_pid_path());
        }
    }
}

/// Best-effort cleanup of a daemon left running by the old wl-screenrec backend.
fn kill_legacy_daemon() {
    let legacy_pid = std::fs::read_to_string(legacy_pid_path())
        .ok()
        .and_then(|c| c.lines().next().unwrap_or("").trim().parse::<u32>().ok());
    if let Some(pid) = legacy_pid {
        let alive = cmdline_contains(pid, LEGACY_BIN).unwrap_or(false);
        if alive {
            eprintln!("initialize: killing legacy {LEGACY_BIN} orphan pid={pid}");
            let _ = Command::new("kill")
                .arg("-INT")
                .arg(pid.to_string())
                .status();
            for _ in 0..10 {
                thread::sleep(Duration::from_millis(100));
                if cmdline_contains(pid, LEGACY_BIN) != Some(true) {
                    break;
                }
            }
            if cmdline_contains(pid, LEGACY_BIN) == Some(true) {
                let _ = Command::new("kill")
                    .arg("-KILL")
                    .arg(pid.to_string())
                    .status();
            }
        }
        let _ = std::fs::remove_file(legacy_pid_path());
    }
}

fn default_videos_dir() -> std::path::PathBuf {
    if let Some(dir) = dirs::video_dir() {
        return dir;
    }
    if let Some(home) = dirs::home_dir() {
        return home.join("Videos");
    }
    std::path::PathBuf::from("/tmp")
}

// --- helpers ---

fn clamp_history_params(duration: u64, fps: u64) -> (u64, u64) {
    // Bound RAM: replay buffer lives in memory (`-replay-storage ram` default).
    // gpu-screen-recorder itself accepts -r 2..86400.
    (duration.clamp(2, 3600), fps.clamp(1, 120))
}

fn cleanup_old_history_files(dir: &Path) {
    // Remove stale wl-screenrec-history-*.mp4 left by crashes of the old backend.
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("wl-screenrec-history-") || !name.ends_with(".mp4") {
            continue;
        }
        // remove files older than 1h or empty stale files
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    if elapsed > Duration::from_secs(3600) {
                        let _ = std::fs::remove_file(entry.path());
                        eprintln!("cleanup: removed stale {}", entry.path().display());
                    }
                }
            }
            // also remove 0-byte files left if process never flushed
            if meta.len() == 0 {
                // keep very recent empty file (just created) - only delete if >5min old
                if let Ok(modified) = meta.modified() {
                    if let Ok(elapsed) = modified.elapsed() {
                        if elapsed > Duration::from_secs(300) {
                            let _ = std::fs::remove_file(entry.path());
                        }
                    }
                }
            }
        }
    }
}

fn spawn_stderr_collector(stderr: Option<std::process::ChildStderr>, buf: Arc<Mutex<String>>) {
    // Drain stderr concurrently with bounded buffer to avoid pipe fill deadlock.
    thread::spawn(move || {
        if let Some(mut f) = stderr {
            let mut tmp = [0u8; 4096];
            loop {
                match f.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&tmp[..n]);
                        let mut g = buf.lock().unwrap_or_else(|e| e.into_inner());
                        g.push_str(&chunk);
                        const MAX: usize = 8192;
                        if g.len() > MAX {
                            let excess = g.len() - MAX;
                            g.drain(..excess);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    });
}

/// Send SIGUSR1 to save the replay buffer: targeted pid first, then a
/// `pkill` fallback for daemons we don't track.
/// `killall` is not installed on all systems, so `pkill` is used instead.
fn signal_save_replay(pid: u32) -> bool {
    if pid != 0 && is_pid_alive(pid) {
        if Command::new("kill")
            .args(["-USR1", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return true;
        }
    }
    // fallback: signal any running recorder daemon.
    // NOTE: unanchored match - the binary is often launched via a wrapper so
    // argv[0] is an absolute store path, not "^gpu-screen-recorder".
    Command::new("pkill")
        .args(["-SIGUSR1", "-f", "gpu-screen-recorder"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Find the replay file saved by a clip: the newest `Replay_*` video in `dir`
/// modified at or after `since`. gpu-screen-recorder writes e.g.
/// `Replay_2026-08-05_14-04-22.mp4` into the replay output directory.
fn find_saved_replay(dir: &Path, since: SystemTime) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("Replay_") {
            continue;
        }
        let ext_ok = name.ends_with(".mp4") || name.ends_with(".mkv") || name.ends_with(".webm");
        if !ext_ok {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.len() == 0 {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < since {
            continue;
        }
        let replace = match &best {
            Some((t, _)) => modified > *t,
            None => true,
        };
        if replace {
            best = Some((modified, entry.path()));
        }
    }
    best.map(|(_, p)| p)
}

fn spawn_replay_process(
    qt_thread: cxx_qt::CxxQtThread<screen_record::ScreenRec>,
    duration: u64,
    fps: u64,
    monitor: String,
    audio: bool,
) {
    // poison-safe check
    if replay_active()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
    {
        return;
    }
    // --- crash resilience: adopt orphan if program previously crashed ---
    // If pid file exists and process still alive, reuse it instead of spawning duplicate.
    // Fix over-recording: if stored duration/fps differs from requested, kill orphan and spawn fresh.
    if let Some((pid, dir, stored_dur, stored_fps)) = read_pid_file_with_params() {
        if is_pid_alive(pid) {
            let (req_dur, req_fps) = clamp_history_params(duration, fps);
            // stored 0 means legacy file without params -> treat as mismatch
            let mismatch = stored_dur == 0
                || stored_fps == 0
                || stored_dur != req_dur
                || stored_fps != req_fps;
            if mismatch {
                eprintln!(
                    "spawn_replay: orphan pid={} stored {}s@{} != requested {}s@{} -> killing for correct duration",
                    pid, stored_dur, stored_fps, req_dur, req_fps
                );
                let _ = Command::new("kill")
                    .arg("-INT")
                    .arg(pid.to_string())
                    .status();
                for _ in 0..10 {
                    thread::sleep(Duration::from_millis(100));
                    if !is_pid_alive(pid) {
                        break;
                    }
                }
                if is_pid_alive(pid) {
                    let _ = Command::new("kill")
                        .arg("-KILL")
                        .arg(pid.to_string())
                        .status();
                }
                let _ = std::fs::remove_file(replay_pid_path());
            } else {
                // adopt existing with matching duration
                {
                    let mut g = replay_active().lock().unwrap_or_else(|e| e.into_inner());
                    *g = Some(ReplayRecording {
                        child: None,
                        pid,
                        dir: dir.clone(),
                    });
                }
                eprintln!(
                    "spawn_replay: adopted existing orphan pid={} dir={} duration={} fps={}",
                    pid,
                    dir.display(),
                    stored_dur,
                    stored_fps
                );
                // monitor adopted pid via polling (no Child handle)
                let qt_clone = qt_thread.clone();
                thread::spawn(move || {
                    loop {
                        thread::sleep(Duration::from_millis(500));
                        if !is_pid_alive(pid) {
                            *replay_active().lock().unwrap_or_else(|e| e.into_inner()) = None;
                            clear_pid_file_if_matches(pid);
                            // best-effort log tail for error
                            let log_tail = std::fs::read_to_string(replay_log_path())
                                .map(|s| {
                                    let t = s.trim();
                                    if t.len() > 800 {
                                        format!(": {}", &t[t.len() - 800..])
                                    } else if t.is_empty() {
                                        String::new()
                                    } else {
                                        format!(": {}", t)
                                    }
                                })
                                .unwrap_or_default();
                            let msg = QString::from(format!(
                                "{RECORDER_BIN} replay pid {pid} exited{log_tail}"
                            ));
                            let _ = qt_clone.queue(move |mut q| {
                                q.as_mut().error(msg);
                            });
                            break;
                        }
                    }
                });
                return;
            }
        } else {
            // stale pid file
            cleanup_stale_pid_file();
        }
    } else if let Some((pid, _dir)) = read_pid_file() {
        // legacy 2-line file
        if is_pid_alive(pid) {
            eprintln!(
                "spawn_replay: legacy orphan pid={} -> killing for fresh duration",
                pid
            );
            let _ = Command::new("kill")
                .arg("-INT")
                .arg(pid.to_string())
                .status();
            for _ in 0..10 {
                thread::sleep(Duration::from_millis(100));
                if !is_pid_alive(pid) {
                    break;
                }
            }
            if is_pid_alive(pid) {
                let _ = Command::new("kill")
                    .arg("-KILL")
                    .arg(pid.to_string())
                    .status();
            }
        }
        let _ = std::fs::remove_file(replay_pid_path());
    }

    let (orig_dur, orig_fps) = (duration, fps);
    let (duration, fps) = clamp_history_params(duration, fps);
    if duration != orig_dur || fps != orig_fps {
        eprintln!(
            "spawn_replay: clamped duration {}->{} fps {}->{}",
            orig_dur, duration, orig_fps, fps
        );
    }

    // In replay mode -o is the output directory: SIGUSR1 saves Replay_*.mp4 there.
    let videos_dir = default_videos_dir();
    let _ = std::fs::create_dir_all(&videos_dir);
    // also clear stale pid file if any race
    cleanup_stale_pid_file();

    let target = capture_target(&monitor);
    eprintln!("spawn_replay: using capture target -w '{target}'");

    let mut cmd = Command::new(RECORDER_BIN);
    cmd.arg("-w").arg(&target);
    cmd.arg("-c").arg("mp4");
    cmd.arg("-f").arg(fps.to_string());
    cmd.arg("-r").arg(duration.to_string());
    if audio {
        cmd.arg("-a").arg("default_output");
    }
    cmd.arg("-o").arg(&videos_dir);
    // --- crash fix: fully detach stdio so child survives parent crash ---
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    // try log file for debugging, fallback to null
    let log_path = replay_log_path();
    match std::fs::File::create(&log_path) {
        Ok(f) => {
            // truncate existing log
            cmd.stderr(f);
        }
        Err(_) => {
            cmd.stderr(Stdio::null());
        }
    }

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = qt_thread.queue(move |mut q| {
                q.as_mut()
                    .error(QString::from(format!("spawn {RECORDER_BIN} replay: {e}")));
            });
            return;
        }
    };

    let pid = child.id();
    // no piped stderr anymore; log file holds errors
    let child_arc = Arc::new(Mutex::new(child));
    // Clone dir for storage and for logging
    let stored_dir = videos_dir.clone();
    eprintln!(
        "spawn_replay: {RECORDER_BIN} -w {target} -c mp4 -f {fps} -r {duration} -o {} pid={}",
        videos_dir.display(),
        pid
    );
    // write pid file for crash recovery (pid + dir + duration/fps)
    if let Err(e) = write_pid_file(pid, &stored_dir, duration, fps) {
        eprintln!("spawn_replay: failed to write pid file: {e}");
    }
    *replay_active().lock().unwrap_or_else(|e| e.into_inner()) = Some(ReplayRecording {
        child: Some(child_arc.clone()),
        pid,
        dir: stored_dir,
    });

    let qt_thread_clone = qt_thread.clone();
    thread::spawn(move || {
        // Poll wait without holding lock continuously.
        let status = loop {
            thread::sleep(Duration::from_millis(200));
            let mut guard = match child_arc.try_lock() {
                Ok(g) => g,
                Err(_) => continue,
            };
            match guard.try_wait() {
                Ok(Some(s)) => break Ok(s),
                Ok(None) => continue,
                Err(e) => break Err(e),
            }
        };

        // read log tail for error detail (instead of piped buf)
        let log_tail = std::fs::read_to_string(replay_log_path())
            .map(|s| {
                let t = s.trim();
                if t.is_empty() {
                    String::new()
                } else if t.len() > 4000 {
                    format!(": {}", &t[t.len() - 4000..])
                } else {
                    format!(": {}", t)
                }
            })
            .unwrap_or_default();

        // Clear active on exit - poison safe
        *replay_active().lock().unwrap_or_else(|e| e.into_inner()) = None;
        clear_pid_file_if_matches(pid);

        match status {
            Ok(s) if s.success() => {
                // normal exit, no signal needed
            }
            Ok(s) => {
                let msg = QString::from(format!("{RECORDER_BIN} replay exited {s}{log_tail}"));
                let _ = qt_thread_clone.queue(move |mut q| {
                    q.as_mut().error(msg);
                });
            }
            Err(e) => {
                let msg = QString::from(format!("wait {RECORDER_BIN} replay: {e}{log_tail}"));
                let _ = qt_thread_clone.queue(move |mut q| {
                    q.as_mut().error(msg);
                });
            }
        }
    });
}

fn stop_replay_process(qt_thread: cxx_qt::CxxQtThread<screen_record::ScreenRec>) {
    let rec = replay_active()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    // also handle orphan case where REPLAY was None but pid file exists
    let rec = rec.or_else(|| {
        if let Some((pid, dir)) = read_pid_file() {
            if is_pid_alive(pid) {
                Some(ReplayRecording {
                    child: None,
                    pid,
                    dir,
                })
            } else {
                cleanup_stale_pid_file();
                None
            }
        } else {
            None
        }
    });
    if let Some(rec) = rec {
        let pid = rec.pid;
        let child = rec.child;
        // clear pid file immediately to prevent re-adopt
        clear_pid_file_if_matches(pid);
        // Don't block Qt thread in set_replay; spawn thread to kill.
        // SIGINT in replay mode stops without saving.
        thread::spawn(move || {
            if pid != 0 {
                let _ = Command::new("kill")
                    .arg("-INT")
                    .arg(pid.to_string())
                    .status();
                for _ in 0..10 {
                    thread::sleep(Duration::from_millis(100));
                    if is_pid_alive(pid) {
                        if let Some(ref ch) = child {
                            if let Ok(mut guard) = ch.try_lock() {
                                if let Ok(Some(_)) = guard.try_wait() {
                                    break;
                                }
                            }
                        }
                    } else {
                        break;
                    }
                }
                if is_pid_alive(pid) {
                    let _ = Command::new("kill")
                        .arg("-KILL")
                        .arg(pid.to_string())
                        .status();
                }
                if let Some(ch) = child {
                    if let Ok(mut guard) = ch.try_lock() {
                        if let Ok(None) = guard.try_wait() {
                            let _ = guard.kill();
                            let _ = guard.wait();
                        }
                    }
                }
            }
            // NOTE: replay output dir is $HOME/Videos - never delete it or its
            // content here; saved clips must survive daemon shutdown.
            eprintln!("stop_replay: stopped pid={pid}");
        });
        let _ = qt_thread.queue(move |mut q| {
            // no is_running change for replay, just ensure UI updated if needed
            q.as_mut().replay_changed();
        });
    }
}

/// SIGINT a pid, wait up to `wait_tenths` x 100ms for exit, SIGKILL fallback.
/// INT lets the recorder finalize a valid mp4; KILL may truncate it.
fn kill_pid_graceful(pid: u32, child: Option<&Arc<Mutex<Child>>>, wait_tenths: u32) {
    if pid == 0 || !is_pid_alive(pid) {
        return;
    }
    let _ = Command::new("kill")
        .arg("-INT")
        .arg(pid.to_string())
        .status();
    for _ in 0..wait_tenths {
        thread::sleep(Duration::from_millis(100));
        let reaped = match child {
            Some(ch) => match ch.try_lock() {
                Ok(mut g) => matches!(g.try_wait(), Ok(Some(_))),
                Err(_) => false,
            },
            None => false,
        };
        if reaped || !is_pid_alive(pid) {
            return;
        }
    }
    eprintln!("pkill: pid {pid} still alive, sending KILL (file may be corrupt)");
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .status();
}

impl screen_record::ScreenRec {
    pub fn set_replay(mut self: Pin<&mut Self>, value: bool) {
        eprintln!("set_replay called: {} -> {}", *self.replay(), value);
        // Avoid duplicate work if value unchanged (getter is *self.replay())
        if *self.replay() == value {
            eprintln!("set_replay: no change, ignoring");
            return;
        }
        // Update Rust field and notify QML
        self.as_mut().rust_mut().replay = value;
        self.as_mut().replay_changed();
        eprintln!("set_replay: replay_changed emitted, now {}", value);

        if value {
            // start replay daemon: gpu-screen-recorder -w <target> -c mp4
            // -f <fps> -r <duration> [-a default_output] -o <Videos>
            let duration = *self.duration();
            let fps = *self.fps();
            let monitor = self.monitor().to_string();
            let audio = *self.audio();
            eprintln!(
                "set_replay: starting replay duration={} fps={} monitor='{}' audio={}",
                duration, fps, monitor, audio
            );
            let qt_thread = self.qt_thread();
            spawn_replay_process(qt_thread, duration, fps, monitor, audio);
        } else {
            // stop replay daemon if running (SIGINT stops without saving)
            eprintln!("set_replay: stopping replay");
            let qt_thread = self.qt_thread();
            stop_replay_process(qt_thread);
        }
    }

    pub fn set_duration(mut self: Pin<&mut Self>, value: u64) {
        if *self.duration() == value {
            return;
        }
        self.as_mut().rust_mut().duration = value;
        self.as_mut().duration_changed();
        eprintln!("set_duration: {} -> {}", *self.duration(), value);
        if *self.replay() {
            // restart replay to honor new duration
            let qt_thread = self.qt_thread();
            stop_replay_process(qt_thread.clone());
            let duration = value;
            let fps = *self.fps();
            let monitor = self.monitor().to_string();
            let audio = *self.audio();
            let qt2 = self.qt_thread();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(400));
                spawn_replay_process(qt2, duration, fps, monitor, audio);
            });
        }
    }

    pub fn set_fps(mut self: Pin<&mut Self>, value: u64) {
        if *self.fps() == value {
            return;
        }
        self.as_mut().rust_mut().fps = value;
        self.as_mut().fps_changed();
        eprintln!("set_fps: {} -> {}", *self.fps(), value);
        if *self.replay() {
            let qt_thread = self.qt_thread();
            stop_replay_process(qt_thread.clone());
            let duration = *self.duration();
            let fps = value;
            let monitor = self.monitor().to_string();
            let audio = *self.audio();
            let qt2 = self.qt_thread();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(400));
                spawn_replay_process(qt2, duration, fps, monitor, audio);
            });
        }
    }

    fn start(mut self: Pin<&mut Self>, path: &QString) {
        if *self.is_running() {
            return;
        }
        let out = path.to_string();
        if out.is_empty() {
            self.as_mut().error(QString::from("empty output path"));
            return;
        }
        if Path::new(&out).is_dir() {
            self.as_mut()
                .error(QString::from(format!("output path is a directory: {out}")));
            return;
        }
        if active().lock().unwrap_or_else(|e| e.into_inner()).is_some() {
            return;
        }

        // is_running/started/elapsed fire only once gsr actually captures
        // (see watcher below) - not while it sits behind auth prompts.
        let qt_thread = self.qt_thread();
        let audio = *self.audio();
        let fps = *self.fps();
        let monitor_cfg = self.monitor().to_string();

        let _ = std::fs::remove_file(&out);

        let display = *self.display();
        // Resolve capture target: slurp selection wins, then monitor, then screen.
        let mut cmd = Command::new(RECORDER_BIN);
        if let Ok(geo) = slurp_geometry(display) {
            if !geo.is_empty() {
                if display {
                    // slurp -o picks an output name
                    cmd.arg("-w").arg(geo);
                } else {
                    cmd.arg("-w").arg("region").arg("-region").arg(geo);
                }
            } else if !monitor_cfg.trim().is_empty() {
                cmd.arg("-w").arg(monitor_cfg.trim());
            } else {
                cmd.arg("-w").arg("screen");
            }
        } else if !monitor_cfg.trim().is_empty() {
            cmd.arg("-w").arg(monitor_cfg.trim());
        } else {
            cmd.arg("-w").arg("screen");
        }
        cmd.arg("-c").arg("mp4");
        cmd.arg("-f").arg(fps.clamp(1, 120).to_string());
        if audio {
            cmd.arg("-a").arg("default_output");
        }
        cmd.arg("-o").arg(&out);
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.as_mut().set_is_running(false);
                self.as_mut()
                    .error(QString::from(format!("spawn {RECORDER_BIN}: {e}")));
                return;
            }
        };

        let pid = child.id();
        let stderr = child.stderr.take();
        let err_buf = Arc::new(Mutex::new(String::new()));
        spawn_stderr_collector(stderr, err_buf.clone());

        let child_arc = Arc::new(Mutex::new(child));
        let stop = Arc::new(AtomicBool::new(false));
        *active().lock().unwrap_or_else(|e| e.into_inner()) = Some(ActiveRecording {
            child: child_arc.clone(),
            pid,
            stop: stop.clone(),
        });

        // reset display; the elapsed timer starts only once capture is live.
        self.as_mut().set_elapsed(0);

        thread::spawn(move || {
            // Gate promotion on live capture: gsr may wait behind a
            // pkexec/portal prompt or die if auth is denied, so is_running,
            // started() and the elapsed timer fire only once the output
            // file starts growing. Early exit (auth denied, kms died,
            // portal cancelled) reports an error without ever starting.
            let mut promoted = false;
            let status = loop {
                thread::sleep(Duration::from_millis(200));
                if !promoted
                    && !stop.load(Ordering::SeqCst)
                    && std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0) > 0
                {
                    promoted = true;
                    let qt2 = qt_thread.clone();
                    let _ = qt_thread.queue(move |mut q| {
                        q.as_mut().set_is_running(true);
                        q.as_mut().set_elapsed(0);
                        q.as_mut().started();
                    });
                    rec_timer_start(qt2);
                }
                let mut guard = match child_arc.try_lock() {
                    Ok(g) => g,
                    Err(_) => continue,
                };
                match guard.try_wait() {
                    Ok(Some(s)) => break Ok(s),
                    Ok(None) => continue,
                    Err(e) => break Err(e),
                }
            };
            let err_buf_str = err_buf.lock().unwrap_or_else(|e| e.into_inner()).clone();

            match status {
                Ok(s) if s.success() => {
                    let done = QString::from(&out);
                    let _ = qt_thread.queue(move |mut q| {
                        *active().lock().unwrap_or_else(|e| e.into_inner()) = None;
                        rec_timer_stop();
                        q.as_mut().set_is_running(false);
                        q.as_mut().set_elapsed(0);
                        q.as_mut().finished(done);
                    });
                }
                Ok(s) => {
                    // NB: a stop-flagged but unsuccessful exit (e.g. SIGKILL after a
                    // timeout) means the file may be corrupt -> report error
                    // instead of finished.
                    if stop.load(Ordering::SeqCst) && s.success() {
                        let done = QString::from(&out);
                        let _ = qt_thread.queue(move |mut q| {
                            *active().lock().unwrap_or_else(|e| e.into_inner()) = None;
                            rec_timer_stop();
                            q.as_mut().set_is_running(false);
                            q.as_mut().set_elapsed(0);
                            q.as_mut().finished(done);
                        });
                    } else {
                        let detail = if err_buf_str.trim().is_empty() {
                            String::new()
                        } else {
                            format!(": {}", err_buf_str.trim())
                        };
                        let msg = QString::from(format!("{RECORDER_BIN} exited {s}{detail}"));
                        let _ = qt_thread.queue(move |mut q| {
                            *active().lock().unwrap_or_else(|e| e.into_inner()) = None;
                            rec_timer_stop();
                            q.as_mut().set_is_running(false);
                            q.as_mut().set_elapsed(0);
                            q.as_mut().error(msg);
                        });
                    }
                }
                Err(e) => {
                    let detail = if err_buf_str.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", err_buf_str.trim())
                    };
                    let msg = QString::from(format!("wait {RECORDER_BIN}: {e}{detail}"));
                    let _ = qt_thread.queue(move |mut q| {
                        *active().lock().unwrap_or_else(|e| e.into_inner()) = None;
                        rec_timer_stop();
                        q.as_mut().set_is_running(false);
                        q.as_mut().set_elapsed(0);
                        q.as_mut().error(msg);
                    });
                }
            }
        });
    }

    fn stop(self: Pin<&mut Self>) {
        // kill the elapsed timer immediately, reset display
        rec_timer_stop();
        let rec = active().lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(rec) = rec {
            rec.stop.store(true, Ordering::SeqCst);
            let pid = rec.pid;
            let child_clone = rec.child.clone();
            let qt_thread = self.qt_thread();
            // Don't block Qt thread — stop async and let wait-thread finish file.
            // SIGINT finalizes a valid mp4; wait up to 15s before SIGKILL
            // (which may truncate the file).
            thread::spawn(move || {
                if pid != 0 {
                    let _ = Command::new("kill")
                        .arg("-INT")
                        .arg(pid.to_string())
                        .status();
                    let mut exited = false;
                    for _ in 0..150 {
                        thread::sleep(Duration::from_millis(100));
                        if let Ok(mut ch) = child_clone.try_lock() {
                            if let Ok(Some(_)) = ch.try_wait() {
                                exited = true;
                                break;
                            }
                        }
                        if !is_pid_alive(pid) {
                            exited = true;
                            break;
                        }
                    }
                    if !exited {
                        eprintln!(
                            "stop: pid {pid} did not exit 15s after INT, killing (file may be corrupt)"
                        );
                        if let Ok(mut ch) = child_clone.try_lock() {
                            if let Ok(None) = ch.try_wait() {
                                let _ = ch.kill();
                                let _ = ch.wait();
                            }
                        }
                    }
                }
            });
            let _ = qt_thread.queue(move |mut q| {
                q.as_mut().set_is_running(false);
                q.as_mut().set_elapsed(0);
            });
        } else {
            let qt_thread = self.qt_thread();
            let _ = qt_thread.queue(move |mut q| {
                q.as_mut().set_is_running(false);
                q.as_mut().set_elapsed(0);
            });
        }
    }

    /// Force-kill any running recorder: normal recording and/or replay daemon.
    /// INT first (keeps mp4 valid when the process cooperates), KILL fallback.
    /// Clears all tracked state and pid file. Saved replay files in Videos are
    /// kept. Output of a killed recording may be corrupt - prefer stop()/clip()
    /// for files you keep.
    fn pkill(self: Pin<&mut Self>) {
        if CLIP_BUSY.load(Ordering::SeqCst) {
            let qt = self.qt_thread();
            let msg = QString::from("clip in progress, wait before pkill");
            let _ = qt.queue(move |mut q| {
                q.as_mut().error(msg);
            });
            return;
        }
        let qt_thread = self.qt_thread();
        rec_timer_stop();

        // normal recording, if any (its wait-thread reports finished/error)
        if let Some(rec) = active().lock().unwrap_or_else(|e| e.into_inner()).take() {
            rec.stop.store(true, Ordering::SeqCst);
            let pid = rec.pid;
            let child = rec.child;
            thread::spawn(move || {
                kill_pid_graceful(pid, Some(&child), 50);
            });
        }

        // replay daemon, if any (or orphan via pid file)
        let replay = replay_active()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .or_else(|| {
                read_pid_file().and_then(|(pid, dir)| {
                    if !dir.as_os_str().is_empty() && is_pid_alive(pid) {
                        Some(ReplayRecording {
                            child: None,
                            pid,
                            dir,
                        })
                    } else {
                        cleanup_stale_pid_file();
                        None
                    }
                })
            });
        if let Some(rec) = replay {
            let pid = rec.pid;
            let child = rec.child;
            clear_pid_file_if_matches(pid);
            thread::spawn(move || {
                kill_pid_graceful(pid, child.as_ref(), 50);
                if let Some(ch) = child {
                    if let Ok(mut guard) = ch.try_lock() {
                        let _ = guard.kill();
                        let _ = guard.wait();
                    }
                }
                eprintln!("pkill: stopped replay pid={pid}");
            });
        }

        // last resort for untracked strays, after graceful attempts had a chance
        thread::spawn(|| {
            thread::sleep(Duration::from_secs(6));
            let _ = Command::new("pkill")
                .args(["-KILL", "-f", "gpu-screen-recorder"])
                .status();
            cleanup_stale_pid_file();
        });

        let _ = qt_thread.queue(move |mut q| {
            q.as_mut().set_is_running(false);
            q.as_mut().set_elapsed(0);
            q.as_mut().error(QString::from("recorder processes killed"));
        });
    }

    fn clip(self: Pin<&mut Self>) {
        let qt_thread = self.qt_thread();

        if CLIP_BUSY.swap(true, Ordering::SeqCst) {
            let msg = QString::from("clip already in progress");
            let _ = qt_thread.queue(move |mut q| {
                q.as_mut().error(msg);
            });
            return;
        }
        struct BusyGuard;
        impl Drop for BusyGuard {
            fn drop(&mut self) {
                CLIP_BUSY.store(false, Ordering::SeqCst);
            }
        }

        // Take ownership so the exit-watcher can't race us; reinserted below
        // since the daemon stays alive across clips.
        let taken: Option<ReplayRecording> = replay_active()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let taken = taken.or_else(|| {
            read_pid_file().and_then(|(pid, dir)| {
                if !dir.as_os_str().is_empty() && is_pid_alive(pid) {
                    Some(ReplayRecording {
                        child: None,
                        pid,
                        dir,
                    })
                } else {
                    cleanup_stale_pid_file();
                    None
                }
            })
        });

        thread::spawn(move || {
            let _busy = BusyGuard;
            // Daemon stays alive across clips: hand state back on every path
            // once we know the pid is (or was) valid.
            let reinsert = |rec: ReplayRecording| {
                *replay_active().lock().unwrap_or_else(|e| e.into_inner()) = Some(rec);
            };
            let Some(rec) = taken else {
                let msg = QString::from("clip failed: replay not running (set replay:true first)");
                let _ = qt_thread.queue(move |mut q| {
                    q.as_mut().error(msg);
                });
                return;
            };
            let pid = rec.pid;
            let out_dir = rec.dir.clone();

            // Dead daemon -> clear stale state so replay can be restarted.
            if !is_pid_alive(pid) {
                cleanup_stale_pid_file();
                let msg = QString::from(
                    "clip failed: recorder exited, replay stopped - set replay:true to restart",
                );
                let _ = qt_thread.queue(move |mut q| {
                    q.as_mut().error(msg);
                });
                return;
            }

            let since = SystemTime::now();
            eprintln!("clip: kill -USR1 {pid} (out {})", out_dir.display());
            if !signal_save_replay(pid) {
                reinsert(rec);
                let msg = QString::from(format!("clip failed: could not signal pid {pid}"));
                let _ = qt_thread.queue(move |mut q| {
                    q.as_mut().error(msg);
                });
                return;
            }

            // Give the daemon a moment to flush the file, then poll for it.
            thread::sleep(Duration::from_millis(500));
            let t0 = Instant::now();
            let mut saved: Option<PathBuf> = None;
            while t0.elapsed() < Duration::from_secs(20) {
                if let Some(p) = find_saved_replay(&out_dir, since) {
                    // wait for the write to settle (size stable across 500ms)
                    let s1 = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                    thread::sleep(Duration::from_millis(500));
                    let s2 = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                    if s1 > 0 && s1 == s2 {
                        saved = Some(p);
                        break;
                    }
                } else {
                    thread::sleep(Duration::from_millis(500));
                }
                if !is_pid_alive(pid) {
                    break;
                }
            }

            // daemon keeps running regardless of outcome
            reinsert(rec);

            match saved {
                Some(path) => {
                    eprintln!("clip: saved {}", path.display());
                    let done = QString::from(path.to_string_lossy().to_string());
                    let _ = qt_thread.queue(move |mut q| {
                        q.as_mut().clipped(done);
                    });
                }
                None => {
                    let msg = QString::from(
                        "clip failed: recorder did not save a file (check daemon log)",
                    );
                    let _ = qt_thread.queue(move |mut q| {
                        q.as_mut().error(msg);
                    });
                }
            }
        });
    }
}
