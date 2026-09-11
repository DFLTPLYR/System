# system

Qt Quick (cxx-qt) system plugin exposing hardware, colorscheme, font, notification and weather data to QML.

All types are QML singletons provided by the `System` module:

```qml
import System
```

## Hardware — `System.Hardware`

Polled every second on a background thread.

| Property | Type | Description |
| --- | --- | --- |
| `os` | string | OS name |
| `kernel_version` | string | Kernel version |
| `os_version` | string | OS version |
| `uptime` | int | System uptime in seconds |
| `boot_time` | int | Boot time Unix timestamp |
| `cpu_architecture` | string | CPU architecture |
| `cpu_usage` | real | Overall CPU usage % |
| `cpu_frequency` | int | CPU frequency MHz |
| `cpu_cores` | int | Total logical cores |
| `physical_cores` | int | Physical cores |
| `memory_total` | int | Total RAM bytes |
| `memory_used` | int | Used RAM bytes |
| `memory_free` | int | Free RAM bytes |
| `memory_swap_total` | int | Total swap bytes |
| `memory_swap_used` | int | Used swap bytes |
| `gpu_vendor` | string | GPU vendor (only if a GPU is found) |
| `gpu_model` | string | GPU model |
| `gpu_family` | string | GPU family |
| `gpu_total_vram` | int | Total VRAM bytes |
| `gpu_used_vram` | int | Used VRAM bytes |
| `gpu_free_vram` | int | Free VRAM bytes (`total - used`) |
| `gpu_temperature` | real | GPU temperature (Celsius) |
| `gpu_utilization` | real | GPU load % |

No functions or signals.

```qml
Text { text: "CPU: " + System.Hardware.cpu_usage.toFixed(1) + "%" }
```

## ScreenRecord — `System.ScreenRec`

