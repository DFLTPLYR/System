use std::pin::Pin;

use cxx_qt_lib::{QMap, QMapPair_QString_QVariant, QString};
use notify_rust::Notification;

#[cxx_qt::bridge]
mod notification {
    extern "C++Qt" {
        include!("cxx-qt-lib/qstring.h");
        include!("cxx-qt-lib/qmap.h");
        include!("cxx-qt-lib/qvariant.h");
        type QString = cxx_qt_lib::QString;
        type QMap_QString_QVariant =
            cxx_qt_lib::QMap<cxx_qt_lib::QMapPair_QString_QVariant>;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        type Notification = super::NotificationRust;

        #[qinvokable]
        fn send(self: Pin<&mut Self>, args: QMap_QString_QVariant);
    }

    impl cxx_qt::Constructor<()> for Notification {}
}

pub struct NotificationRust;

impl Default for NotificationRust {
    fn default() -> Self {
        Self
    }
}

impl cxx_qt::Initialize for notification::Notification {
    fn initialize(self: Pin<&mut Self>) {}
}

impl notification::Notification {
    fn send(self: Pin<&mut Self>, args: QMap<QMapPair_QString_QVariant>) {
        let get = |key: &str| {
            args.get(&QString::from(key))
                .and_then(|v| v.value::<QString>())
                .map(|s| s.to_string())
        };

        let mut n = Notification::new();
        if let Some(appname) = get("appname") {
            n.appname(&appname);
        }
        if let Some(summary) = get("title").or_else(|| get("summary")) {
            n.summary(&summary);
        }
        if let Some(body) = get("body") {
            n.body(&body);
        }
        if let Some(icon) = get("icon") {
            n.icon(&icon);
        }
        if let Some(timeout) = args
            .get(&QString::from("timeout"))
            .and_then(|v| v.value::<i32>())
        {
            n.timeout(timeout);
        }

        let _ = n.show();
    }
}