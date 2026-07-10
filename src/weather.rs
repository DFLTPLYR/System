use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use reqwest::blocking::Client;
use serde::Deserialize;
use urlencoding::encode;

static WEATHER_CACHE: LazyLock<Arc<Mutex<Option<(String, Instant)>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(None)));

fn clear_cache() {
    let mut cache = WEATHER_CACHE.lock().unwrap();
    *cache = None;
}

#[derive(Deserialize)]
struct WeatherApiResponse {
    location: Location,
    current: Current,
}

#[derive(Deserialize)]
struct Location {
    name: String,
    region: String,
    country: String,
}

#[derive(Deserialize)]
struct Condition {
    text: String,
    icon: String,
}

#[derive(Deserialize)]
struct Current {
    temp_c: f64,
    temp_f: f64,
    is_day: u8,
    condition: Condition,
    wind_kph: f64,
    humidity: u32,
    cloud: u32,
    feelslike_c: f64,
    feelslike_f: f64,
    precip_mm: f64,
    pressure_mb: f64,
    gust_kph: f64,
    uv: f64,
    last_updated: String,
}

#[cxx_qt::bridge]
mod weather {
    extern "C++Qt" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, weather_json)]
        #[qproperty(QString, location_name)]
        #[qproperty(QString, location_region)]
        #[qproperty(QString, location_country)]
        #[qproperty(f64, temp_c)]
        #[qproperty(f64, temp_f)]
        #[qproperty(QString, condition)]
        #[qproperty(QString, condition_icon)]
        #[qproperty(u32, humidity)]
        #[qproperty(f64, wind_kph)]
        #[qproperty(f64, feelslike_c)]
        #[qproperty(f64, feelslike_f)]
        #[qproperty(f64, uv)]
        #[qproperty(f64, precip_mm)]
        #[qproperty(f64, pressure_mb)]
        #[qproperty(f64, gust_kph)]
        #[qproperty(QString, last_updated)]
        #[qproperty(bool, is_day)]
        #[qproperty(u32, cloud)]
        type Weather = super::WeatherRust;

    }

    impl cxx_qt::Constructor<()> for Weather {}
    impl cxx_qt::Threading for Weather {}
}

pub struct WeatherRust {
    pub running: Arc<AtomicBool>,
    pub use_curl: bool,
    pub weather_json: QString,
    pub location_name: QString,
    pub location_region: QString,
    pub location_country: QString,
    pub temp_c: f64,
    pub temp_f: f64,
    pub condition: QString,
    pub condition_icon: QString,
    pub humidity: u32,
    pub wind_kph: f64,
    pub feelslike_c: f64,
    pub feelslike_f: f64,
    pub uv: f64,
    pub precip_mm: f64,
    pub pressure_mb: f64,
    pub gust_kph: f64,
    pub last_updated: QString,
    pub is_day: bool,
    pub cloud: u32,
}

impl Default for WeatherRust {
    fn default() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(true)),
            use_curl: false,
            weather_json: QString::default(),
            location_name: QString::default(),
            location_region: QString::default(),
            location_country: QString::default(),
            temp_c: 0.0,
            temp_f: 0.0,
            condition: QString::default(),
            condition_icon: QString::default(),
            humidity: 0,
            wind_kph: 0.0,
            feelslike_c: 0.0,
            feelslike_f: 0.0,
            uv: 0.0,
            precip_mm: 0.0,
            pressure_mb: 0.0,
            gust_kph: 0.0,
            last_updated: QString::default(),
            is_day: false,
            cloud: 0,
        }
    }
}

