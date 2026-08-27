use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::pin::Pin;
use std::process::Command;

use cxx_qt_lib::{QString, QStringList};
use material_colors::{
    color::Argb,
    image::{FilterType, ImageReader},
    scheme::Scheme,
    theme::ThemeBuilder,
};
use rayon::prelude::*;
use regex::Regex;

#[cxx_qt::bridge]
mod colorgen {
    extern "C++Qt" {
        include!("cxx-qt-lib/qstring.h");
        include!("cxx-qt-lib/qstringlist.h");
        type QString = cxx_qt_lib::QString;
        type QStringList = cxx_qt_lib::QStringList;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, is_running)]
        #[qproperty(bool, is_dark_mode)]
        #[qproperty(QString, config_path)]
        type ColorGen = super::ColorGenRust;

        #[qinvokable]
        fn generate(self: Pin<&mut Self>, paths: &QStringList, type_: QString);

        #[qinvokable]
        fn change_theme(self: Pin<&mut Self>, json: QString);

        #[qsignal]
        fn output(self: Pin<&mut Self>, theme_json: QString);

        #[qsignal]
        fn error(self: Pin<&mut Self>, message: QString);
    }

    impl cxx_qt::Constructor<()> for ColorGen {}
}

pub struct ColorGenRust {
    pub is_running: bool,
    pub is_dark_mode: bool,
    pub config_path: QString,
}

impl Default for ColorGenRust {
    fn default() -> Self {
        Self {
            is_running: false,
            is_dark_mode: true,
            config_path: QString::default(),
        }
    }
}

impl cxx_qt::Initialize for colorgen::ColorGen {
    fn initialize(self: Pin<&mut Self>) {}
}

impl colorgen::ColorGen {
    fn generate(mut self: Pin<&mut Self>, paths: &QStringList, _type_: QString) {
        self.as_mut().set_is_running(true);
        let paths: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
        let is_dark = *self.is_dark_mode();
        let config_path = self.config_path().to_string();
        let config_path = config_path.strip_prefix("file://").unwrap_or(&config_path);

        let image_path = match combine_wallpaper(paths) {
            Some(p) => p,
            None => {
                self.as_mut().set_is_running(false);
                self.as_mut()
                    .error(QString::from("failed to combine wallpapers"));
                self.as_mut().output(QString::default());
                return;
            }
        };

        let bytes = match std::fs::read(&image_path) {
            Ok(b) => b,
            Err(e) => {
                self.as_mut().set_is_running(false);
                self.as_mut()
                    .error(QString::from(format!("failed to read image: {e}")));
                self.as_mut().output(QString::default());
                return;
            }
        };

        let mut data = ImageReader::read(bytes).expect("failed to read image");
        data.resize(128, 128, FilterType::Lanczos3);
        let theme = ThemeBuilder::with_source(ImageReader::extract_color(&data)).build();

        let scheme = if is_dark {
            &theme.schemes.dark
        } else {
            &theme.schemes.light
        };

        let variables = build_color_map(scheme, is_dark, &image_path);

        if !config_path.is_empty() {
            let errors = process_templates(Path::new(&config_path), &variables);
            for err in errors {
                self.as_mut().error(QString::from(err));
            }
        }

        let payload = serde_json::json!({
            "light": scheme_json(&theme.schemes.light),
            "dark": scheme_json(&theme.schemes.dark),
        });

        self.as_mut().set_is_running(false);

        match serde_json::to_string(&payload) {
            Ok(json) => self.as_mut().output(QString::from(json)),
            Err(e) => self
                .as_mut()
                .error(QString::from(format!("failed to serialize: {e}"))),
        }
    }

    fn change_theme(mut self: Pin<&mut Self>, json: QString) {
        self.as_mut().set_is_running(true);

        let raw = json.to_string();
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&raw);

