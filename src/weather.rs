use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Timelike;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QList, QString};
use open_meteo_rs::forecast::{
    CellSelection, Elevation, ForecastResultItem, Model, Options, PrecipitationUnit,
    TemperatureUnit, WindSpeedUnit,
};
use open_meteo_rs::geocoding;
use open_meteo_rs::{Client, Location};
use serde::Deserialize;

const KMH_TO_MPH: f64 = 0.621_371;
const MB_TO_INHG: f64 = 0.029_53;
const MM_TO_IN: f64 = 25.4;

const DEFAULT_CURRENT: &[&str] = &[
    "temperature_2m",
    "relative_humidity_2m",
    "apparent_temperature",
    "precipitation",
    "weather_code",
    "wind_speed_10m",
    "wind_direction_10m",
    "wind_gusts_10m",
    "surface_pressure",
    "cloud_cover",
    "dew_point_2m",
    "visibility",
    "uv_index",
    "is_day",
];
const DEFAULT_HOURLY: &[&str] = &[
    "temperature_2m",
    "apparent_temperature",
    "precipitation_probability",
    "weather_code",
    "wind_speed_10m",
];
const DEFAULT_DAILY: &[&str] = &[
    "weather_code",
    "temperature_2m_max",
    "temperature_2m_min",
    "sunrise",
    "sunset",
    "precipitation_sum",
    "precipitation_probability_max",
    "uv_index_max",
];

fn vec_of(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

static WEATHER_CACHE: LazyLock<Arc<Mutex<Option<(WeatherData, Instant)>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(None)));

fn clear_cache() {
    let mut cache = WEATHER_CACHE.lock().unwrap();
    *cache = None;
}

