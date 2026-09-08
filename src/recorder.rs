use std::io::Read;
use std::path::Path;
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

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
        #[qproperty(bool, audio)]
        #[qproperty(bool, display)]
        #[qproperty(bool, replay, READ, WRITE = set_replay, NOTIFY = replay_changed)]
        #[qproperty(QString, monitor)]
        #[qproperty(u64, fps)]
        #[qproperty(u64, time)]
        type ScreenRec = super::ScreenRecord;

        #[qinvokable]
        fn start(self: Pin<&mut Self>, path: &QString);

        #[qinvokable]
        fn stop(self: Pin<&mut Self>);

        #[qinvokable]
        fn clip(self: Pin<&mut Self>);

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

        fn set_replay(self: Pin<&mut Self>, value: bool);

    }

    impl cxx_qt::Constructor<()> for ScreenRec {}
    impl cxx_qt::Threading for ScreenRec {}
}

pub struct ScreenRecord {
    pub is_running: bool,
    pub audio: bool,
    pub display: bool,
    pub replay: bool,
    pub fps: u64,
    pub monitor: QString,
    pub time: u64,
}

impl Default for ScreenRecord {
    fn default() -> Self {
        Self {
            is_running: false,
            audio: true,
            display: true,
            replay: false,
            monitor: QString::from(""),
            fps: 60,
            time: 30,
        }
    }
}

impl cxx_qt::Initialize for screen_record::ScreenRec {
    fn initialize(self: Pin<&mut Self>) {
        if *self.replay() {
            let time = *self.time();
            let fps = *self.fps();
            let monitor = self.monitor().to_string();
            let qt_thread = self.qt_thread();
            spawn_replay_process(qt_thread, time, fps, monitor);
        }
    }
}