        let value = match parsed {
            Ok(v) => v,
            Err(e) => {
                self.as_mut().set_is_running(false);
                self.as_mut()
                    .error(QString::from(format!("invalid JSON: {e}")));
                self.as_mut().output(QString::default());
                return;
            }
        };

        let mode_key = if *self.is_dark_mode() {
            "dark"
        } else {
            "light"
        };
        let scheme = match value.get(mode_key) {
            Some(s) => s,
            None => {
                self.as_mut().set_is_running(false);
                self.as_mut()
                    .error(QString::from(format!("missing \"{mode_key}\" key in JSON")));
                self.as_mut().output(QString::default());
                return;
            }
        };

        let variables = build_color_map_from_json(scheme);

        let config_path = self.config_path().to_string();
        let config_path = config_path.strip_prefix("file://").unwrap_or(&config_path);
        if !config_path.is_empty() {
            let errors = process_templates(Path::new(config_path), &variables);
            for err in errors {
                self.as_mut().error(QString::from(err));
            }
        }

        self.as_mut().set_is_running(false);
        self.as_mut().output(json);
    }
}

fn build_color_map(scheme: &Scheme, is_dark: bool, image: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();

    let fields: &[(&str, Argb)] = &[
        ("primary", scheme.primary),
        ("on_primary", scheme.on_primary),
        ("primary_container", scheme.primary_container),
        ("on_primary_container", scheme.on_primary_container),
        ("inverse_primary", scheme.inverse_primary),
        ("primary_fixed", scheme.primary_fixed),
        ("primary_fixed_dim", scheme.primary_fixed_dim),
        ("on_primary_fixed", scheme.on_primary_fixed),
        ("on_primary_fixed_variant", scheme.on_primary_fixed_variant),
        ("secondary", scheme.secondary),
        ("on_secondary", scheme.on_secondary),
        ("secondary_container", scheme.secondary_container),
        ("on_secondary_container", scheme.on_secondary_container),
        ("secondary_fixed", scheme.secondary_fixed),
        ("secondary_fixed_dim", scheme.secondary_fixed_dim),
        ("on_secondary_fixed", scheme.on_secondary_fixed),
        (
            "on_secondary_fixed_variant",
            scheme.on_secondary_fixed_variant,
        ),
        ("tertiary", scheme.tertiary),
        ("on_tertiary", scheme.on_tertiary),
        ("tertiary_container", scheme.tertiary_container),
        ("on_tertiary_container", scheme.on_tertiary_container),
        ("tertiary_fixed", scheme.tertiary_fixed),
        ("tertiary_fixed_dim", scheme.tertiary_fixed_dim),
        ("on_tertiary_fixed", scheme.on_tertiary_fixed),
        (
            "on_tertiary_fixed_variant",
            scheme.on_tertiary_fixed_variant,
        ),
        ("error", scheme.error),
        ("on_error", scheme.on_error),
        ("error_container", scheme.error_container),
        ("on_error_container", scheme.on_error_container),
        ("surface_dim", scheme.surface_dim),
        ("surface", scheme.surface),
        ("surface_tint", scheme.surface_tint),
        ("surface_bright", scheme.surface_bright),
        ("surface_container_lowest", scheme.surface_container_lowest),
        ("surface_container_low", scheme.surface_container_low),
        ("surface_container", scheme.surface_container),
        ("surface_container_high", scheme.surface_container_high),
        (
            "surface_container_highest",
            scheme.surface_container_highest,
        ),
        ("on_surface", scheme.on_surface),
        ("on_surface_variant", scheme.on_surface_variant),
        ("outline", scheme.outline),
        ("outline_variant", scheme.outline_variant),
        ("inverse_surface", scheme.inverse_surface),
        ("inverse_on_surface", scheme.inverse_on_surface),
        ("surface_variant", scheme.surface_variant),
        ("background", scheme.background),
        ("on_background", scheme.on_background),
        ("shadow", scheme.shadow),
        ("scrim", scheme.scrim),
    ];

    for (name, color) in fields {
        let hex = color.to_hex_with_pound();
        map.insert(format!("colors.{name}.default.hex"), hex.clone());
        map.insert(format!("colors.{name}.hex"), hex.clone());
        map.insert(format!("colors.{name}"), hex);
    }

    map.insert(
        "mode".to_string(),
        if is_dark { "dark" } else { "light" }.to_string(),
    );
    map.insert("image".to_string(), image.to_string());

    let terminal_colors: &[(&str, &str, Argb)] = &[
        (
            "terminal.normal.black",
            "surface_container_high",
            scheme.surface_container_high,
        ),
        ("terminal.normal.red", "error", scheme.error),
        ("terminal.normal.green", "secondary", scheme.secondary),
        ("terminal.normal.yellow", "tertiary", scheme.tertiary),
        ("terminal.normal.blue", "primary", scheme.primary),
        (
            "terminal.normal.magenta",
            "tertiary_fixed_dim",
            scheme.tertiary_fixed_dim,
        ),
        (
            "terminal.normal.cyan",
            "secondary_fixed_dim",
            scheme.secondary_fixed_dim,
        ),
        (
            "terminal.normal.white",
            "surface_bright",
            scheme.surface_bright,
        ),
        (
            "terminal.bright.black",
            "outline_variant",
            scheme.outline_variant,
        ),
        (
            "terminal.bright.red",
            "error_container",
            scheme.error_container,
        ),
        (
            "terminal.bright.green",
            "secondary_container",
            scheme.secondary_container,
        ),
        (
            "terminal.bright.yellow",
            "tertiary_container",
            scheme.tertiary_container,
        ),
        (
            "terminal.bright.blue",
            "primary_fixed_dim",
            scheme.primary_fixed_dim,
        ),
        (
            "terminal.bright.magenta",
            "tertiary_fixed",
            scheme.tertiary_fixed,
        ),
        (
            "terminal.bright.cyan",
            "secondary_fixed",
            scheme.secondary_fixed,
        ),
        ("terminal.bright.white", "on_surface", scheme.on_surface),
    ];

    for (key, _name, color) in terminal_colors {
        let hex = color.to_hex_with_pound();
        map.insert(key.to_string(), hex.clone());
        map.insert(format!("colors.{key}"), hex);
    }

    map.insert(
        "terminal.foreground".to_string(),
        scheme.on_surface.to_hex_with_pound(),
    );
    map.insert(
        "colors.terminal.foreground".to_string(),
        scheme.on_surface.to_hex_with_pound(),
    );
    map.insert(
        "terminal.background".to_string(),
        scheme.surface.to_hex_with_pound(),
    );
    map.insert(
        "colors.terminal.background".to_string(),
        scheme.surface.to_hex_with_pound(),
    );
    map.insert(
        "terminal.selectionfg".to_string(),
        scheme.surface.to_hex_with_pound(),
    );
    map.insert(
        "colors.terminal.selectionfg".to_string(),
        scheme.surface.to_hex_with_pound(),
    );
    map.insert(
        "terminal.selectionFg".to_string(),
        scheme.surface.to_hex_with_pound(),
    );
    map.insert(
        "colors.terminal.selectionFg".to_string(),
        scheme.surface.to_hex_with_pound(),
    );
    map.insert(
        "terminal.selectionbg".to_string(),
        scheme.primary.to_hex_with_pound(),
    );
    map.insert(
        "colors.terminal.selectionbg".to_string(),
        scheme.primary.to_hex_with_pound(),
    );
    map.insert(
        "terminal.selectionBg".to_string(),
        scheme.primary.to_hex_with_pound(),
    );
    map.insert(
        "colors.terminal.selectionBg".to_string(),
        scheme.primary.to_hex_with_pound(),
    );
    map.insert(
        "terminal.cursortext".to_string(),
        scheme.surface.to_hex_with_pound(),
    );
    map.insert(
        "colors.terminal.cursortext".to_string(),
        scheme.surface.to_hex_with_pound(),
    );
    map.insert(
        "terminal.cursorText".to_string(),
        scheme.surface.to_hex_with_pound(),
    );
    map.insert(
        "colors.terminal.cursorText".to_string(),
        scheme.surface.to_hex_with_pound(),
    );
    map.insert(
        "terminal.cursor".to_string(),
        scheme.primary.to_hex_with_pound(),
    );
    map.insert(
        "colors.terminal.cursor".to_string(),
        scheme.primary.to_hex_with_pound(),
    );

    map
}