Screen/audio capture via [`gpu-screen-recorder`](https://git.dec05eba.com/gpu-screen-recorder/about). Regular recording uses `gpu-screen-recorder -w <target> -c mp4 -f <fps> -o <path>` with optional `slurp` geometry selection and desktop audio capture. Replay mode runs `gpu-screen-recorder -w <target> -c mp4 -f <fps> -r <duration> -o <Videos>` started when `replay` is `true`, and `clip()` triggers `kill -SIGUSR1 <pid>` (saves the buffer as `Replay_*.mp4`, daemon keeps running).

| Property | Type | Description |
| --- | --- | --- |
| `is_running` | bool | `true` once capture is actually live (after KMS/portal auth) — stays `false` while waiting for permission |
| `elapsed` | int | Recording elapsed time in seconds, starts ticking once capture is live, resets to `0` on stop |
| `audio` | bool | Capture desktop audio (`-a default_output`) |
| `display` | bool | `true` = capture output (`-w` via `slurp -o -f "%o"`), `false` = capture region (`-w region -region` via `slurp -f "%wx%h+%x+%y"`) |
| `replay` | bool | Enable replay mode on startup |
| `monitor` | string | Capture target (`-w`): monitor name, `screen`, `portal`, … (empty = `screen`) |
| `fps` | int | Frame rate (`-f`) for both regular and replay mode |
| `duration` | int | Replay buffer length in seconds (`-r`) (default `30`, clamped 2–3600) |

### Functions

| Function | Arguments | Description |
| --- | --- | --- |
| `start(path)` | `path`: string | Request recording to `path` (`is_running`/`started` fire once capture goes live, `error` if auth is denied) |
| `stop()` | — | Stop active recording (`kill -INT`, finalizes the file) |
| `clip()` | — | Save replay buffer (`kill -SIGUSR1`, daemon keeps running) and emit `clipped(path)` |

### Signals

| Signal | Arguments |
| --- | --- |
| `started()` | Emitted when capture actually begins (after auth) |
| `finished(path)` | Emitted when recording finishes |
| `clipped(path)` | Emitted after `clip()` succeeds with the saved file path |
| `error(message)` | Emitted on spawn/wait/kill failures |

```qml
// Regular recording
System.ScreenRec.audio = true
System.ScreenRec.display = true
System.ScreenRec.start("/tmp/out.mp4")
System.ScreenRec.stop()

// Replay/history (auto-starts if replay:true)
System.ScreenRec.replay = true
System.ScreenRec.duration = 30
System.ScreenRec.fps = 60
System.ScreenRec.clip()
System.ScreenRec.clipped.connect(() => console.log("clip saved"))
System.ScreenRec.finished.connect((path) => console.log("saved to", path))
```

History mode saves to `$HOME/Videos/Replay_YYYY-MM-DD_HH-MM-SS.mp4` (created if needed; `dirs::video_dir` fallback to `~/Videos`). The daemon keeps buffering after each clip (ShadowPlay-style). Regular `start(path)` still uses the caller-provided path.

Requires `gpu-screen-recorder`, `slurp` (for geometry), and `kill`/`pkill` (`procps`) for signaling (`clip` tries `kill -USR1 <pid>` first, falling back to `pkill -SIGUSR1 -f gpu-screen-recorder`).

## Colorscheme — `System.Colorscheme`

Generates and applies a Material You color scheme via [matugen](https://github.com/InioX/matugen).

| Property | Type | Description |
| --- | --- | --- |
| `is_running` | bool | `true` while generate/apply is executing |

### Functions

| Function | Arguments | Description |
| --- | --- | --- |
| `generate(paths, typeKey)` | `paths`: list<string> (file:// URLs or paths), `typeKey`: string (`"dark"`, `"light"`…) | Combine wallpapers (via ImageMagick `magick`) and run `matugen -t <typeKey> image <combined>` |
| `apply(maincolor, path, json)` | `maincolor`: string hex, `path`: string config path, `json`: string import JSON | Run `matugen color hex` with config + import JSON |

### Signals

| Signal | Arguments |
| --- | --- |
| `generated(success)` | Emitted when `generate()` finishes |
| `applied(success)` | Emitted when `apply()` finishes |

```qml
System.Colorscheme.generated.connect((ok) => {
    console.log(ok ? "scheme generated" : "failed")
})
System.Colorscheme.generate(["/tmp/a.png", "/tmp/b.png"], "dark")
```

Requires `matugen` and ImageMagick (`magick`) installed.

## SysFont — `System.SysFont`

Lists fonts available on the system via Qt's `QFontDatabase`.

| Property | Type | Description |
| --- | --- | --- |
| `list` | list\<object\> | One object per font family: `{ name, family, families, category, mono }` |
| `current` | string | The system's current default font family name |
| `app_font_family` | string | Last family passed to `apply()` |
| `app_font_size` | int | Last size passed to `apply()` |

Each `list` entry is a real QML object (a `QVariantMap`), so it supports
JS array helpers like `filter`/`map`/`find` without `JSON.parse`:

| Key | Type | Description |
| --- | --- | --- |
| `name` / `family` / `families` | string | Font family name (e.g. `"Inter"`) — three aliases for the same value |
| `category` | string | `"monospace"` or `"sans-serif"` |
| `mono` | bool | Same as `category === "monospace"` |

### Functions

| Function | Arguments | Description |
| --- | --- | --- |
| `refresh()` | — | Re-scan fonts on a background thread (runs automatically at startup) |
| `apply(family, pointSize, category)` | `family`: string or list object, `pointSize`: int, `category`: string (`"monospace"`/`"sans-serif"`/`"serif"`) | Set the app font, persist a fontconfig alias, and publish `current`/`app_font_family`/`app_font_size` |

`apply()` accepts a plain name (`"Inter"`), a legacy JSON string, or a full
`list` entry object.

Populated asynchronously — bind to the properties rather than reading immediately
after construction.

```qml
// Filter by category or name — no JSON.parse needed.
property var monoFonts: System.SysFont.list.filter(s => s.category === "monospace")
property var inter: System.SysFont.list.filter(s => s.families === "Inter")
// `family` and `name` are aliases of `families`:
property var found: System.SysFont.list.find(s => s.family === "Inter")

ListView {
    model: System.SysFont.list
    delegate: Text { text: modelData.name }
}

// All of these work:
System.SysFont.apply("Inter", 12, "sans-serif")
System.SysFont.apply(monoFonts[0], 12, "monospace")
System.SysFont.apply(monoFonts[0].name, 12, "monospace")
```

> Troubleshooting an empty `filter()` result:
> - `family` / `families` / `name` hold the font **name** (`"Inter"`).
>   `category` holds `"monospace"` / `"sans-serif"` — so
>   `filter(s => s.families === "sans-serif")` is always `[]`; use
>   `filter(s => s.category === "sans-serif")`.
> - `list` fills in asynchronously. A `property var x: SysFont.list.filter(...)`
>   snapshot taken at startup stays `[]`; recompute in
>   `onListChanged` or guard with `SysFont.list.length`.
> - Console shows entries as `[object V4ReferenceObject]` — that is normal
>   rendering for these objects; check fields with
>   `JSON.stringify(SysFont.list[0])` and `Object.keys(SysFont.list[0])`.

## FileManager — `System.FileManager`

Instantiable QML object that pops up the system's default file picker via
[`rfd`](https://docs.rs/rfd) (which uses the XDG Desktop Portal). Emits `output`
with the picked file's path and MIME type once a real file is selected.

### Functions

| Function | Arguments | Description |
| --- | --- | --- |
| `open()` | — | Pop up the file picker (starts in `$HOME`) |

### Signals

| Signal | Arguments |
| --- | --- |
| `output(path, mimeType)` | Emitted when a file is selected — `path` is the local path, `mimeType` is the Qt-detected MIME type (e.g. `image/png`) |

```qml
FileManager {
    id: fileManager
    onOutput: (path, mimeType) => console.log(path, mimeType)
}

Button {
    text: "Pick a file"
    onClicked: fileManager.open()
}
```

## Notification — `System.Notification`

Desktop notifications over DBus (`org.freedesktop.Notifications`) via notify-rust.
Works with any notification daemon (mako, dunst, swaync, …).

### Functions

| Function | Arguments | Description |
| --- | --- | --- |
| `send(args)` | `args`: object | Show a notification |

`send()` accepts a JS object:

| Key | Type | Description |
| --- | --- | --- |
| `appname` | string | Custom app name shown in the notification |
| `title` (or `summary`) | string | The title line |
| `body` | string | Supporting text (optional) |
| `icon` | string | Icon name from your icon theme (optional) |
| `timeout` | int | Lifetime in ms (optional, `-1` = persistent) |

No properties or signals.

```qml
System.Notification.send({
    appname: "MyApp",
    title: "Backup complete",
    body: "All files synced.",
    icon: "dialog-information",
    timeout: 5000,
})
```

## Weather — `System.Weather`

Open-Meteo weather, refreshed on a background thread (cached for 1 hour).

| Property | Type | Description |
| --- | --- | --- |
| `opts` | string | JSON config (write to change options) |
| `weather_json` | string | Every metric as one JSON object |
| `location_name` / `location_region` / `location_country` | string | Location display names |
| `location_lat` / `location_lon` | real | Coordinates |
| `location_tz_id` | string | Timezone |
| `location_localtime` | string | Local time at location |
| `temp_c` / `temp_f` | real | Temperature |
| `condition` | string | Condition text (e.g. "Partly cloudy") |
| `condition_icon` | string | Condition icon slug (e.g. "partly-cloudy") |
| `wind_kph` / `wind_mph` | real | Wind speed |
| `wind_degree` | int | Wind direction in degrees |
| `wind_dir` | string | Cardinal direction (e.g. "NNW") |
| `pressure_mb` / `pressure_in` | real | Surface pressure |
| `precip_mm` / `precip_in` | real | Precipitation |
| `humidity` | int | Relative humidity % |
| `cloud` | int | Cloud cover % |
| `feelslike_c` / `feelslike_f` | real | Apparent temperature |
| `windchill_c` / `windchill_f` | real | (populated by weather API when applicable) |
| `heatindex_c` / `heatindex_f` | real | (populated by weather API when applicable) |
| `dewpoint_c` / `dewpoint_f` | real | Dew point |
| `vis_km` / `vis_miles` | real | Visibility |
| `uv` | real | UV index |
| `gust_kph` / `gust_mph` | real | Wind gusts |
| `last_updated` | string | Last observation time |
| `is_day` | bool | Day/night flag |
| `forecast_json` | string | Full forecast JSON (`{ forecastday: [...] }`) |
| `forecast_days` | list\<string\> | One JSON string per forecast day |

### Functions

| Function | Arguments | Description |
| --- | --- | --- |
| `set opts(...)` | string | Assign `opts` JSON to reconfigure (NOTIFY `optsChanged`) |

### Signals

| Signal | Arguments |
| --- | --- |
| `optsChanged()` | Emitted after `opts` is reassigned |

### `opts` config

JSON keys (all optional):

| Key | Type | Description |
| --- | --- | --- |
| `location` | object `{lat, lng}` or string (COORDS / name) | Place; falls back to `WEATHER_LAT`+`WEATHER_LON`, then `WEATHER_LOCATION` |
| `current` / `hourly` / `daily` | list\<string\> | Variable lists (defaults applied per Open-Meteo) |
| `temperature_unit` | string | `"celsius"` / `"fahrenheit"` |
| `wind_speed_unit` | string | `"kmh"` / `"ms"` / `"mph"` / `"kn"` |
| `precipitation_unit` | string | `"mm"` / `"inch"` |
| `time_zone` | string | IANA timezone |
| `forecast_days` | int | Forecast length |
| `past_days` | int | Past days included |
| `cell_selection` | string | `"land"` or `"sea"` |
| `models` | list\<string\> | Open-Meteo model names |
| `apikey` | string | Optional API key |
| `start_date` / `end_date` | string | `YYYY-MM-DD` |

Location selection priority: `opts.location` → `WEATHER_LAT`/`WEATHER_LON` env → `WEATHER_LOCATION` env → default.

```qml
System.Weather.opts = JSON.stringify({
    location: "Berlin",
    temperature_unit: "celsius",
    forecast_days: 5,
})
```

## Widgets — `System.Widgets`

Stateless helpers to create QtObjects from QML and write dynamic properties
onto them (`QObject::setProperty()`).

### Functions

| Function | Arguments | Description |
| --- | --- | --- |
| `create_object()` | — | Creates a plain QtObject and returns it |
| `set_property(target, key, value)` | `target`: QtObject, `key`: string, `value`: any | Writes a dynamic property onto `target` |

No properties or signals.

Note: dynamic properties written this way have no change signals — readers
won't re-evaluate automatically when a value changes.

```qml
import System

Component {
    id: factory
    QtObject {}
}

property var obj: Widgets.createObject()
Component.onCompleted: Widgets.set_property(obj, "size", 20)
```

## Building

### Prerequisites

A Nix flake dev shell is provided with all required dependencies (Qt, Rust, CMake, Wayland, etc.):

```sh
nix develop    # enter the dev shell
just build     # cargo build --release and install to ~/.local/share/qt6/qml/System
```

### Manual

```sh
cargo build --release
just build
```

## Notes

- All workers (Hardware poll, font scan, weather) run off the Qt thread and post
  results back — properties are reactive, so bind to them.
- Plugin is loaded into the host QML process; its own footprint is ~1.6 MiB
  resident and ~0% CPU when idle.

## Credits

- [matugen](https://github.com/InioX/matugen) — Material You color generation used by `System.Colorscheme`