#[derive(Deserialize, Default)]
struct OptsConfig {
    location: Option<Location>,
    elevation: Option<ElevationConfig>,
    minutely_15: Option<Vec<String>>,
    hourly: Option<Vec<String>>,
    daily: Option<Vec<String>>,
    current: Option<Vec<String>>,
    temperature_unit: Option<String>,
    wind_speed_unit: Option<String>,
    precipitation_unit: Option<String>,
    time_zone: Option<String>,
    past_days: Option<u8>,
    forecast_days: Option<u8>,
    forecast_minutely_15: Option<u16>,
    start_date: Option<String>,
    end_date: Option<String>,
    models: Option<Vec<String>>,
    cell_selection: Option<String>,
    apikey: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ElevationConfig {
    Nan(String),
    Value(f64),
}

struct OptsState {
    options: Option<Options>,
    error: Option<String>,
    generation: u64,
}

static OPTS_STATE: LazyLock<Arc<Mutex<OptsState>>> = LazyLock::new(|| {
    Arc::new(Mutex::new(OptsState {
        options: None,
        error: None,
        generation: 0,
    }))
});

fn default_options() -> Options {
    Options {
        location: match env_coords() {
            Some((lat, lng)) => Location { lat, lng },
            None => Location::default(),
        },
        current: vec_of(DEFAULT_CURRENT),
        hourly: vec_of(DEFAULT_HOURLY),
        daily: vec_of(DEFAULT_DAILY),
        temperature_unit: Some(TemperatureUnit::Celsius),
        wind_speed_unit: Some(WindSpeedUnit::Kmh),
        precipitation_unit: Some(PrecipitationUnit::Millimeters),
        forecast_days: Some(3),
        ..Default::default()
    }
}

fn build_options(cfg: &OptsConfig) -> Options {
    let mut opts = Options::default();
    opts.location = cfg.location.clone().unwrap_or_else(|| match env_coords() {
        Some((lat, lng)) => Location { lat, lng },
        None => Location::default(),
    });
    opts.elevation = cfg.elevation.as_ref().and_then(|e| match e {
        ElevationConfig::Nan(s) if s == "nan" => Some(Elevation::Nan),
        ElevationConfig::Value(v) => Some(Elevation::Value(*v as f32)),
        _ => None,
    });
    opts.minutely_15 = cfg.minutely_15.clone().unwrap_or_default();
    opts.current = cfg.current.clone().unwrap_or_else(|| vec_of(DEFAULT_CURRENT));
    opts.hourly = cfg.hourly.clone().unwrap_or_else(|| vec_of(DEFAULT_HOURLY));
    opts.daily = cfg.daily.clone().unwrap_or_else(|| vec_of(DEFAULT_DAILY));
    opts.temperature_unit = cfg
        .temperature_unit
        .as_deref()
        .and_then(|s| TemperatureUnit::try_from(s).ok());
    opts.wind_speed_unit = cfg
        .wind_speed_unit
        .as_deref()
        .and_then(|s| WindSpeedUnit::try_from(s).ok());
    opts.precipitation_unit = cfg
        .precipitation_unit
        .as_deref()
        .and_then(|s| PrecipitationUnit::try_from(s).ok());
    opts.time_zone = cfg.time_zone.clone();
    opts.past_days = cfg.past_days;
    opts.forecast_days = cfg.forecast_days.or(Some(3));
    opts.forecast_minutely_15 = cfg.forecast_minutely_15;
    opts.start_date = cfg
        .start_date
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
    opts.end_date = cfg
        .end_date
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
    opts.models = cfg
        .models
        .as_ref()
        .map(|ms| ms.iter().filter_map(|m| Model::try_from(m.as_str()).ok()).collect());
    opts.cell_selection = cfg
        .cell_selection
        .as_deref()
        .and_then(|s| CellSelection::try_from(s).ok());
    opts.apikey = cfg.apikey.clone();
    opts
}

#[derive(Clone)]
struct WeatherData {
    name: String,
    region: String,
    country: String,
    lat: f64,
    lon: f64,
    tz_id: String,
    localtime: String,
    temp_c: f64,
    condition_text: String,
    condition_icon: String,
    wind_kph: f64,
    wind_degree: f64,
    pressure_mb: f64,
    precip_mm: f64,
    humidity: f64,
    cloud: f64,
    feelslike_c: f64,
    dewpoint_c: f64,
    vis_km: f64,
    uv: f64,
    gust_kph: f64,
    is_day: bool,
    last_updated: String,
    weather_json: String,
    forecast_json: String,
    daily_json: Vec<String>,
}

fn c2f(c: f64) -> f64 {
    c * 9.0 / 5.0 + 32.0
}

fn compass(deg: f64) -> String {
    const DIRS: [&str; 16] = [
        "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW",
        "NW", "NNW",
    ];
    let idx = ((deg / 22.5).round() as i64).rem_euclid(16) as usize;
    DIRS[idx].to_string()
}

fn condition(code: f64, is_day: bool) -> (String, String) {
    let (text, icon) = match code as i64 {
        0 => ("Clear", if is_day { "clear-day" } else { "clear-night" }),
        1 => (
            "Mainly clear",
            if is_day {
                "partly-cloudy"
            } else {
                "partly-cloudy-night"
            },
        ),
        2 => ("Partly cloudy", "partly-cloudy"),
        3 => ("Overcast", "cloudy"),
        45 => ("Fog", "fog"),
        48 => ("Depositing rime fog", "fog"),
        51 => ("Light drizzle", "drizzle"),
        53 => ("Drizzle", "drizzle"),
        55 => ("Dense drizzle", "drizzle"),
        56 => ("Light freezing drizzle", "drizzle"),
        57 => ("Dense freezing drizzle", "drizzle"),
        61 => ("Light rain", "rain"),
        63 => ("Rain", "rain"),
        65 => ("Heavy rain", "rain"),
        66 => ("Light freezing rain", "rain"),
        67 => ("Heavy freezing rain", "rain"),
        71 => ("Light snow", "snow"),
        73 => ("Snow", "snow"),
        75 => ("Heavy snow", "snow"),
        77 => ("Snow grains", "snow"),
        80 => ("Light rain showers", "rain-shower"),
        81 => ("Rain showers", "rain-shower"),
        82 => ("Violent rain showers", "rain-shower"),
        85 => ("Light snow showers", "snow-shower"),
        86 => ("Heavy snow showers", "snow-shower"),
        95 => ("Thunderstorm", "thunderstorm"),
        96 => ("Thunderstorm with slight hail", "thunderstorm"),
        99 => ("Thunderstorm with hail", "thunderstorm"),
        _ => ("Unknown", ""),
    };
    (text.to_string(), icon.to_string())
}

fn fval(map: &HashMap<String, ForecastResultItem>, key: &str) -> f64 {
    map.get(key).and_then(|i| i.value.as_f64()).unwrap_or(0.0)
}

fn hour_is_day(hour: u32) -> bool {
    (6..=20).contains(&hour)
}

fn fmt_hhmm(unix: f64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix as i64, 0)
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default()
}

fn parse_coords(query: &str) -> Option<(f64, f64)> {
    let parts: Vec<&str> = query.split(',').collect();
    if parts.len() == 2 {
        let lat = parts[0].trim().parse::<f64>().ok()?;
        let lon = parts[1].trim().parse::<f64>().ok()?;
        if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) {
            return Some((lat, lon));
        }
    }
    None
}

