use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QList, QString};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
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
    forecast: Forecast,
}

#[derive(Deserialize)]
struct Location {
    name: String,
    region: String,
    country: String,
    lat: f64,
    lon: f64,
    tz_id: String,
    localtime: String,
}

#[derive(Deserialize, Serialize)]
struct Condition {
    text: String,
    icon: String,
}

#[derive(Deserialize)]
struct Current {
    temp_c: f64,
    temp_f: f64,
    condition: Condition,
    wind_mph: f64,
    wind_kph: f64,
    wind_degree: u32,
    wind_dir: String,
    pressure_mb: f64,
    pressure_in: f64,
    precip_mm: f64,
    precip_in: f64,
    humidity: u32,
    cloud: u32,
    feelslike_c: f64,
    feelslike_f: f64,
    windchill_c: f64,
    windchill_f: f64,
    heatindex_c: f64,
    heatindex_f: f64,
    dewpoint_c: f64,
    dewpoint_f: f64,
    vis_km: f64,
    vis_miles: f64,
    uv: f64,
    gust_mph: f64,
    gust_kph: f64,
    is_day: u8,
    last_updated: String,
}

#[derive(Deserialize, Serialize)]
struct Forecast {
    forecastday: Vec<ForecastDay>,
}

#[derive(Deserialize, Serialize)]
struct ForecastDay {
    date: String,
    date_epoch: i64,
    day: Day,
    astro: Astro,
    hour: Vec<Hour>,
}

#[derive(Deserialize, Serialize)]
struct Day {
    maxtemp_c: f64,
    maxtemp_f: f64,
    mintemp_c: f64,
    mintemp_f: f64,
    avgtemp_c: f64,
    avgtemp_f: f64,
    maxwind_mph: f64,
    maxwind_kph: f64,
    totalprecip_mm: f64,
    totalprecip_in: f64,
    totalsnow_cm: f64,
    avgvis_km: f64,
    avgvis_miles: f64,
    avghumidity: u32,
    condition: Condition,
    uv: f64,
    daily_will_it_rain: u8,
    daily_will_it_snow: u8,
    daily_chance_of_rain: u32,
    daily_chance_of_snow: u32,
}

#[derive(Deserialize, Serialize)]
struct Astro {
    sunrise: String,
    sunset: String,
    moonrise: String,
    moonset: String,
    moon_phase: String,
    moon_illumination: f64,
    is_moon_up: u8,
    is_sun_up: u8,
}

#[derive(Deserialize, Serialize)]
struct Hour {
    time_epoch: i64,
    time: String,
    temp_c: f64,
    temp_f: f64,
    condition: Condition,
    wind_mph: f64,
    wind_kph: f64,
    wind_degree: u32,
    wind_dir: String,
    pressure_mb: f64,
    pressure_in: f64,
    precip_mm: f64,
    precip_in: f64,
    snow_cm: f64,
    humidity: u32,
    cloud: u32,
    feelslike_c: f64,
    feelslike_f: f64,
    windchill_c: f64,
    windchill_f: f64,
    heatindex_c: f64,
    heatindex_f: f64,
    dewpoint_c: f64,
    dewpoint_f: f64,
    will_it_rain: u8,
    will_it_snow: u8,
    chance_of_rain: u32,
    chance_of_snow: u32,
    vis_km: f64,
    vis_miles: f64,
    gust_mph: f64,
    gust_kph: f64,
    uv: f64,
    is_day: u8,
}