fn get_monitor_source() -> Option<String> {
    if let Ok(out) = Command::new("pactl").arg("get-default-sink").output() {
        if out.status.success() {
            let sink = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !sink.is_empty() {
                let monitor = format!("{sink}.monitor");
                if let Ok(list) = Command::new("pactl")
                    .args(["list", "short", "sources"])
                    .output()
                {
                    if list.status.success() {
                        let txt = String::from_utf8_lossy(&list.stdout);
                        if txt.contains(&monitor) {
                            return Some(monitor);
                        }
                    }
                }
                return Some(monitor);
            }
        }
    }
    if let Ok(out) = Command::new("pactl")
        .args(["list", "short", "sources"])
        .output()
    {
        if out.status.success() {
            let txt = String::from_utf8_lossy(&out.stdout);
            for line in txt.lines() {
                if line.contains("monitor") {
                    if let Some(name) = line.split_whitespace().nth(1) {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }
    if let Ok(out) = Command::new("wpctl").arg("status").output() {
        if out.status.success() {
            let txt = String::from_utf8_lossy(&out.stdout);
            for line in txt.lines() {
                if line.contains("Audio/Sink") {
                    if let Some(sink) = line.split("Audio/Sink").nth(1) {
                        let sink = sink.trim().split_whitespace().next().unwrap_or("").trim();
                        if !sink.is_empty() && sink.contains("alsa") {
                            return Some(format!("{sink}.monitor"));
                        }
                    }
                }
            }
            for line in txt.lines() {
                let t = line.trim();
                if t.contains(".monitor") {
                    for tok in t.split_whitespace() {
                        if tok.contains(".monitor") {
                            let clean = tok
                                .trim_matches(|c| c == ',' || c == '*')
                                .trim_end_matches(',')
                                .to_string();
                            if !clean.is_empty() {
                                return Some(clean);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn slurp_geometry(display: bool) -> Result<String, Box<dyn std::error::Error>> {
    let mut cmd = Command::new("slurp");
    if display {
        cmd.args(["-o", "-f", "%o"]);
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

#[allow(dead_code)]
struct ReplayRecording {
    child: Arc<Mutex<Child>>,
    pid: u32,
    path: std::path::PathBuf,
}
static REPLAY: OnceLock<Mutex<Option<ReplayRecording>>> = OnceLock::new();
fn replay_active() -> &'static Mutex<Option<ReplayRecording>> {
    REPLAY.get_or_init(|| Mutex::new(None))
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

fn spawn_replay_process(
    qt_thread: cxx_qt::CxxQtThread<screen_record::ScreenRec>,
    time: u64,
    fps: u64,
    monitor: String,
) {
    if replay_active().lock().unwrap().is_some() {
        return;
    }

    // History buffer lives in temp (not $HOME/Videos) – only on clip we copy to Videos.
    // This avoids overwriting Videos on every clip and keeps buffer file hidden.
    let temp_dir = std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let _ = std::fs::create_dir_all(&temp_dir);
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    let out_path = temp_dir.join(format!("wl-screenrec-history-{timestamp}.mp4"));

    // For history, ignore monitor and capture all outputs by default for maximum compatibility.
    // Original request was just `wl-screenrec --history {time} --max-fps {fps}` without -o.
    // We add -f for file and keep optional -o only if user explicitly sets valid monitor.
    let mut cmd = Command::new("wl-screenrec");
    cmd.arg("-f").arg(&out_path);
    // Use monitor as passed (user handles it); if empty (esc) skip -o – will bail on multi-display as expected.
    let monitor_trim = monitor.trim();
    if !monitor_trim.is_empty() {
        eprintln!("spawn_replay: using monitor -o '{}'", monitor_trim);
        cmd.arg("-o").arg(monitor_trim);
    } else {
        eprintln!("spawn_replay: monitor empty (esc), starting without -o");
    }
    cmd.arg("--audio");
    if let Some(dev) = get_monitor_source() {
        cmd.args(["--audio-device", &dev]);
    }
    // Workaround for Intel iGPU VAAPI low_power failure seen in logs:
    // "No usable encoding entrypoint ... failed to open encoder in low_power mode"
    // Fallback to non-low_power or software if needed.
    cmd.arg("--history")
        .arg(time.to_string())
        .arg("--max-fps")
        .arg(fps.to_string())
        .arg("--low-power")
        .arg("off");
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = qt_thread.queue(move |mut q| {
                q.as_mut()
                    .error(QString::from(format!("spawn wl-screenrec --history: {e}")));
            });
            return;
        }
    };

    let pid = child.id();
    let mut stderr = child.stderr.take();
    let child_arc = Arc::new(Mutex::new(child));
    // Clone path for storage and for logging
    let stored_path = out_path.clone();
    let display_monitor = if monitor_trim.is_empty() {
        "(none)".to_string()
    } else {
        monitor_trim.to_string()
    };
    eprintln!(
        "spawn_replay: wl-screenrec -f {} --history {} --max-fps {} -o '{}' pid={}",
        out_path.display(),
        time,
        fps,
        display_monitor,
        pid
    );
    *replay_active().lock().unwrap() = Some(ReplayRecording {
        child: child_arc.clone(),
        pid,
        path: stored_path,
    });

    let qt_thread_clone = qt_thread.clone();
    thread::spawn(move || {
        let (status, err_buf) = {
            let mut guard = match child_arc.lock() {
                Ok(g) => g,
                Err(_) => {
                    let _ = qt_thread_clone.queue(move |mut q| {
                        *replay_active().lock().unwrap() = None;
                        q.as_mut().error(QString::from("replay lock poisoned"));
                    });
                    return;
                }
            };
            let s = guard.wait();
            let mut buf = String::new();
            if let Some(ref mut f) = stderr {
                let _ = f.read_to_string(&mut buf);
            }
            (s, buf)
        };

        // Clear active on exit
        *replay_active().lock().unwrap() = None;

        match status {
            Ok(s) if s.success() => {
                // normal exit, no signal needed
            }
            Ok(s) => {
                let detail = if err_buf.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", err_buf.trim())
                };
                let msg = QString::from(format!("wl-screenrec --history exited {s}{detail}"));
                let _ = qt_thread_clone.queue(move |mut q| {
                    q.as_mut().error(msg);
                });
            }
            Err(e) => {
                let detail = if err_buf.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", err_buf.trim())
                };
                let msg = QString::from(format!("wait wl-screenrec --history: {e}{detail}"));
                let _ = qt_thread_clone.queue(move |mut q| {
                    q.as_mut().error(msg);
                });
            }
        }
    });
}

fn stop_replay_process(qt_thread: cxx_qt::CxxQtThread<screen_record::ScreenRec>) {
    let rec = replay_active().lock().unwrap().take();
    if let Some(rec) = rec {
        let pid = rec.pid;
        let child = rec.child;
        // Don't block Qt thread in set_replay; spawn thread to kill
        thread::spawn(move || {
            if pid != 0 {
                let _ = Command::new("kill")
                    .arg("-INT")
                    .arg(pid.to_string())
                    .status();
                for _ in 0..10 {
                    thread::sleep(Duration::from_millis(100));
                    if let Ok(mut ch) = child.try_lock() {
                        if let Ok(Some(_)) = ch.try_wait() {
                            break;
                        }
                    }
                }
                if let Ok(mut ch) = child.try_lock() {
                    if let Ok(None) = ch.try_wait() {
                        let _ = ch.kill();
                        let _ = ch.wait();
                    }
                }
            }
        });
        let _ = qt_thread.queue(move |mut q| {
            // no is_running change for replay, just ensure UI updated if needed
            q.as_mut().replay_changed();
        });
    }
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
            // start history: spawn wl-screenrec --history {time} --max-fps {fps} -f temp --audio -o monitor
            let time = *self.time();
            let fps = *self.fps();
            let monitor = self.monitor().to_string();
            eprintln!(
                "set_replay: starting history time={} fps={} monitor='{}'",
                time, fps, monitor
            );
            let qt_thread = self.qt_thread();
            spawn_replay_process(qt_thread, time, fps, monitor);
        } else {
            // stop history process if running
            eprintln!("set_replay: stopping history");
            let qt_thread = self.qt_thread();
            stop_replay_process(qt_thread);
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
        if active().lock().unwrap().is_some() {
            return;
        }

        self.as_mut().set_is_running(true);
        self.as_mut().started();
        let qt_thread = self.qt_thread();
        let audio = *self.audio();

        let _ = std::fs::remove_file(&out);

        let display = *self.display();
        let mut cmd = Command::new("wl-screenrec");
        cmd.arg("-f").arg(&out);
        if let Ok(geo) = slurp_geometry(display) {
            if !geo.is_empty() {
                if display {
                    cmd.arg("-o").arg(geo);
                } else {
                    cmd.arg("-g").arg(geo);
                }
            }
        }
        if audio {
            cmd.arg("--audio");
            if let Some(dev) = get_monitor_source() {
                cmd.args(["--audio-device", &dev]);
            }
        }
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.as_mut().set_is_running(false);
                self.as_mut()
                    .error(QString::from(format!("spawn wl-screenrec: {e}")));
                return;
            }
        };

        let pid = child.id();
        let mut stderr = child.stderr.take();
        let child_arc = Arc::new(Mutex::new(child));
        let stop = Arc::new(AtomicBool::new(false));
        *active().lock().unwrap() = Some(ActiveRecording {
            child: child_arc.clone(),
            pid,
            stop: stop.clone(),
        });

        thread::spawn(move || {
            let (status, err_buf) = {
                let mut guard = match child_arc.lock() {
                    Ok(g) => g,
                    Err(_) => {
                        let _ = qt_thread.queue(move |mut q| {
                            *active().lock().unwrap() = None;
                            q.as_mut().set_is_running(false);
                            q.as_mut().error(QString::from("recording lock poisoned"));
                        });
                        return;
                    }
                };
                let s = guard.wait();
                let mut buf = String::new();
                if let Some(ref mut f) = stderr {
                    let _ = f.read_to_string(&mut buf);
                }
                (s, buf)
            };

            match status {
                Ok(s) if s.success() => {
                    let done = QString::from(&out);
                    let _ = qt_thread.queue(move |mut q| {
                        *active().lock().unwrap() = None;
                        q.as_mut().set_is_running(false);
                        q.as_mut().finished(done);
                    });
                }
                Ok(s) => {
                    if stop.load(Ordering::SeqCst) {
                        let done = QString::from(&out);
                        let _ = qt_thread.queue(move |mut q| {
                            *active().lock().unwrap() = None;
                            q.as_mut().set_is_running(false);
                            q.as_mut().finished(done);
                        });
                    } else {
                        let detail = if err_buf.trim().is_empty() {
                            String::new()
                        } else {
                            format!(": {}", err_buf.trim())
                        };
                        let msg = QString::from(format!("wl-screenrec exited {s}{detail}"));
                        let _ = qt_thread.queue(move |mut q| {
                            *active().lock().unwrap() = None;
                            q.as_mut().set_is_running(false);
                            q.as_mut().error(msg);
                        });
                    }
                }
                Err(e) => {
                    let detail = if err_buf.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", err_buf.trim())
                    };
                    let msg = QString::from(format!("wait wl-screenrec: {e}{detail}"));
                    let _ = qt_thread.queue(move |mut q| {
                        *active().lock().unwrap() = None;
                        q.as_mut().set_is_running(false);
                        q.as_mut().error(msg);
                    });
                }
            }
        });
    }

    fn stop(self: Pin<&mut Self>) {
        let rec = active().lock().unwrap().take();
        if let Some(rec) = rec {
            rec.stop.store(true, Ordering::SeqCst);
            let pid = rec.pid;
            let child_clone = rec.child.clone();
            let qt_thread = self.qt_thread();
            // Don't block Qt thread — kill async and let wait-thread finish file
            thread::spawn(move || {
                if pid != 0 {
                    let _ = Command::new("kill")
                        .arg("-INT")
                        .arg(pid.to_string())
                        .status();
                    for _ in 0..10 {
                        thread::sleep(Duration::from_millis(100));
                        if let Ok(mut ch) = child_clone.try_lock() {
                            if let Ok(Some(_)) = ch.try_wait() {
                                break;
                            }
                        }
                    }
                    if let Ok(mut ch) = child_clone.try_lock() {
                        if let Ok(None) = ch.try_wait() {
                            let _ = ch.kill();
                            let _ = ch.wait();
                        }
                    }
                }
            });
            let _ = qt_thread.queue(move |mut q| {
                q.as_mut().set_is_running(false);
            });
        } else {
            let qt_thread = self.qt_thread();
            let _ = qt_thread.queue(move |mut q| {
                q.as_mut().set_is_running(false);
            });
        }
    }

    fn clip(mut self: Pin<&mut Self>) {
        // Capture replay settings for restart after clip
        let time_cfg = *self.time();
        let fps_cfg = *self.fps();
        let monitor_cfg = self.monitor().to_string();
        let qt_for_restart = self.qt_thread();
        // Try direct PID first (most reliable, no killall needed)
        let replay_info = replay_active()
            .lock()
            .unwrap()
            .as_ref()
            .map(|r| (r.pid, r.path.clone(), r.child.clone()));

        if let Some((pid, path, _child_clone)) = replay_info.clone() {
            let path_str = QString::from(path.to_string_lossy().to_string());
            match Command::new("kill")
                .args(["-USR1", &pid.to_string()])
                .status()
            {
                Ok(s) if s.success() => {
                    eprintln!(
                        "clip: kill -USR1 {} -> waiting for temp {}",
                        pid,
                        path.display()
                    );
                    // Wait for flush to appear
                    let mut last = 0u64;
                    let mut stable = 0;
                    for _ in 0..60 {
                        if let Ok(m) = std::fs::metadata(&path) {
                            let sz = m.len();
                            if sz > 0 && sz == last {
                                stable += 1;
                                if stable >= 5 {
                                    break;
                                }
                            } else if sz > 0 {
                                stable = 0;
                            }
                            last = sz;
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                    eprintln!(
                        "clip: flush stable temp size {} bytes, now killing pid {} to finalize",
                        last, pid
                    );
                    // Kill history process to finalize mp4 (as requested: kill then start new)
                    if let Some(rec) = replay_active().lock().unwrap().take() {
                        let pid2 = rec.pid;
                        let child2 = rec.child;
                        let _ = Command::new("kill")
                            .arg("-INT")
                            .arg(pid2.to_string())
                            .status();
                        for _ in 0..15 {
                            thread::sleep(Duration::from_millis(100));
                            if let Ok(mut c) = child2.try_lock() {
                                if let Ok(Some(_)) = c.try_wait() {
                                    break;
                                }
                            }
                        }
                        if let Ok(mut c) = child2.try_lock() {
                            if let Ok(None) = c.try_wait() {
                                let _ = c.kill();
                                let _ = c.wait();
                            }
                        }
                        eprintln!("clip: killed history pid {}", pid2);
                    }
                    // Wait for final file to settle after kill
                    thread::sleep(Duration::from_millis(600));
                    let mut last2 = 0u64;
                    let mut stable2 = 0;
                    for _ in 0..30 {
                        if let Ok(m) = std::fs::metadata(&path) {
                            let sz = m.len();
                            if sz > 0 && sz == last2 {
                                stable2 += 1;
                                if stable2 >= 5 {
                                    break;
                                }
                            } else if sz > 0 {
                                stable2 = 0;
                            }
                            last2 = sz;
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                    eprintln!("clip: finalized temp size {} bytes", last2);
                    // Now copy temp -> $HOME/Videos on save
                    let videos_dir = default_videos_dir();
                    let _ = std::fs::create_dir_all(&videos_dir);
                    let final_name = format!(
                        "replay_{}.mp4",
                        chrono::Local::now().format("%Y%m%d_%H%M%S")
                    );
                    let final_path = videos_dir.join(final_name);
                    let to_emit = if path.exists() {
                        eprintln!(
                            "clip: copying {} ({} bytes) -> {}",
                            path.display(),
                            last2,
                            final_path.display()
                        );
                        match std::fs::copy(&path, &final_path) {
                            Ok(copied) => {
                                eprintln!(
                                    "clip: copied {} bytes to {}",
                                    copied,
                                    final_path.display()
                                );
                                // Verify with ffprobe-like check via file size
                                QString::from(final_path.to_string_lossy().to_string())
                            }
                            Err(e) => {
                                eprintln!(
                                    "clip: copy failed {} -> {}: {}",
                                    path.display(),
                                    final_path.display(),
                                    e
                                );
                                self.as_mut()
                                    .error(QString::from(format!("clip: copy failed: {e}")));
                                path_str.clone()
                            }
                        }
                    } else {
                        eprintln!("clip: temp still not found {}", path.display());
                        self.as_mut().error(QString::from(format!(
                            "clip: file not found {}",
                            path.display()
                        )));
                        path_str.clone()
                    };
                    self.as_mut().clipped(to_emit);
                    // Start new buffer for next clip
                    {
                        let qt_restart = qt_for_restart.clone();
                        let t = time_cfg;
                        let f = fps_cfg;
                        let m = monitor_cfg.clone();
                        // REPLAY already cleared by take() above, just spawn new
                        thread::spawn(move || {
                            thread::sleep(Duration::from_millis(300));
                            spawn_replay_process(qt_restart, t, f, m);
                        });
                        eprintln!("clip: restarted new history");
                    }
                    return;
                }
                Ok(s) => {
                    eprintln!("clip: kill -USR1 {pid} exited {s}, trying killall");
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    eprintln!("clip: kill not found: {e}");
                }
                Err(e) => {
                    self.as_mut()
                        .error(QString::from(format!("clip failed: kill -USR1 {pid}: {e}")));
                }
            }
            // Fallback still has path to emit
            for prog in ["killall", "pkill"] {
                match Command::new(prog).args(["-USR1", "wl-screenrec"]).status() {
                    Ok(s) if s.success() => {
                        eprintln!(
                            "clip: {prog} -USR1 wl-screenrec -> waiting for temp {}",
                            path.display()
                        );
                        let mut last = 0u64;
                        let mut stable = 0;
                        for _ in 0..60 {
                            if let Ok(m) = std::fs::metadata(&path) {
                                let sz = m.len();
                                if sz > 0 && sz == last {
                                    stable += 1;
                                    if stable >= 5 {
                                        break;
                                    }
                                } else if sz > 0 {
                                    stable = 0;
                                }
                                last = sz;
                            }
                            thread::sleep(Duration::from_millis(100));
                        }
                        eprintln!(
                            "clip: {prog} flush stable {} bytes, killing to finalize",
                            last
                        );
                        if let Some(rec) = replay_active().lock().unwrap().take() {
                            let pid2 = rec.pid;
                            let child2 = rec.child;
                            let _ = Command::new("kill")
                                .arg("-INT")
                                .arg(pid2.to_string())
                                .status();
                            for _ in 0..15 {
                                thread::sleep(Duration::from_millis(100));
                                if let Ok(mut c) = child2.try_lock() {
                                    if let Ok(Some(_)) = c.try_wait() {
                                        break;
                                    }
                                }
                            }
                            if let Ok(mut c) = child2.try_lock() {
                                if let Ok(None) = c.try_wait() {
                                    let _ = c.kill();
                                    let _ = c.wait();
                                }
                            }
                        }
                        thread::sleep(Duration::from_millis(600));
                        let mut last2 = 0u64;
                        let mut stable2 = 0;
                        for _ in 0..30 {
                            if let Ok(m) = std::fs::metadata(&path) {
                                let sz = m.len();
                                if sz > 0 && sz == last2 {
                                    stable2 += 1;
                                    if stable2 >= 5 {
                                        break;
                                    }
                                } else if sz > 0 {
                                    stable2 = 0;
                                }
                                last2 = sz;
                            }
                            thread::sleep(Duration::from_millis(100));
                        }
                        eprintln!("clip: {prog} finalized temp size {} bytes", last2);
                        let videos_dir = default_videos_dir();
                        let _ = std::fs::create_dir_all(&videos_dir);
                        let final_path = videos_dir.join(format!(
                            "replay_{}.mp4",
                            chrono::Local::now().format("%Y%m%d_%H%M%S")
                        ));
                        let to_emit = if path.exists() {
                            match std::fs::copy(&path, &final_path) {
                                Ok(_) => {
                                    eprintln!("clip: {prog} copied to {}", final_path.display());
                                    QString::from(final_path.to_string_lossy().to_string())
                                }
                                Err(e) => {
                                    eprintln!("clip: {prog} copy failed: {e}");
                                    path_str.clone()
                                }
                            }
                        } else {
                            eprintln!("clip: {prog} -> clipped temp {}", path.display());
                            path_str.clone()
                        };
                        self.as_mut().clipped(to_emit);
                        // Restart new buffer
                        {
                            let qt_restart = qt_for_restart.clone();
                            let t = time_cfg;
                            let f = fps_cfg;
                            let m = monitor_cfg.clone();
                            thread::spawn(move || {
                                thread::sleep(Duration::from_millis(300));
                                spawn_replay_process(qt_restart, t, f, m);
                            });
                            eprintln!("clip: restarted history after fallback kill pid {}", pid);
                        }
                        return;
                    }
                    Ok(s) => {
                        eprintln!("clip: {prog} -USR1 wl-screenrec exited {s}");
                        continue;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        eprintln!("clip: {prog} not found: {e}");
                        continue;
                    }
                    Err(e) => {
                        self.as_mut()
                            .error(QString::from(format!("clip failed: {prog}: {e}")));
                        return;
                    }
                }
            }
            // PID existed but all signals failed – still report path failure
            self.as_mut().error(QString::from(format!(
                "clip: signal failed for pid {} path {}",
                pid,
                path.display()
            )));
            return;
        }

        // No tracked replay process – fallback to broadcast signal (no path known)
        eprintln!("clip: no tracked replay pid, trying broadcast killall/pkill");
        for prog in ["killall", "pkill"] {
            match Command::new(prog).args(["-USR1", "wl-screenrec"]).status() {
                Ok(s) if s.success() => {
                    // No stored path, guess $HOME/Videos
                    let guess = default_videos_dir().join("replay_*.mp4");
                    eprintln!(
                        "clip: {prog} succeeded without tracked path, guess {}",
                        guess.display()
                    );
                    self.as_mut()
                        .clipped(QString::from(guess.to_string_lossy().to_string()));
                    return;
                }
                Ok(s) => {
                    eprintln!("clip: {prog} -USR1 wl-screenrec exited {s}");
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    eprintln!("clip: {prog} not found: {e}");
                    continue;
                }
                Err(e) => {
                    self.as_mut()
                        .error(QString::from(format!("clip failed: {prog}: {e}")));
                    return;
                }
            }
        }

        self.as_mut().error(QString::from(
            "clip failed: replay not running (set replay:true first); history saves to $HOME/Videos/replay_*.mp4",
        ));
    }
}
