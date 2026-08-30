use std::fs;
use std::pin::Pin;
use std::process::Command;
use std::thread;

use cxx_qt::Threading;
use cxx_qt_lib::{QList, QString, QStringList};

#[cxx_qt::bridge]
mod font {
    extern "C++Qt" {
        include!("cxx-qt-lib/qstring.h");
        include!("cxx-qt-lib/qstringlist.h");
        include!("cxx-qt-lib/qlist.h");
        type QString = cxx_qt_lib::QString;
        type QStringList = cxx_qt_lib::QStringList;
        type QList_QString = cxx_qt_lib::QList<QString>;
        type QList_i32 = cxx_qt_lib::QList<i32>;
    }

    unsafe extern "C++" {
        include!("system/src/qfontdatabase.h");
        fn system_font_families(families: &mut QStringList);
        fn system_font_styles(family: &QString, styles: &mut QStringList);
        fn system_font_sizes(family: &QString, style: &QString, sizes: &mut QList_i32);
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QList_QString, list)]
        type SysFont = super::FontRust;

        #[qinvokable]
        fn refresh(self: Pin<&mut Self>);

        #[qinvokable]
        fn change_sans_serif(self: Pin<&mut Self>, preferred: QString, mono: bool);

        #[qinvokable]
        fn change_mono(self: Pin<&mut Self>, preferred: QString);

        #[qinvokable]
        fn change_serif(self: Pin<&mut Self>, preferred: QString, mono: bool);
    }

    impl cxx_qt::Constructor<()> for SysFont {}
    impl cxx_qt::Threading for SysFont {}
}

pub struct FontRust {
    pub list: QList<QString>,
}

impl Default for FontRust {
    fn default() -> Self {
        Self {
            list: QList::<QString>::default(),
        }
    }
}

impl cxx_qt::Initialize for font::SysFont {
    fn initialize(mut self: Pin<&mut Self>) {
        self.as_mut().refresh();
    }
}

impl font::SysFont {
    fn refresh(self: Pin<&mut Self>) {
        let qt_thread = self.qt_thread();
        thread::spawn(move || {
            let mut families = QStringList::default();
            font::system_font_families(&mut families);

            let mut tree = serde_json::Map::new();
            let mut list = QList::<QString>::default();

            let mono_keywords = [
                "mono",
                "code",
                "console",
                "terminal",
                "courier",
                "proggy",
                "dejavu sans mono",
                "liberation mono",
                "noto sans mono",
                "fira code",
                "iosevka",
                "jetbrains",
            ];
            let serif_keywords = [
                "serif",
                "noto serif",
                "dejavu serif",
                "liberation serif",
                "tex gyre pagella",
                "tex gyre schola",
                "tex gyre termes",
            ];

            for family in families.iter() {
                let family_lower = family.to_string().to_lowercase();

                let category = if mono_keywords.iter().any(|k| family_lower.contains(k)) {
                    "monospace"
                } else if serif_keywords.iter().any(|k| family_lower.contains(k)) {
                    "serif"
                } else {
                    "sans-serif"
                };

                let is_mono = mono_keywords.iter().any(|k| family_lower.contains(k));

                let obj = serde_json::json!({
                    "family": category,
                    "name": family.to_string(),
                    "mono": is_mono
                });
                list.append(QString::from(&obj.to_string()));

                let mut styles_json = serde_json::Map::new();
                let mut styles = QStringList::default();
                font::system_font_styles(family, &mut styles);
                for style in styles.iter() {
                    let mut sizes = QList::<i32>::default();
                    font::system_font_sizes(family, style, &mut sizes);
                    styles_json.insert(
                        style.to_string(),
                        serde_json::json!(sizes.iter().map(|s| *s).collect::<Vec<_>>()),
                    );
                }
                tree.insert(family.to_string(), serde_json::Value::Object(styles_json));
            }

            let _ = qt_thread.queue(move |mut this| {
                let _ = this.as_mut().set_list(list);
            });
        });
    }

    fn change_sans_serif(self: Pin<&mut Self>, preferred: QString, mono: bool) {
        Self::write_fontconfig("sans-serif", &preferred.to_string(), mono);
    }

    fn change_mono(self: Pin<&mut Self>, preferred: QString) {
        Self::write_fontconfig("monospace", &preferred.to_string(), false);
    }

    fn change_serif(self: Pin<&mut Self>, preferred: QString, mono: bool) {
        Self::write_fontconfig("serif", &preferred.to_string(), mono);
    }

    fn write_fontconfig(family: &str, preferred: &str, mono: bool) {
        let Some(home) = dirs::home_dir() else {
            eprintln!("Could not determine home directory");
            return;
        };

        let fontconfig_dir = home.join(".config").join("fontconfig");
        if fs::create_dir_all(&fontconfig_dir).is_err() {
            eprintln!("Failed to create fontconfig directory");
            return;
        }

        let conf_path = fontconfig_dir.join("fonts.conf");

        let mono_alias = if mono {
            format!(
                r#"
  <alias>
    <family>monospace</family>
    <prefer><family>{preferred}</family></prefer>
  </alias>"#
            )
        } else {
            String::new()
        };

        let conf_content = format!(
            r#"<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <alias>
    <family>{family}</family>
    <prefer><family>{preferred}</family></prefer>
  </alias>{mono_alias}
</fontconfig>"#
        );

        if fs::write(&conf_path, conf_content).is_err() {
            eprintln!("Failed to write fonts.conf");
            return;
        }

        let _ = Command::new("fc-cache").args(["-fv"]).status();
    }
}