#[cxx_qt::bridge]
mod weather {
    extern "C++Qt" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qlist.h");
        type QList_QString = cxx_qt_lib::QList<QString>;
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
        #[qproperty(f64, location_lat)]
        #[qproperty(f64, location_lon)]
        #[qproperty(QString, location_tz_id)]
        #[qproperty(QString, location_localtime)]
        #[qproperty(f64, temp_c)]
        #[qproperty(f64, temp_f)]
        #[qproperty(QString, condition)]
        #[qproperty(QString, condition_icon)]
        #[qproperty(f64, wind_mph)]
        #[qproperty(f64, wind_kph)]
        #[qproperty(u32, wind_degree)]
        #[qproperty(QString, wind_dir)]
        #[qproperty(f64, pressure_mb)]
        #[qproperty(f64, pressure_in)]
        #[qproperty(f64, precip_mm)]
        #[qproperty(f64, precip_in)]
        #[qproperty(u32, humidity)]
        #[qproperty(u32, cloud)]
        #[qproperty(f64, feelslike_c)]
        #[qproperty(f64, feelslike_f)]
        #[qproperty(f64, windchill_c)]
        #[qproperty(f64, windchill_f)]
        #[qproperty(f64, heatindex_c)]
        #[qproperty(f64, heatindex_f)]
        #[qproperty(f64, dewpoint_c)]
        #[qproperty(f64, dewpoint_f)]
        #[qproperty(f64, vis_km)]
        #[qproperty(f64, vis_miles)]
        #[qproperty(f64, uv)]
        #[qproperty(f64, gust_mph)]
        #[qproperty(f64, gust_kph)]
        #[qproperty(QString, last_updated)]
        #[qproperty(bool, is_day)]
        #[qproperty(QString, forecast_json)]
        #[qproperty(QList_QString, forecast_days)]
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
    pub location_lat: f64,
    pub location_lon: f64,
    pub location_tz_id: QString,
    pub location_localtime: QString,
    pub temp_c: f64,
    pub temp_f: f64,
    pub condition: QString,
    pub condition_icon: QString,
    pub wind_mph: f64,
    pub wind_kph: f64,
    pub wind_degree: u32,
    pub wind_dir: QString,
    pub pressure_mb: f64,
    pub pressure_in: f64,
    pub precip_mm: f64,
    pub precip_in: f64,
    pub humidity: u32,
    pub cloud: u32,
    pub feelslike_c: f64,
    pub feelslike_f: f64,
    pub windchill_c: f64,
    pub windchill_f: f64,
    pub heatindex_c: f64,
    pub heatindex_f: f64,
    pub dewpoint_c: f64,
    pub dewpoint_f: f64,
    pub vis_km: f64,
    pub vis_miles: f64,
    pub uv: f64,
    pub gust_mph: f64,
    pub gust_kph: f64,
    pub last_updated: QString,
    pub is_day: bool,
    pub forecast_json: QString,
    pub forecast_days: QList<QString>,
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
            location_lat: 0.0,
            location_lon: 0.0,
            location_tz_id: QString::default(),
            location_localtime: QString::default(),
            temp_c: 0.0,
            temp_f: 0.0,
            condition: QString::default(),
            condition_icon: QString::default(),
            wind_mph: 0.0,
            wind_kph: 0.0,
            wind_degree: 0,
            wind_dir: QString::default(),
            pressure_mb: 0.0,
            pressure_in: 0.0,
            precip_mm: 0.0,
            precip_in: 0.0,
            humidity: 0,
            cloud: 0,
            feelslike_c: 0.0,
            feelslike_f: 0.0,
            windchill_c: 0.0,
            windchill_f: 0.0,
            heatindex_c: 0.0,
            heatindex_f: 0.0,
            dewpoint_c: 0.0,
            dewpoint_f: 0.0,
            vis_km: 0.0,
            vis_miles: 0.0,
            uv: 0.0,
            gust_mph: 0.0,
            gust_kph: 0.0,
            last_updated: QString::default(),
            is_day: false,
            forecast_json: QString::default(),
            forecast_days: QList::<QString>::default(),
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
                            "https://api.weatherapi.com/v1/forecast.json?key={}&q={}&days=3&aqi=yes&alerts=no",
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
                        let _ = this.as_mut().set_location_lat(data.location.lat);
                        let _ = this.as_mut().set_location_lon(data.location.lon);
                        let _ = this
                            .as_mut()
                            .set_location_tz_id(QString::from(&data.location.tz_id));
                        let _ = this
                            .as_mut()
                            .set_location_localtime(QString::from(&data.location.localtime));
                        let _ = this.as_mut().set_temp_c(data.current.temp_c);
                        let _ = this.as_mut().set_temp_f(data.current.temp_f);
                        let _ = this
                            .as_mut()
                            .set_condition(QString::from(&data.current.condition.text));
                        let _ = this
                            .as_mut()
                            .set_condition_icon(QString::from(&data.current.condition.icon));
                        let _ = this.as_mut().set_wind_mph(data.current.wind_mph);
                        let _ = this.as_mut().set_wind_kph(data.current.wind_kph);
                        let _ = this.as_mut().set_wind_degree(data.current.wind_degree);
                        let _ = this
                            .as_mut()
                            .set_wind_dir(QString::from(&data.current.wind_dir));
                        let _ = this.as_mut().set_pressure_mb(data.current.pressure_mb);
                        let _ = this.as_mut().set_pressure_in(data.current.pressure_in);
                        let _ = this.as_mut().set_precip_mm(data.current.precip_mm);
                        let _ = this.as_mut().set_precip_in(data.current.precip_in);
                        let _ = this.as_mut().set_humidity(data.current.humidity);
                        let _ = this.as_mut().set_cloud(data.current.cloud);
                        let _ = this.as_mut().set_feelslike_c(data.current.feelslike_c);
                        let _ = this.as_mut().set_feelslike_f(data.current.feelslike_f);
                        let _ = this.as_mut().set_windchill_c(data.current.windchill_c);
                        let _ = this.as_mut().set_windchill_f(data.current.windchill_f);
                        let _ = this.as_mut().set_heatindex_c(data.current.heatindex_c);
                        let _ = this.as_mut().set_heatindex_f(data.current.heatindex_f);
                        let _ = this.as_mut().set_dewpoint_c(data.current.dewpoint_c);
                        let _ = this.as_mut().set_dewpoint_f(data.current.dewpoint_f);
                        let _ = this.as_mut().set_vis_km(data.current.vis_km);
                        let _ = this.as_mut().set_vis_miles(data.current.vis_miles);
                        let _ = this.as_mut().set_uv(data.current.uv);
                        let _ = this.as_mut().set_gust_mph(data.current.gust_mph);
                        let _ = this.as_mut().set_gust_kph(data.current.gust_kph);
                        let _ = this
                            .as_mut()
                            .set_last_updated(QString::from(&data.current.last_updated));
                        let _ = this.as_mut().set_is_day(data.current.is_day != 0);

                        let _ = this.as_mut().set_forecast_json(QString::from(
                            &serde_json::to_string(&data.forecast).unwrap_or_default(),
                        ));
                        let mut days = QList::<QString>::default();
                        for day in &data.forecast.forecastday {
                            if let Ok(json) = serde_json::to_string(day) {
                                days.append_clone(&QString::from(&json));
                            }
                        }
                        let _ = this.as_mut().set_forecast_days(days);
                    }
                });

                thread::sleep(Duration::from_secs(1));
            }

            clear_cache();
        });
    }
}
