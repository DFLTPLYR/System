//! Wayland color control via the `wl-gammarelay-rs` / `wl-gammarelay`
//! daemon over session DBus (`rs.wl-gammarelay / rs.wl.gammarelay`).
//!
//! QML API (`System.Gamma` singleton):
//! - properties: `temperature` (K), `brightness` (0.1..1.0),
//!   `gamma` (0.5..2.0), each writable (`Gamma.temperature = 5000`)
//! - per channel: `increaseTemperature()` / `decreaseTemperature()` /
//!   `setTemperature(v)`, same for `Brightness` / `Gamma`
//! - legacy shorthands: `increase()` / `decrease()` (= temperature)
//! - `refresh()`, `available`

use std::pin::Pin;
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use cxx_qt::{CxxQtType, Threading};

#[cxx_qt::bridge]
mod gamma {
    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(i32, temperature, READ, WRITE = set_temperature, NOTIFY = temperature_changed)]
        #[qproperty(f64, brightness, READ, WRITE = set_brightness, NOTIFY = brightness_changed)]
        #[qproperty(f64, gamma, READ, WRITE = set_gamma, NOTIFY = gamma_changed)]
        #[qproperty(i32, step)]
        #[qproperty(f64, brightness_step)]
        #[qproperty(f64, gamma_step)]
        #[qproperty(i32, min_temperature)]
        #[qproperty(i32, max_temperature)]
        #[qproperty(f64, min_brightness)]
        #[qproperty(f64, max_brightness)]
        #[qproperty(f64, min_gamma)]
        #[qproperty(f64, max_gamma)]
        #[qproperty(bool, available)]
        type Gamma = super::GammaRust;

        #[qinvokable]
        fn increase(self: Pin<&mut Self>);

        #[qinvokable]
        fn decrease(self: Pin<&mut Self>);

        #[qinvokable]
        fn increase_temperature(self: Pin<&mut Self>);

        #[qinvokable]
        fn decrease_temperature(self: Pin<&mut Self>);

        #[qinvokable]
        fn increase_brightness(self: Pin<&mut Self>);

        #[qinvokable]
        fn decrease_brightness(self: Pin<&mut Self>);

        #[qinvokable]
        fn increase_gamma(self: Pin<&mut Self>);

        #[qinvokable]
        fn decrease_gamma(self: Pin<&mut Self>);

        #[qinvokable]
        fn set_temperature(self: Pin<&mut Self>, value: i32);

        #[qinvokable]
        fn set_brightness(self: Pin<&mut Self>, value: f64);

        #[qinvokable]
        fn set_gamma(self: Pin<&mut Self>, value: f64);

        #[qinvokable]
        fn refresh(self: Pin<&mut Self>);

        #[qsignal]
        fn temperature_changed(self: Pin<&mut Self>);

        #[qsignal]
        fn brightness_changed(self: Pin<&mut Self>);

        #[qsignal]
        fn gamma_changed(self: Pin<&mut Self>);
    }

    impl cxx_qt::Constructor<()> for Gamma {}
    impl cxx_qt::Threading for Gamma {}
}

pub struct GammaRust {
    pub temperature: i32,
    pub brightness: f64,
    pub gamma: f64,
    pub step: i32,
    pub brightness_step: f64,
    pub gamma_step: f64,
    pub min_temperature: i32,
    pub max_temperature: i32,
    pub min_brightness: f64,
    pub max_brightness: f64,
    pub min_gamma: f64,
    pub max_gamma: f64,
    pub available: bool,
    pub running: Arc<AtomicBool>,
}

