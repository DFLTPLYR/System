use cxx_qt_lib::{QString, QStringList};
use material_colors::{
    color::Argb,
    image::{FilterType, ImageReader},
    scheme::Scheme,
    theme::ThemeBuilder,
};
use std::pin::Pin;
use std::process::Command;

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
        type ColorGen = super::ColorGenRust;

        #[qinvokable]
        fn generate(self: Pin<&mut Self>, paths: &QStringList, type_: QString);

        #[qsignal]
        fn output(self: Pin<&mut Self>, success: bool, theme_json: QString);
    }

    impl cxx_qt::Constructor<()> for ColorGen {}
}

pub struct ColorGenRust {
    pub is_running: bool,
}

impl Default for ColorGenRust {
    fn default() -> Self {
        Self { is_running: false }
    }
}

impl cxx_qt::Initialize for colorgen::ColorGen {
    fn initialize(self: Pin<&mut Self>) {}
}

impl colorgen::ColorGen {
    fn generate(mut self: Pin<&mut Self>, paths: &QStringList, _type_: QString) {
        let paths: Vec<String> = paths.iter().map(|s| s.to_string()).collect();

        let theme_json = combine_wallpaper(paths).and_then(|image_path| {
            let bytes = std::fs::read(&image_path).ok()?;
            let mut data = ImageReader::read(bytes).expect("failed to read image");
            data.resize(128, 128, FilterType::Lanczos3);
            let theme = ThemeBuilder::with_source(ImageReader::extract_color(&data)).build();
            let payload = serde_json::json!({
                "light": scheme_json(&theme.schemes.light),
                "dark": scheme_json(&theme.schemes.dark),
            });
            serde_json::to_string(&payload).ok()
        });

        self.as_mut().set_is_running(false);

        match theme_json {
            Some(json) => self.as_mut().output(true, QString::from(json)),
            None => self.as_mut().output(false, QString::default()),
        }
    }
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
