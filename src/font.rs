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
        fn system_default_font(family: &mut QString);
        fn system_set_application_font(family: &QString, pointSize: i32);
        fn system_font_is_monospace(family: &QString) -> bool;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QList_QString, list)]
        #[qproperty(QString, current)]
        #[qproperty(QString, app_font_family)]
        #[qproperty(i32, app_font_size)]
        type SysFont = super::FontRust;

        #[qinvokable]
        fn refresh(self: Pin<&mut Self>);

        #[qinvokable]
        fn apply(self: Pin<&mut Self>, family: QString, pointSize: i32, category: QString);
    }

    impl cxx_qt::Constructor<()> for SysFont {}
    impl cxx_qt::Threading for SysFont {}
}

pub struct FontRust {
    pub list: QList<QString>,
    pub current: QString,
    pub app_font_family: QString,
    pub app_font_size: i32,
}

impl Default for FontRust {
    fn default() -> Self {
        Self {
            list: QList::<QString>::default(),
            current: QString::default(),
            app_font_family: QString::default(),
            app_font_size: 12,
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

            for family in families.iter() {
                let is_mono = font::system_font_is_monospace(&family);

                let category = if is_mono { "monospace" } else { "sans-serif" };

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
                let mut default_family = QString::default();
                font::system_default_font(&mut default_family);
                let _ = this.as_mut().set_current(default_family);
                let _ = this.as_mut().set_list(list);
            });
        });
    }

    fn apply(mut self: Pin<&mut Self>, family: QString, point_size: i32, category: QString) {
        let category_str = category.to_string();
        let family_str = family.to_string();

        match category_str.as_str() {
            "monospace" => Self::update_fontconfig_alias("monospace", &family_str),
            "serif" => Self::update_fontconfig_alias("serif", &family_str),
            _ => Self::update_fontconfig_alias("sans-serif", &family_str),
        }

        font::system_set_application_font(&family, point_size);
        let _ = self.as_mut().set_app_font_family(family);
        let _ = self.as_mut().set_app_font_size(point_size);
    }

    fn update_fontconfig_alias(family: &str, preferred: &str) {
        let Some(home) = dirs::home_dir() else {
            eprintln!("Could not determine home directory");
            return;
        };

        let fontconfig_dir = home.join(".config").join("fontconfig");
        let conf_path = fontconfig_dir.join("fonts.conf");

        let new_alias = format!(
            r#" <alias>
                    <family>{family}</family>
                    <prefer><family>{preferred}</family></prefer>
                </alias>"#
        );

        let content = fs::read_to_string(&conf_path).unwrap_or_default();

        let family_tag = format!("    <family>{family}</family>");
        let new_content = if let Some(start) = content.find(&family_tag) {
            let alias_open = content[..start].rfind("<alias>").unwrap_or(0);
            let alias_close = content[start..]
                .find("</alias>")
                .map(|i| start + i + 8)
                .unwrap_or(content.len());
            let mut result = content[..alias_open].to_string();
            result.push_str(&new_alias);
            result.push_str(&content[alias_close..]);
            result
        } else if content.contains("</fontconfig>") {
            let pos = content.rfind("</fontconfig>").unwrap();
            let mut result = content[..pos].to_string();
            if !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(&new_alias);
            result.push('\n');
            result.push_str(&content[pos..]);
            result
        } else if content.is_empty() {
            format!(
                r#" <?xml version="1.0"?>
                        <!DOCTYPE fontconfig SYSTEM "fonts.dtd">
                        <fontconfig>
                        {new_alias}
                    </fontconfig>"#
            )
        } else {
            content
        };

        if fs::create_dir_all(&fontconfig_dir).is_err() {
            eprintln!("Failed to create fontconfig directory");
            return;
        }

        if fs::write(&conf_path, new_content).is_err() {
            eprintln!("Failed to write fonts.conf");
            return;
        }

        let _ = Command::new("fc-cache").args(["-fv"]).status();
    }
}
