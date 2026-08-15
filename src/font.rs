use std::pin::Pin;
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
        #[qproperty(QString, families_json)]
        type SysFont = super::FontRust;

        #[qinvokable]
        fn refresh(self: Pin<&mut Self>);
    }

    impl cxx_qt::Constructor<()> for SysFont {}
    impl cxx_qt::Threading for SysFont {}
}

pub struct FontRust {
    pub list: QList<QString>,
    pub families_json: QString,
}

impl Default for FontRust {
    fn default() -> Self {
        Self {
            list: QList::<QString>::default(),
            families_json: QString::default(),
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
            for family in families.iter() {
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

            let json = serde_json::Value::Object(tree).to_string();
            let list: QList<QString> = (&families).into();
            let _ = qt_thread.queue(move |mut this| {
                let _ = this.as_mut().set_list(list);
                let _ = this.as_mut().set_families_json(QString::from(&json));
            });
        });
    }
}