use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use gfxinfo::active_gpu;
use sysinfo::{Disks, Networks, System};

#[cxx_qt::bridge]
mod system {
    extern "C++Qt" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, os)]
        #[qproperty(QString, kernel_version)]
        #[qproperty(QString, os_version)]
        #[qproperty(u64, uptime)]
        #[qproperty(u64, boot_time)]
        #[qproperty(QString, cpu_architecture)]
        #[qproperty(f64, cpu_usage)]
        #[qproperty(u64, cpu_frequency)]
        #[qproperty(u32, cpu_cores)]
        #[qproperty(u32, physical_cores)]
        #[qproperty(u64, memory_total)]
        #[qproperty(u64, memory_used)]
        #[qproperty(u64, memory_free)]
        #[qproperty(u64, memory_swap_total)]
        #[qproperty(u64, memory_swap_used)]
        #[qproperty(QString, gpu_vendor)]
        #[qproperty(QString, gpu_model)]
        #[qproperty(QString, gpu_family)]
        #[qproperty(u64, gpu_total_vram)]
        #[qproperty(u64, gpu_used_vram)]
        #[qproperty(u64, gpu_free_vram)]
        #[qproperty(f64, gpu_temperature)]
        #[qproperty(f64, gpu_utilization)]
        type System = super::SystemRust;

        #[qinvokable]
        fn init(self: Pin<&mut System>);
    }

    impl cxx_qt::Threading for System {}
}

pub struct SystemRust {
    pub running: Arc<AtomicBool>,
    pub initiated: bool,
    pub os: QString,
    pub kernel_version: QString,
    pub os_version: QString,
    pub uptime: u64,
    pub boot_time: u64,
    pub cpu_architecture: QString,
    pub cpu_usage: f64,
    pub cpu_frequency: u64,
    pub cpu_cores: u32,
    pub physical_cores: u32,
    pub memory_total: u64,
    pub memory_used: u64,
    pub memory_free: u64,
    pub memory_swap_total: u64,
    pub memory_swap_used: u64,
    pub gpu_vendor: QString,
    pub gpu_model: QString,
    pub gpu_family: QString,
    pub gpu_total_vram: u64,
    pub gpu_used_vram: u64,
    pub gpu_free_vram: u64,
    pub gpu_temperature: f64,
    pub gpu_utilization: f64,
}

impl Default for SystemRust {
    fn default() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(true)),
            initiated: false,
            os: QString::default(),
            kernel_version: QString::default(),
            os_version: QString::default(),
            uptime: 0,
            boot_time: 0,
            cpu_architecture: QString::default(),
            cpu_usage: 0.0,
            cpu_frequency: 0,
            cpu_cores: 0,
            physical_cores: 0,
            memory_total: 0,
            memory_used: 0,
            memory_free: 0,
            memory_swap_total: 0,
            memory_swap_used: 0,
            gpu_vendor: QString::default(),
            gpu_model: QString::default(),
            gpu_family: QString::default(),
            gpu_total_vram: 0,
            gpu_used_vram: 0,
            gpu_free_vram: 0,
            gpu_temperature: 0.0,
            gpu_utilization: 0.0,
        }
    }
}

impl Drop for SystemRust {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

impl system::System {
    fn init(mut self: Pin<&mut Self>) {
        if self.rust().initiated {
            return;
        }
        self.as_mut().rust_mut().initiated = true;

        let qt_thread = self.qt_thread();
        let running = self.rust().running.clone();

        thread::spawn(move || {
            let mut _disks = Disks::new_with_refreshed_list();
            let mut _networks = Networks::new_with_refreshed_list();
            let mut sys = System::new_all();

            while running.load(Ordering::SeqCst) {
                sys.refresh_all();
                _disks.refresh(true);
                _networks.refresh(true);

                let os = System::name();
                let kernel = System::kernel_version();
                let os_ver = System::os_version();

                let cpu_usage = sys.global_cpu_usage();
                let cpu_freq = sys.cpus().first().map(|c| c.frequency()).unwrap_or(0);
                let cpu_cores = sys.cpus().len();
                let phys_cores = sys.physical_core_count().unwrap_or(0);

                let mem_total = sys.total_memory();
                let mem_used = sys.used_memory();
                let mem_free = sys.free_memory();
                let swap_total = sys.total_swap();
                let swap_used = sys.used_swap();

                let uptime = System::uptime();
                let boot_time = System::boot_time();

                let gpu = active_gpu().ok().map(|g| {
                    let info = g.info();
                    (
                        g.vendor().to_string(),
                        g.model().to_string(),
                        g.family().to_string(),
                        info.total_vram(),
                        info.used_vram(),
                        info.temperature(),
                        info.load_pct(),
                    )
                });

                let _ = qt_thread.queue(move |mut this| {
                    let _ = this.as_mut().set_os(QString::from(
                        &os.clone().unwrap_or_else(|| "<unknown>".to_owned()),
                    ));
                    let _ = this.as_mut().set_kernel_version(QString::from(
                        &kernel.clone().unwrap_or_else(|| "<unknown>".to_owned()),
                    ));
                    let _ = this.as_mut().set_os_version(QString::from(
                        &os_ver.clone().unwrap_or_else(|| "<unknown>".to_owned()),
                    ));
                    let _ = this.as_mut().set_uptime(uptime);
                    let _ = this.as_mut().set_boot_time(boot_time);
                    let _ = this
                        .as_mut()
                        .set_cpu_architecture(QString::from(std::env::consts::ARCH));
                    let _ = this.as_mut().set_cpu_usage(cpu_usage as f64);
                    let _ = this.as_mut().set_cpu_frequency(cpu_freq);
                    let _ = this.as_mut().set_cpu_cores(cpu_cores as u32);
                    let _ = this.as_mut().set_physical_cores(phys_cores as u32);
                    let _ = this.as_mut().set_memory_total(mem_total);
                    let _ = this.as_mut().set_memory_used(mem_used);
                    let _ = this.as_mut().set_memory_free(mem_free);
                    let _ = this.as_mut().set_memory_swap_total(swap_total);
                    let _ = this.as_mut().set_memory_swap_used(swap_used);

                    if let Some((ref vendor, ref model, ref family, total, used, temp, util)) = gpu
                    {
                        let _ = this.as_mut().set_gpu_vendor(QString::from(&vendor.clone()));
                        let _ = this.as_mut().set_gpu_model(QString::from(&model.clone()));
                        let _ = this.as_mut().set_gpu_family(QString::from(&family.clone()));
                        let _ = this.as_mut().set_gpu_total_vram(total);
                        let _ = this.as_mut().set_gpu_used_vram(used);
                        let _ = this.as_mut().set_gpu_free_vram(total - used);
                        let _ = this
                            .as_mut()
                            .set_gpu_temperature(temp as f64 / 1000.0);
                        let _ = this.as_mut().set_gpu_utilization(util as f64);
                    }
                });

                thread::sleep(Duration::from_secs(1));
            }
        });
    }
}