impl Default for GammaRust {
    fn default() -> Self {
        Self {
            temperature: DEFAULT_TEMPERATURE,
            brightness: DEFAULT_BRIGHTNESS,
            gamma: DEFAULT_GAMMA,
            step: DEFAULT_STEP,
            brightness_step: DEFAULT_BRIGHTNESS_STEP,
            gamma_step: DEFAULT_GAMMA_STEP,
            min_temperature: MIN_TEMPERATURE,
            max_temperature: MAX_TEMPERATURE,
            min_brightness: MIN_BRIGHTNESS,
            max_brightness: MAX_BRIGHTNESS,
            min_gamma: MIN_GAMMA,
            max_gamma: MAX_GAMMA,
            available: false,
            running: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl Drop for GammaRust {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

const BUS: &str = "rs.wl-gammarelay";
const PATH: &str = "/";
const IFACE: &str = "rs.wl.gammarelay";
const DEFAULT_TEMPERATURE: i32 = 6500;
const MIN_TEMPERATURE: i32 = 1000;
const MAX_TEMPERATURE: i32 = 10000;
const DEFAULT_STEP: i32 = 100;
const DEFAULT_BRIGHTNESS: f64 = 1.0;
const MIN_BRIGHTNESS: f64 = 0.1;
const MAX_BRIGHTNESS: f64 = 1.0;
const DEFAULT_BRIGHTNESS_STEP: f64 = 0.05;
const DEFAULT_GAMMA: f64 = 1.0;
const MIN_GAMMA: f64 = 0.5;
const MAX_GAMMA: f64 = 2.0;
const DEFAULT_GAMMA_STEP: f64 = 0.05;

/// Read a live DBus property via
/// `busctl --user -- get-property rs.wl-gammarelay / rs.wl.gammarelay <name>`
/// (prints e.g. `q 6500` / `d 1`). None when the daemon/busctl is missing.
///
/// NOTE: the `--` after `--user` is load-bearing: without it, negative
/// values (e.g. `UpdateTemperature n -100`) are misparsed as CLI flags
/// (`busctl: unrecognized option '-1'`).
fn query_property(name: &str) -> Option<String> {
    let out = Command::new("busctl")
        .args(["--user", "--", "get-property", BUS, PATH, IFACE, name])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace().last().map(|s| s.to_string())
}

fn query_temperature() -> Option<i32> {
    query_property("Temperature")?.parse::<i32>().ok()
}

fn query_brightness() -> Option<f64> {
    query_property("Brightness")?.parse::<f64>().ok()
}

fn query_gamma() -> Option<f64> {
    query_property("Gamma")?.parse::<f64>().ok()
}

fn daemon_available() -> bool {
    // get-property doubles as the presence probe; a second introspect call
    // would just add latency to every poll tick.
    query_temperature().is_some()
}

/// True if the daemon binary is already a running OS process (even if it
/// hasn't owned the DBus name yet). Uses `pgrep -x` so we don't spawn a
/// duplicate while the first instance is still starting up.
fn is_process_running(name: &str) -> bool {
    Command::new("pgrep")
        .args(["-x", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn is_daemon_process_running() -> bool {
    is_process_running("wl-gammarelay-rs") || is_process_running("wl-gammarelay")
}

fn call_update_temperature(delta: i32) {
    let delta = delta.clamp(i16::MIN as i32, i16::MAX as i32);
    let _ = Command::new("busctl")
        .args([
            "--user",
            "--",
            "call",
            BUS,
            PATH,
            IFACE,
            "UpdateTemperature",
            "n",
            &delta.to_string(),
        ])
        .status();
}

fn call_set_temperature(value: i32) {
    let value = value.clamp(MIN_TEMPERATURE, MAX_TEMPERATURE);
    let _ = Command::new("busctl")
        .args([
            "--user",
            "--",
            "set-property",
            BUS,
            PATH,
            IFACE,
            "Temperature",
            "q",
            &value.to_string(),
        ])
        .status();
}

fn call_update_brightness(delta: f64) {
    let _ = Command::new("busctl")
        .args([
            "--user",
            "--",
            "call",
            BUS,
            PATH,
            IFACE,
            "UpdateBrightness",
            "d",
            &delta.to_string(),
        ])
        .status();
}

fn call_set_brightness(value: f64) {
    let value = value.clamp(MIN_BRIGHTNESS, MAX_BRIGHTNESS);
    let _ = Command::new("busctl")
        .args([
            "--user",
            "--",
            "set-property",
            BUS,
            PATH,
            IFACE,
            "Brightness",
            "d",
            &value.to_string(),
        ])
        .status();
}

fn call_update_gamma(delta: f64) {
    let _ = Command::new("busctl")
        .args([
            "--user",
            "--",
            "call",
            BUS,
            PATH,
            IFACE,
            "UpdateGamma",
            "d",
            &delta.to_string(),
        ])
        .status();
}

fn call_set_gamma(value: f64) {
    let value = value.clamp(MIN_GAMMA, MAX_GAMMA);
    let _ = Command::new("busctl")
        .args([
            "--user",
            "--",
            "set-property",
            BUS,
            PATH,
            IFACE,
            "Gamma",
            "d",
            &value.to_string(),
        ])
        .status();
}

/// Best-effort autostart of the gamma daemon so `increase()` works on a fresh
/// session. Tries `wl-gammarelay-rs` first, then the Go `wl-gammarelay`.
/// Both expose the same `rs.wl-gammarelay` DBus interface above.
fn ensure_daemon() {
    if daemon_available() {
        return;
    }
    // Process is alive but hasn't owned the bus name yet: wait for it
    // instead of spawning a second copy.
    if is_daemon_process_running() {
        for _ in 0..10 {
            thread::sleep(Duration::from_millis(200));
            if daemon_available() {
                return;
            }
        }
        eprintln!("gamma: daemon process running but DBus name {BUS} never appeared");
        return;
    }
    for bin in ["wl-gammarelay-rs", "wl-gammarelay"] {
        // Skip binaries not in PATH so we prefer whichever is installed.
        if Command::new("sh")
            .args(["-c", &format!("command -v {bin} >/dev/null 2>&1")])
            .status()
            .map(|s| !s.success())
            .unwrap_or(true)
        {
            continue;
        }
        let spawned = Command::new(bin)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if spawned.is_ok() {
            // Give the daemon a moment to own the bus name.
            for _ in 0..10 {
                thread::sleep(Duration::from_millis(200));
                if daemon_available() {
                    eprintln!("gamma: started {bin} daemon");
                    return;
                }
            }
            // Binary ran but never owned the bus; don't stack a second
            // daemon on top of it.
            eprintln!("gamma: {bin} started but DBus name {BUS} never appeared");
            return;
        }
    }
    eprintln!("gamma: no wl-gammarelay daemon running and neither wl-gammarelay-rs nor wl-gammarelay found in PATH");
}

/// Re-read the daemon state and push it to the Qt thread.
fn poll_once(qt_thread: &cxx_qt::CxxQtThread<gamma::Gamma>) {
    let temp = query_temperature();
    let bright = query_brightness();
    let gam = query_gamma();
    let _ = qt_thread.queue(move |mut q| {
        if temp.is_none() && bright.is_none() && gam.is_none() {
            if *q.available() {
                q.as_mut().set_available(false);
            }
            return;
        }
        if !*q.available() {
            q.as_mut().set_available(true);
        }
        if let Some(t) = temp {
            if *q.temperature() != t {
                q.as_mut().rust_mut().temperature = t;
                q.as_mut().temperature_changed();
            }
        }
        if let Some(b) = bright {
            if (*q.brightness() - b).abs() > f64::EPSILON {
                q.as_mut().rust_mut().brightness = b;
                q.as_mut().brightness_changed();
            }
        }
        if let Some(g) = gam {
            if (*q.gamma() - g).abs() > f64::EPSILON {
                q.as_mut().rust_mut().gamma = g;
                q.as_mut().gamma_changed();
            }
        }
    });
}

impl cxx_qt::Initialize for gamma::Gamma {
    fn initialize(self: Pin<&mut Self>) {
        let qt_thread = self.qt_thread();
        let running = self.rust().running.clone();

        thread::spawn(move || {
            ensure_daemon();
            poll_once(&qt_thread);
            while running.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_secs(1));
                if !running.load(Ordering::SeqCst) {
                    break;
                }
                poll_once(&qt_thread);
            }
        });
    }
}

impl gamma::Gamma {
    /// `Gamma.setTemperature(5000)` / `Gamma.temperature = 5000`: applies
    /// immediately to the daemon (clamped to [min_temperature,
    /// max_temperature]); the poll loop reconciles afterwards.
    pub fn set_temperature(mut self: Pin<&mut Self>, value: i32) {
        let lo = *self.min_temperature();
        let hi = *self.max_temperature();
        let clamped = value.clamp(lo.min(hi), lo.max(hi));
        // Apply even when equal: the daemon may have drifted while the cache
        // stayed the same.
        self.as_mut().rust_mut().temperature = clamped;
        self.as_mut().temperature_changed();
        let qt_thread = self.qt_thread();
        thread::spawn(move || {
            ensure_daemon();
            call_set_temperature(clamped);
            poll_once(&qt_thread);
        });
    }

    /// `Gamma.setBrightness(0.8)` / `Gamma.brightness = 0.8` (clamped to
    /// [min_brightness, max_brightness]).
    pub fn set_brightness(mut self: Pin<&mut Self>, value: f64) {
        let lo = *self.min_brightness();
        let hi = *self.max_brightness();
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        let clamped = value.clamp(lo, hi);
        self.as_mut().rust_mut().brightness = clamped;
        self.as_mut().brightness_changed();
        let qt_thread = self.qt_thread();
        thread::spawn(move || {
            ensure_daemon();
            call_set_brightness(clamped);
            poll_once(&qt_thread);
        });
    }

    /// `Gamma.setGamma(1.0)` / `Gamma.gamma = 1.0` (clamped to [min_gamma,
    /// max_gamma]).
    pub fn set_gamma(mut self: Pin<&mut Self>, value: f64) {
        let lo = *self.min_gamma();
        let hi = *self.max_gamma();
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        let clamped = value.clamp(lo, hi);
        self.as_mut().rust_mut().gamma = clamped;
        self.as_mut().gamma_changed();
        let qt_thread = self.qt_thread();
        thread::spawn(move || {
            ensure_daemon();
            call_set_gamma(clamped);
            poll_once(&qt_thread);
        });
    }

    /// Legacy temperature shorthands (kept for backwards compatibility).
    fn increase(mut self: Pin<&mut Self>) {
        self.as_mut().increase_temperature();
    }

    fn decrease(mut self: Pin<&mut Self>) {
        self.as_mut().decrease_temperature();
    }

    fn increase_temperature(mut self: Pin<&mut Self>) {
        let step = *self.step();
        // Optimistic UI update so key repeats feel instant.
        let hi = *self.max_temperature();
        let next = (*self.temperature() + step).min(hi);
        self.as_mut().rust_mut().temperature = next;
        self.as_mut().temperature_changed();
        let qt_thread = self.qt_thread();
        thread::spawn(move || {
            ensure_daemon();
            call_update_temperature(step);
            poll_once(&qt_thread);
        });
    }

    fn decrease_temperature(mut self: Pin<&mut Self>) {
        let step = *self.step();
        let lo = *self.min_temperature();
        let next = (*self.temperature() - step).max(lo);
        self.as_mut().rust_mut().temperature = next;
        self.as_mut().temperature_changed();
        let qt_thread = self.qt_thread();
        thread::spawn(move || {
            ensure_daemon();
            call_update_temperature(-step);
            poll_once(&qt_thread);
        });
    }

    fn increase_brightness(mut self: Pin<&mut Self>) {
        let step = *self.brightness_step();
        let hi = *self.max_brightness();
        let next = (*self.brightness() + step).min(hi);
        self.as_mut().rust_mut().brightness = next;
        self.as_mut().brightness_changed();
        let qt_thread = self.qt_thread();
        thread::spawn(move || {
            ensure_daemon();
            call_update_brightness(step);
            poll_once(&qt_thread);
        });
    }

    fn decrease_brightness(mut self: Pin<&mut Self>) {
        let step = *self.brightness_step();
        let lo = *self.min_brightness();
        let next = (*self.brightness() - step).max(lo);
        self.as_mut().rust_mut().brightness = next;
        self.as_mut().brightness_changed();
        let qt_thread = self.qt_thread();
        thread::spawn(move || {
            ensure_daemon();
            call_update_brightness(-step);
            poll_once(&qt_thread);
        });
    }

    fn increase_gamma(mut self: Pin<&mut Self>) {
        let step = *self.gamma_step();
        let hi = *self.max_gamma();
        let next = (*self.gamma() + step).min(hi);
        self.as_mut().rust_mut().gamma = next;
        self.as_mut().gamma_changed();
        let qt_thread = self.qt_thread();
        thread::spawn(move || {
            ensure_daemon();
            call_update_gamma(step);
            poll_once(&qt_thread);
        });
    }

    fn decrease_gamma(mut self: Pin<&mut Self>) {
        let step = *self.gamma_step();
        let lo = *self.min_gamma();
        let next = (*self.gamma() - step).max(lo);
        self.as_mut().rust_mut().gamma = next;
        self.as_mut().gamma_changed();
        let qt_thread = self.qt_thread();
        thread::spawn(move || {
            ensure_daemon();
            call_update_gamma(-step);
            poll_once(&qt_thread);
        });
    }

    /// Re-read the daemon state immediately.
    fn refresh(self: Pin<&mut Self>) {
        let qt_thread = self.qt_thread();
        thread::spawn(move || {
            poll_once(&qt_thread);
        });
    }
}