fn build_color_map_from_json(scheme: &serde_json::Value) -> HashMap<String, String> {
    let mut map = HashMap::new();

    let flat_keys = &[
        "primary",
        "onprimary",
        "primarycontainer",
        "onprimarycontainer",
        "inverseprimary",
        "primaryfixed",
        "primaryfixeddim",
        "onprimaryfixed",
        "onprimaryfixedvariant",
        "secondary",
        "onsecondary",
        "secondarycontainer",
        "onsecondarycontainer",
        "secondaryfixed",
        "secondaryfixeddim",
        "onsecondaryfixed",
        "onsecondaryfixedvariant",
        "tertiary",
        "ontertiary",
        "tertiarycontainer",
        "ontertiarycontainer",
        "tertiaryfixed",
        "tertiaryfixeddim",
        "ontertiaryfixed",
        "ontertiaryfixedvariant",
        "error",
        "onerror",
        "errorcontainer",
        "onerrorcontainer",
        "surfacedim",
        "surface",
        "surfacetint",
        "surfacebright",
        "surfacecontainerlowest",
        "surfacecontainerlow",
        "surfacecontainer",
        "surfacecontainerhigh",
        "surfacecontainerhighest",
        "onsurface",
        "onsurfacevariant",
        "outline",
        "outlinevariant",
        "inversesurface",
        "inverseonsurface",
        "surfacevariant",
        "background",
        "onbackground",
        "shadow",
        "scrim",
        "hover",
        "onhover",
    ];

    for key in flat_keys {
        if let Some(val) = scheme.get(*key) {
            let hex = val.as_str().unwrap_or("");
            map.insert(format!("colors.{key}.default.hex"), hex.to_string());
            map.insert(format!("colors.{key}.hex"), hex.to_string());
            map.insert(format!("colors.{key}"), hex.to_string());
        }
    }

    let camel_to_snake: &[(&str, &str)] = &[
        ("onprimary", "on_primary"),
        ("primarycontainer", "primary_container"),
        ("onprimarycontainer", "on_primary_container"),
        ("inverseprimary", "inverse_primary"),
        ("primaryfixed", "primary_fixed"),
        ("primaryfixeddim", "primary_fixed_dim"),
        ("onprimaryfixed", "on_primary_fixed"),
        ("onprimaryfixedvariant", "on_primary_fixed_variant"),
        ("onsecondary", "on_secondary"),
        ("secondarycontainer", "secondary_container"),
        ("onsecondarycontainer", "on_secondary_container"),
        ("secondaryfixed", "secondary_fixed"),
        ("secondaryfixeddim", "secondary_fixed_dim"),
        ("onsecondaryfixed", "on_secondary_fixed"),
        ("onsecondaryfixedvariant", "on_secondary_fixed_variant"),
        ("ontertiary", "on_tertiary"),
        ("tertiarycontainer", "tertiary_container"),
        ("ontertiarycontainer", "on_tertiary_container"),
        ("tertiaryfixed", "tertiary_fixed"),
        ("tertiaryfixeddim", "tertiary_fixed_dim"),
        ("ontertiaryfixed", "on_tertiary_fixed"),
        ("ontertiaryfixedvariant", "on_tertiary_fixed_variant"),
        ("onerror", "on_error"),
        ("errorcontainer", "error_container"),
        ("onerrorcontainer", "on_error_container"),
        ("surfacedim", "surface_dim"),
        ("surfacetint", "surface_tint"),
        ("surfacebright", "surface_bright"),
        ("surfacecontainerlowest", "surface_container_lowest"),
        ("surfacecontainerlow", "surface_container_low"),
        ("surfacecontainerhigh", "surface_container_high"),
        ("surfacecontainerhighest", "surface_container_highest"),
        ("onsurfacevariant", "on_surface_variant"),
        ("outlinevariant", "outline_variant"),
        ("inversesurface", "inverse_surface"),
        ("inverseonsurface", "inverse_on_surface"),
        ("surfacevariant", "surface_variant"),
        ("onbackground", "on_background"),
        ("onhover", "on_hover"),
    ];

    for &(camel, snake) in camel_to_snake {
        if let Some(val) = scheme.get(camel) {
            let hex = val.as_str().unwrap_or("");
            map.insert(format!("colors.{snake}.default.hex"), hex.to_string());
            map.insert(format!("colors.{snake}.hex"), hex.to_string());
            map.insert(format!("colors.{snake}"), hex.to_string());
        }
    }

    if let Some(terminal) = scheme.get("terminal") {
        for section in &["normal", "bright"] {
            if let Some(obj) = terminal.get(*section) {
                for color in &[
                    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
                ] {
                    if let Some(val) = obj.get(*color) {
                        let hex = val.as_str().unwrap_or("");
                        map.insert(format!("terminal.{section}.{color}"), hex.to_string());
                        map.insert(
                            format!("colors.terminal.{section}.{color}"),
                            hex.to_string(),
                        );
                    }
                }
            }
        }

        for key in &[
            "foreground",
            "background",
            "selectionfg",
            "selectionbg",
            "cursortext",
            "cursor",
        ] {
            if let Some(val) = terminal.get(*key) {
                let hex = val.as_str().unwrap_or("");
                map.insert(format!("terminal.{key}"), hex.to_string());
                map.insert(format!("colors.terminal.{key}"), hex.to_string());
            }
        }

        let terminal_aliases: &[(&str, &str)] = &[
            ("selectionfg", "selectionFg"),
            ("selectionbg", "selectionBg"),
            ("cursortext", "cursorText"),
        ];
        for &(lower, camel) in terminal_aliases {
            if let Some(val) = terminal.get(lower) {
                let hex = val.as_str().unwrap_or("");
                map.insert(format!("terminal.{camel}"), hex.to_string());
                map.insert(format!("colors.terminal.{camel}"), hex.to_string());
            }
        }
    }

    map
}