fn env_coords() -> Option<(f64, f64)> {
    let lat = std::env::var("WEATHER_LAT").ok()?.parse::<f64>().ok()?;
    let lon = std::env::var("WEATHER_LON").ok()?.parse::<f64>().ok()?;
    if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) {
        return Some((lat, lon));
    }
    None
}

async fn geocode(
    client: &Client,
    name: String,
) -> Option<(Location, String, String, String, String)> {
    if name.trim().is_empty() {
        return None;
    }
    if let Some((lat, lng)) = parse_coords(&name) {
        return Some((
            Location { lat, lng },
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ));
    }
    let resp = client
        .geocoding(geocoding::Options::default().with_name(name).with_count(1))
        .await
        .ok()?;
    let r = resp.results?.into_iter().next()?;
    Some((
        Location {
            lat: r.latitude?,
            lng: r.longitude?,
        },
        r.name.unwrap_or_default(),
        r.admin1.unwrap_or_default(),
        r.country.unwrap_or_default(),
        r.timezone.unwrap_or_default(),
    ))
}

async fn resolve_location(client: &Client) -> (Location, String, String, String, String) {
    if let Some((lat, lng)) = env_coords() {
        return (
            Location { lat, lng },
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );
    }
    if let Some(found) = geocode(
        client,
        std::env::var("WEATHER_LOCATION").unwrap_or_default(),
    )
    .await
    {
        return found;
    }
    (
        Location::default(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    )
}

async fn fetch_weather(client: &Client, opts: &Options) -> Result<WeatherData, String> {
    let (_, name, region, country, tz_id) = resolve_location(client).await;
    let loc = Location {
        lat: opts.location.lat,
        lng: opts.location.lng,
    };

    let res = client.forecast(opts.clone()).await.map_err(|e| e.to_string())?;

    let mut data = WeatherData {
        name,
        region,
        country,
        lat: loc.lat,
        lon: loc.lng,
        tz_id,
        localtime: String::new(),
        temp_c: 0.0,
        condition_text: String::new(),
        condition_icon: String::new(),
        wind_kph: 0.0,
        wind_degree: 0.0,
        pressure_mb: 0.0,
        precip_mm: 0.0,
        humidity: 0.0,
        cloud: 0.0,
        feelslike_c: 0.0,
        dewpoint_c: 0.0,
        vis_km: 0.0,
        uv: 0.0,
        gust_kph: 0.0,
        is_day: false,
        last_updated: String::new(),
        weather_json: String::new(),
        forecast_json: String::new(),
        daily_json: Vec::new(),
    };

    if let Some(cur) = res.current {
        let v = &cur.values;
        let temp_c = fval(v, "temperature_2m");
        let is_day = fval(v, "is_day") != 0.0;
        let (text, icon) = condition(fval(v, "weather_code"), is_day);
        let localtime = cur.datetime.format("%Y-%m-%d %H:%M").to_string();

        data.temp_c = temp_c;
        data.condition_text = text;
        data.condition_icon = icon;
        data.wind_kph = fval(v, "wind_speed_10m");
        data.wind_degree = fval(v, "wind_direction_10m");
        data.pressure_mb = fval(v, "surface_pressure");
        data.precip_mm = fval(v, "precipitation");
        data.humidity = fval(v, "relative_humidity_2m");
        data.cloud = fval(v, "cloud_cover");
        data.feelslike_c = fval(v, "apparent_temperature");
        data.dewpoint_c = fval(v, "dew_point_2m");
        data.vis_km = fval(v, "visibility") / 1000.0;
        data.uv = fval(v, "uv_index");
        data.gust_kph = fval(v, "wind_gusts_10m");
        data.is_day = is_day;
        data.last_updated = localtime.clone();
        data.localtime = localtime;
    }

    let mut hours_json = Vec::new();
    for hr in res.hourly.unwrap_or_default() {
        let v = &hr.values;
        let temp = fval(v, "temperature_2m");
        let (text, icon) = condition(fval(v, "weather_code"), hour_is_day(hr.datetime.hour()));
        hours_json.push(serde_json::json!({
            "time": hr.datetime.format("%Y-%m-%dT%H:%M").to_string(),
            "temp_c": temp,
            "temp_f": c2f(temp),
            "condition": { "text": text, "icon": icon },
            "wind_kph": fval(v, "wind_speed_10m"),
            "wind_mph": fval(v, "wind_speed_10m") * KMH_TO_MPH,
            "feelslike_c": fval(v, "apparent_temperature"),
            "chance_of_rain": fval(v, "precipitation_probability") as u32,
        }));
    }

    let mut days_json = Vec::new();
    for day in res.daily.unwrap_or_default() {
        let v = &day.values;
        let (text, icon) = condition(fval(v, "weather_code"), true);
        days_json.push(serde_json::json!({
            "date": day.date.to_string(),
            "day": {
                "maxtemp_c": fval(v, "temperature_2m_max"),
                "mintemp_c": fval(v, "temperature_2m_min"),
                "maxtemp_f": c2f(fval(v, "temperature_2m_max")),
                "mintemp_f": c2f(fval(v, "temperature_2m_min")),
                "condition": { "text": text, "icon": icon },
                "precip_mm": fval(v, "precipitation_sum"),
                "chance_of_rain": fval(v, "precipitation_probability_max") as u32,
                "uv": fval(v, "uv_index_max"),
            },
            "astro": {
                "sunrise": fmt_hhmm(fval(v, "sunrise")),
                "sunset": fmt_hhmm(fval(v, "sunset")),
            },
        }));
    }

    let current_json = serde_json::json!({
        "temp_c": data.temp_c,
        "temp_f": c2f(data.temp_c),
        "condition": { "text": &data.condition_text, "icon": &data.condition_icon },
        "wind_kph": data.wind_kph,
        "wind_mph": data.wind_kph * KMH_TO_MPH,
        "wind_degree": data.wind_degree,
        "wind_dir": compass(data.wind_degree),
        "pressure_mb": data.pressure_mb,
        "pressure_in": data.pressure_mb * MB_TO_INHG,
        "precip_mm": data.precip_mm,
        "precip_in": data.precip_mm / MM_TO_IN,
        "humidity": data.humidity as u32,
        "cloud": data.cloud as u32,
        "feelslike_c": data.feelslike_c,
        "feelslike_f": c2f(data.feelslike_c),
        "dewpoint_c": data.dewpoint_c,
        "dewpoint_f": c2f(data.dewpoint_c),
        "vis_km": data.vis_km,
        "vis_miles": data.vis_km * KMH_TO_MPH,
        "uv": data.uv,
        "gust_kph": data.gust_kph,
        "gust_mph": data.gust_kph * KMH_TO_MPH,
        "is_day": data.is_day,
        "last_updated": &data.last_updated,
    });

    let forecast_json_val = serde_json::json!({ "forecastday": days_json });
    let weather_json_val = serde_json::json!({
        "location": {
            "name": &data.name,
            "region": &data.region,
            "country": &data.country,
            "lat": data.lat,
            "lon": data.lon,
            "tz_id": &data.tz_id,
            "localtime": &data.localtime,
        },
        "current": current_json,
        "forecast": forecast_json_val,
    });

    data.weather_json = weather_json_val.to_string();
    data.forecast_json = serde_json::to_string(&forecast_json_val).unwrap_or_default();
    data.daily_json = days_json.iter().map(ToString::to_string).collect();

    Ok(data)
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
        #[qproperty(QString, opts, READ, WRITE = set_opts, NOTIFY = opts_changed)]
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

        #[qsignal]
        fn opts_changed(self: Pin<&mut Self>);

        fn set_opts(self: Pin<&mut Self>, opts: QString);
    }

    impl cxx_qt::Constructor<()> for Weather {}
    impl cxx_qt::Threading for Weather {}
}