impl Drop for WeatherRust {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

impl cxx_qt::Initialize for weather::Weather {
    fn initialize(self: Pin<&mut Self>) {
        let qt_thread = self.qt_thread();
        let running = self.rust().running.clone();
        let use_curl = self.rust().use_curl;

        thread::spawn(move || {
            let client_ip = if use_curl {
                match std::process::Command::new("curl")
                    .arg("-s")
                    .arg("https://api.ipify.org")
                    .output()
                {
                    Ok(output) => {
                        if output.status.success() {
                            String::from_utf8_lossy(&output.stdout).trim().to_string()
                        } else {
                            "auto:ip".to_string()
                        }
                    }
                    Err(_) => "auto:ip".to_string(),
                }
            } else {
                "auto:ip".to_string()
            };

            let api_key = match std::env::var("WEATHER_API") {
                Ok(k) => k,
                Err(_) => {
                    let _ = qt_thread.queue(move |mut this| {
                        let _ = this.as_mut().set_weather_json(QString::from(
                            r#"{"error":"WEATHER_API env var not set"}"#,
                        ));
                    });
                    return;
                }
            };

            loop {
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                let now = Instant::now();
                let cached_data = {
                    let cache = WEATHER_CACHE.lock().unwrap();
                    cache.as_ref().and_then(|(data, timestamp)| {
                        if now.duration_since(*timestamp) < Duration::from_secs(3600) {
                            Some(data.clone())
                        } else {
                            None
                        }
                    })
                };

                let weather_data = match cached_data {
                    Some(data) => data,
                    None => {
                        if !running.load(Ordering::SeqCst) {
                            break;
                        }
                        let client = Client::new();
                        let url = format!(
                            "https://api.weatherapi.com/v1/forecast.json?key={}&q={}&days=3&aqi=no&alerts=no",
                            api_key,
                            encode(&client_ip)
                        );
                        match client.get(&url).send() {
                            Ok(resp) if resp.status().is_success() => match resp.text() {
                                Ok(text) => {
                                    {
                                        let mut cache = WEATHER_CACHE.lock().unwrap();
                                        *cache = Some((text.clone(), now));
                                    }
                                    text
                                }
                                Err(_) => {
                                    let _ = qt_thread.queue(move |mut this| {
                                        let _ = this.as_mut().set_weather_json(QString::from(
                                            r#"{"error":"Failed to read weather response"}"#,
                                        ));
                                    });
                                    thread::sleep(Duration::from_secs(1));
                                    continue;
                                }
                            },
                            Ok(resp) => {
                                let err = format!(
                                    r#"{{"error":"Failed to fetch weather data: {}"}}"#,
                                    resp.status()
                                );
                                let _ = qt_thread.queue(move |mut this| {
                                    let _ = this.as_mut().set_weather_json(QString::from(&err));
                                });
                                thread::sleep(Duration::from_secs(1));
                                continue;
                            }
                            Err(e) => {
                                let err = format!(r#"{{"error":"Request failed: {}"}}"#, e);
                                let _ = qt_thread.queue(move |mut this| {
                                    let _ = this.as_mut().set_weather_json(QString::from(&err));
                                });
                                thread::sleep(Duration::from_secs(1));
                                continue;
                            }
                        }
                    }
                };

                let json = weather_data.clone();
                let parsed = serde_json::from_str::<WeatherApiResponse>(&weather_data).ok();

                let _ = qt_thread.queue(move |mut this| {
                    let _ = this.as_mut().set_weather_json(QString::from(&json));
                    if let Some(ref data) = parsed {
                        let _ = this
                            .as_mut()
                            .set_location_name(QString::from(&data.location.name));
                        let _ = this
                            .as_mut()
                            .set_location_region(QString::from(&data.location.region));
                        let _ = this
                            .as_mut()
                            .set_location_country(QString::from(&data.location.country));
                        let _ = this.as_mut().set_temp_c(data.current.temp_c);
                        let _ = this.as_mut().set_temp_f(data.current.temp_f);
                        let _ = this
                            .as_mut()
                            .set_condition(QString::from(&data.current.condition.text));
                        let _ = this
                            .as_mut()
                            .set_condition_icon(QString::from(&data.current.condition.icon));
                        let _ = this.as_mut().set_humidity(data.current.humidity);
                        let _ = this.as_mut().set_wind_kph(data.current.wind_kph);
                        let _ = this.as_mut().set_feelslike_c(data.current.feelslike_c);
                        let _ = this.as_mut().set_feelslike_f(data.current.feelslike_f);
                        let _ = this.as_mut().set_uv(data.current.uv);
                        let _ = this.as_mut().set_precip_mm(data.current.precip_mm);
                        let _ = this.as_mut().set_pressure_mb(data.current.pressure_mb);
                        let _ = this.as_mut().set_gust_kph(data.current.gust_kph);
                        let _ = this
                            .as_mut()
                            .set_last_updated(QString::from(&data.current.last_updated));
                        let _ = this.as_mut().set_is_day(data.current.is_day != 0);
                        let _ = this.as_mut().set_cloud(data.current.cloud);
                    }
                });

                thread::sleep(Duration::from_secs(1));
            }

            clear_cache();
        });
    }
}