fn render_template(content: &str, variables: &HashMap<String, String>) -> String {
    let re = Regex::new(r"\{\{\s*(.+?)\s*\}\}").unwrap();
    re.replace_all(content, |caps: &regex::Captures| {
        let expr = caps[1].trim();
        variables
            .get(expr)
            .cloned()
            .unwrap_or_else(|| caps[0].to_string())
    })
    .to_string()
}

fn run_hook(hook: &str, variables: &HashMap<String, String>) -> bool {
    let rendered = render_template(hook, variables);
    Command::new("sh")
        .arg("-c")
        .arg(&rendered)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(serde::Deserialize)]
struct TemplatesConfig {
    templates: Option<HashMap<String, TemplateEntry>>,
}

#[derive(serde::Deserialize)]
struct TemplateEntry {
    input_path: String,
    output_path: String,
    pre_hook: Option<String>,
    post_hook: Option<String>,
}

fn process_templates(config_path: &Path, variables: &HashMap<String, String>) -> Vec<String> {
    let config_file = config_path.join("config.toml");
    let content = match fs::read_to_string(&config_file) {
        Ok(c) => c,
        Err(e) => {
            return vec![format!("failed to read {}: {e}", config_file.display())];
        }
    };

    let config: TemplatesConfig = match toml::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            return vec![format!("failed to parse config.toml: {e}")];
        }
    };

    let templates = match config.templates {
        Some(t) => t,
        None => {
            return vec!["no [templates] sections in config.toml".to_string()];
        }
    };

    let home = std::env::var("HOME").unwrap_or_default();
    let config_path = config_path.to_path_buf();

    templates
        .into_par_iter()
        .flat_map(|(name, entry)| {
            let mut errors = Vec::new();

            let input = config_path.join(&entry.input_path);
            let content = match fs::read_to_string(&input) {
                Ok(c) => c,
                Err(e) => {
                    errors.push(format!("{name}: failed to read {}: {e}", input.display()));
                    return errors;
                }
            };

            if let Some(ref hook) = entry.pre_hook {
                if !run_hook(hook, variables) {
                    errors.push(format!("{name}: pre_hook failed: {hook}"));
                }
            }

            let rendered = render_template(&content, variables);

            let output_raw = entry.output_path.replace("~", &home);
            let output = Path::new(&output_raw);
            if let Some(parent) = output.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(e) = fs::write(output, &rendered) {
                errors.push(format!(
                    "{name}: failed to write {}: {e}",
                    entry.output_path
                ));
            }

            if let Some(ref hook) = entry.post_hook {
                if !run_hook(hook, variables) {
                    errors.push(format!("{name}: post_hook failed: {hook}"));
                }
            }

            errors
        })
        .collect()
}

