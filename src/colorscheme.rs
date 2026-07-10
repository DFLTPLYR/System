use std::pin::Pin;
use std::process::Command;

use cxx_qt_lib::{QString, QStringList};

#[cxx_qt::bridge]
mod colorscheme {
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
        type Colorscheme = super::ColorschemeRust;

        #[qinvokable]
        fn generate(self: Pin<&mut Self>, paths: &QStringList, type_: QString);

        #[qsignal]
        fn generated(self: Pin<&mut Self>, success: bool);
    }

    impl cxx_qt::Constructor<()> for Colorscheme {}
}

pub struct ColorschemeRust;

impl Default for ColorschemeRust {
    fn default() -> Self {
        Self
    }
}

impl cxx_qt::Initialize for colorscheme::Colorscheme {
    fn initialize(self: Pin<&mut Self>) {}
}

impl colorscheme::Colorscheme {
    fn generate(self: Pin<&mut Self>, paths: &QStringList, type_: QString) {
        let paths: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
        let success = if let Some(path) = combine_wallpaper(paths) {
            let mut cmd = Command::new("matugen");
            let mut child = cmd
                .arg("-t")
                .arg(type_.to_string())
                .arg("image")
                .arg(path)
                .arg("--source-color-index")
                .arg("0")
                .spawn()
                .expect("Failed to spawn matugen");
            child.wait().map(|s| s.success()).unwrap_or(false)
        } else {
            false
        };
        self.generated(success);
    }
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
