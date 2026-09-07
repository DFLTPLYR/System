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

use cxx_qt::Threading;
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
        type ScreenRec = super::ScreenRecord;

        #[qinvokable]
        fn start(self: Pin<&mut Self>, path: &QString);

        #[qinvokable]
        fn stop(self: Pin<&mut Self>);

        #[qsignal]
        fn finished(self: Pin<&mut Self>, path: QString);

        #[qsignal]
        fn error(self: Pin<&mut Self>, message: QString);
    }

    impl cxx_qt::Constructor<()> for ScreenRec {}
    impl cxx_qt::Threading for ScreenRec {}
}

pub struct ScreenRecord {
    pub is_running: bool,
    pub audio: bool,
    pub display: bool,
}

impl Default for ScreenRecord {
    fn default() -> Self {
        Self {
            is_running: false,
            audio: true,
            display: true,
        }
    }
}

impl cxx_qt::Initialize for screen_record::ScreenRec {
    fn initialize(self: Pin<&mut Self>) {}
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
    // PipeWire without pactl: try wpctl status -> Audio/Sink -> .monitor
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

impl screen_record::ScreenRec {
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
        let qt_thread = self.qt_thread();
        let audio = *self.audio();

        // Overwrite existing file like ffmpeg -y
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
            let (status, mut err_buf) = {
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
                    // If stop was requested, treat as normal finish (user stopped)
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
}
