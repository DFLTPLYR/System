use std::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QMap, QMapPair_QString_QVariant, QString, QVariant};

use widgets::{QQmlPropertyMap, ws_create, ws_insert, ws_seed, ws_unwrap, ws_wrap};

#[cxx_qt::bridge]
mod widgets {
    unsafe extern "C++Qt" {
        include!("cxx-qt-lib/qstring.h");
        include!("cxx-qt-lib/qmap.h");
        include!("cxx-qt-lib/qvariant.h");
        type QString = cxx_qt_lib::QString;
        type QVariant = cxx_qt_lib::QVariant;
        type QMap_QString_QVariant = cxx_qt_lib::QMap<cxx_qt_lib::QMapPair_QString_QVariant>;
    }

    unsafe extern "C++" {
        include!("widgets_helper.h");
        type QQmlPropertyMap;

        #[namespace = "rust::widgetstore"]
        fn ws_create() -> *mut QQmlPropertyMap;
        #[namespace = "rust::widgetstore"]
        unsafe fn ws_wrap(map: *mut QQmlPropertyMap) -> QVariant;
        #[namespace = "rust::widgetstore"]
        fn ws_unwrap(variant: &QVariant) -> *mut QQmlPropertyMap;
        #[namespace = "rust::widgetstore"]
        unsafe fn ws_insert(map: *mut QQmlPropertyMap, key: &QString, value: &QVariant);
        #[namespace = "rust::widgetstore"]
        unsafe fn ws_seed(map: *mut QQmlPropertyMap, props: &QMap_QString_QVariant);
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QMap_QString_QVariant, instances)]
        type WidgetStore = super::WidgetStoreRust;

        #[qinvokable]
        fn get_shared_instance(
            self: Pin<&mut Self>,
            path: &QString,
            defaults: &QMap_QString_QVariant,
        ) -> QVariant;

        #[qinvokable]
        fn widget_props(self: &Self, path: &QString) -> QVariant;

        #[qinvokable]
        fn set_widget_prop(self: Pin<&mut Self>, path: &QString, key: &QString, value: &QVariant);

        #[qinvokable]
        fn seed_shared_instance(
            self: Pin<&mut Self>,
            path: &QString,
            props: &QMap_QString_QVariant,
        );
    }

    impl cxx_qt::Constructor<()> for WidgetStore {}
}

impl cxx_qt::Initialize for widgets::WidgetStore {
    fn initialize(self: Pin<&mut Self>) {}
}

pub struct WidgetStoreRust {
    pub instances: QMap<QMapPair_QString_QVariant>,
}

impl Default for WidgetStoreRust {
    fn default() -> Self {
        Self {
            instances: QMap::default(),
        }
    }
}

fn norm_key(path: &QString) -> QString {
    let s = path.to_string();
    let base = s.rsplit('/').next().unwrap_or(&s);
    let base = base
        .strip_suffix(".desktop.qml")
        .or_else(|| base.strip_suffix(".dock.qml"))
        .or_else(|| base.strip_suffix(".qml"))
        .unwrap_or(base);
    QString::from(base)
}

impl widgets::WidgetStore {
    fn get_shared_instance(
        mut self: Pin<&mut Self>,
        path: &QString,
        defaults: &QMap<QMapPair_QString_QVariant>,
    ) -> QVariant {
        let path = norm_key(path);
        if let Some(existing) = self.rust().instances.get(&path) {
            return existing.clone();
        }
        let map = unsafe { ws_create() };
        unsafe { ws_seed(map, defaults) };
        let wrapped = unsafe { ws_wrap(map) };
        let mut next = self.rust().instances.clone();
        next.insert_clone(&path, &wrapped);
        self.as_mut().set_instances(next);
        wrapped
    }

    fn widget_props(&self, path: &QString) -> QVariant {
        self.rust()
            .instances
            .get(&norm_key(path))
            .unwrap_or_default()
    }

    fn set_widget_prop(mut self: Pin<&mut Self>, path: &QString, key: &QString, value: &QVariant) {
        let path = norm_key(path);
        let map = self
            .rust()
            .instances
            .get(&path)
            .map(|v| unsafe { ws_unwrap(&v) })
            .unwrap_or(std::ptr::null_mut());
        unsafe { ws_insert(map, key, value) };
    }

    fn seed_shared_instance(
        mut self: Pin<&mut Self>,
        path: &QString,
        props: &QMap<QMapPair_QString_QVariant>,
    ) {
        if props.is_empty() {
            return;
        }
        let path = norm_key(path);
        if self.rust().instances.contains(&path) {
            return;
        }
        let map = unsafe { ws_create() };
        unsafe { ws_seed(map, props) };
        let mut next = self.rust().instances.clone();
        next.insert_clone(&path, &unsafe { ws_wrap(map) });
        self.as_mut().set_instances(next);
    }
}
