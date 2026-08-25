use std::pin::Pin;

use cxx_qt_lib::{QString, QVariant};

use widgets::{ws_create, ws_set_property, ws_wrap};

#[cxx_qt::bridge]
mod widgets {
    unsafe extern "C++Qt" {
        include!("cxx-qt-lib/qstring.h");
        include!("cxx-qt-lib/qvariant.h");
        type QString = cxx_qt_lib::QString;
        type QVariant = cxx_qt_lib::QVariant;
    }

    unsafe extern "C++" {
        include!("widgets_helper.h");

        #[namespace = "rust::widgetstore"]
        fn ws_create() -> *mut QObject;
        #[namespace = "rust::widgetstore"]
        unsafe fn ws_wrap(object: *mut QObject) -> QVariant;
        #[namespace = "rust::widgetstore"]
        fn ws_set_property(target: &QVariant, key: &QString, value: &QVariant);
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        type Widgets = super::WidgetsRust;

        /// Creates a QtObject and returns it to QML.
        #[qinvokable]
        fn create_object(&self) -> QVariant;

        /// Writes a dynamic property onto the QtObject passed from QML.
        #[qinvokable]
        fn set_property(&self, target: &QVariant, key: &QString, value: &QVariant);
    }

    impl cxx_qt::Constructor<()> for Widgets {}
}

impl cxx_qt::Initialize for widgets::Widgets {
    fn initialize(self: Pin<&mut Self>) {}
}

pub struct WidgetsRust;

impl Default for WidgetsRust {
    fn default() -> Self {
        Self
    }
}

impl widgets::Widgets {
    fn create_object(&self) -> QVariant {
        unsafe { ws_wrap(ws_create()) }
    }

    fn set_property(&self, target: &QVariant, key: &QString, value: &QVariant) {
        ws_set_property(target, key, value)
    }
}