pub struct WeatherRust {
    pub running: Arc<AtomicBool>,
    pub opts: QString,
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
            opts: QString::default(),
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

impl weather::Weather {
    pub fn set_opts(mut self: Pin<&mut Self>, opts: QString) {
        let raw = opts.to_string();
        self.as_mut().rust_mut().opts = opts;
        let mut state = OPTS_STATE.lock().unwrap();
        state.generation = state.generation.wrapping_add(1);
        if raw.trim().is_empty() {
            state.options = None;
            state.error = None;
        } else {
            match serde_json::from_str::<OptsConfig>(&raw) {
                Ok(cfg) => {
                    state.options = Some(build_options(&cfg));
                    state.error = None;
                }
                Err(e) => {
                    state.options = None;
                    state.error = Some(format!("Invalid opts JSON: {e}"));
                }
            }
        }
        self.as_mut().opts_changed();
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

        thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = qt_thread.queue(move |mut this| {
                        let _ = this.as_mut().set_weather_json(QString::from(format!(
                            r#"{{"error":"Failed to start async runtime: {e}"}}"#
                        )));
                    });
                    return;
                }
            };

            let client = Client::new();
            let mut last_gen = u64::MAX;

            loop {
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                let (state_opts, state_err, generation) = {
                    let state = OPTS_STATE.lock().unwrap();
                    (state.options.clone(), state.error.clone(), state.generation)
                };
                if generation != last_gen {
                    last_gen = generation;
                    clear_cache();
                    if let Some(err) = state_err {
                        let _ = qt_thread.queue(move |mut this| {
                            let _ = this.as_mut().set_weather_json(QString::from(&err));
                        });
                        thread::sleep(Duration::from_secs(1));
                        continue;
                    }
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

                let data = match cached_data {
                    Some(data) => data,
                    None => {
                        if !running.load(Ordering::SeqCst) {
                            break;
                        }
                        let opts = state_opts.unwrap_or_else(default_options);
                        match rt.block_on(fetch_weather(&client, &opts)) {
                            Ok(data) => {
                                {
                                    let mut cache = WEATHER_CACHE.lock().unwrap();
                                    *cache = Some((data.clone(), now));
                                }
                                data
                            }
                            Err(e) => {
                                let err = format!(r#"{{"error":"Request failed: {e}"}}"#);
                                let _ = qt_thread.queue(move |mut this| {
                                    let _ = this.as_mut().set_weather_json(QString::from(&err));
                                });
                                thread::sleep(Duration::from_secs(1));
                                continue;
                            }
                        }
                    }
                };

                let _ = qt_thread.queue(move |mut this| {
                    let _ = this
                        .as_mut()
                        .set_weather_json(QString::from(&data.weather_json));
                    let _ = this.as_mut().set_location_name(QString::from(&data.name));
                    let _ = this
                        .as_mut()
                        .set_location_region(QString::from(&data.region));
                    let _ = this
                        .as_mut()
                        .set_location_country(QString::from(&data.country));
                    let _ = this.as_mut().set_location_lat(data.lat);
                    let _ = this.as_mut().set_location_lon(data.lon);
                    let _ = this.as_mut().set_location_tz_id(QString::from(&data.tz_id));
                    let _ = this
                        .as_mut()
                        .set_location_localtime(QString::from(&data.localtime));
                    let _ = this.as_mut().set_temp_c(data.temp_c);
                    let _ = this.as_mut().set_temp_f(c2f(data.temp_c));
                    let _ = this
                        .as_mut()
                        .set_condition(QString::from(&data.condition_text));
                    let _ = this
                        .as_mut()
                        .set_condition_icon(QString::from(&data.condition_icon));
                    let _ = this.as_mut().set_wind_mph(data.wind_kph * KMH_TO_MPH);
                    let _ = this.as_mut().set_wind_kph(data.wind_kph);
                    let _ = this.as_mut().set_wind_degree(data.wind_degree as u32);
                    let _ = this
                        .as_mut()
                        .set_wind_dir(QString::from(&compass(data.wind_degree)));
                    let _ = this.as_mut().set_pressure_mb(data.pressure_mb);
                    let _ = this.as_mut().set_pressure_in(data.pressure_mb * MB_TO_INHG);
                    let _ = this.as_mut().set_precip_mm(data.precip_mm);
                    let _ = this.as_mut().set_precip_in(data.precip_mm / MM_TO_IN);
                    let _ = this.as_mut().set_humidity(data.humidity as u32);
                    let _ = this.as_mut().set_cloud(data.cloud as u32);
                    let _ = this.as_mut().set_feelslike_c(data.feelslike_c);
                    let _ = this.as_mut().set_feelslike_f(c2f(data.feelslike_c));
                    let _ = this.as_mut().set_dewpoint_c(data.dewpoint_c);
                    let _ = this.as_mut().set_dewpoint_f(c2f(data.dewpoint_c));
                    let _ = this.as_mut().set_vis_km(data.vis_km);
                    let _ = this.as_mut().set_vis_miles(data.vis_km * KMH_TO_MPH);
                    let _ = this.as_mut().set_uv(data.uv);
                    let _ = this.as_mut().set_gust_mph(data.gust_kph * KMH_TO_MPH);
                    let _ = this.as_mut().set_gust_kph(data.gust_kph);
                    let _ = this
                        .as_mut()
                        .set_last_updated(QString::from(&data.last_updated));
                    let _ = this.as_mut().set_is_day(data.is_day);
                    let _ = this
                        .as_mut()
                        .set_forecast_json(QString::from(&data.forecast_json));
                    let mut days = QList::<QString>::default();
                    for d in &data.daily_json {
                        days.append_clone(&QString::from(d));
                    }
                    let _ = this.as_mut().set_forecast_days(days);
                });

                thread::sleep(Duration::from_secs(1));
            }

            clear_cache();
        });
    }
}