fn hex(color: Argb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue)
}

fn terminal_json(s: &Scheme) -> serde_json::Value {
    serde_json::json!({
        "normal": {
            "black": hex(s.surface_container_high),
            "red": hex(s.error),
            "green": hex(s.secondary),
            "yellow": hex(s.tertiary),
            "blue": hex(s.primary),
            "magenta": hex(s.tertiary_fixed_dim),
            "cyan": hex(s.secondary_fixed_dim),
            "white": hex(s.surface_bright),
        },
        "bright": {
            "black": hex(s.outline_variant),
            "red": hex(s.error_container),
            "green": hex(s.secondary_container),
            "yellow": hex(s.tertiary_container),
            "blue": hex(s.primary_fixed_dim),
            "magenta": hex(s.tertiary_fixed),
            "cyan": hex(s.secondary_fixed),
            "white": hex(s.on_surface),
        },
        "foreground": hex(s.on_surface),
        "background": hex(s.surface),
        "selectionFg": hex(s.surface),
        "selectionBg": hex(s.primary),
        "cursorText": hex(s.surface),
        "cursor": hex(s.primary),
    })
}

fn scheme_json(s: &Scheme) -> serde_json::Value {
    serde_json::json!({
        "primary": hex(s.primary),
        "on_primary": hex(s.on_primary),
        "secondary": hex(s.secondary),
        "on_secondary": hex(s.on_secondary),
        "tertiary": hex(s.tertiary),
        "on_tertiary": hex(s.on_tertiary),
        "error": hex(s.error),
        "on_error": hex(s.on_error),
        "surface": hex(s.surface),
        "on_surface": hex(s.on_surface),
        "surface_variant": hex(s.surface_variant),
        "on_surface_variant": hex(s.on_surface_variant),
        "outline": hex(s.outline),
        "shadow": hex(s.shadow),
        "hover": hex(s.tertiary),
        "on_hover": hex(s.on_tertiary),
        "terminal": terminal_json(s),
    })
}

fn combine_wallpaper(paths: Vec<String>) -> Option<String> {
    let output = "/tmp/combined_wallpaper.png".to_string();
    let mut cmd = Command::new("magick");
    for path in paths {
        let local_path = if let Some(stripped) = path.strip_prefix("file://") {
            stripped
        } else {
            &path
        };
        cmd.arg("(")
            .arg(local_path)
            .arg("-resize")
            .arg("960x1080!")
            .arg(")");
    }
    cmd.arg("+append").arg(&output);
    match cmd.status() {
        Ok(status) if status.success() => Some(output),
        _ => None,
    }
}
