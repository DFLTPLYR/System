#pragma once

#ifndef WIDGETS_HELPER_H
#define WIDGETS_HELPER_H

#include <QMap>
#include <QObject>
#include <QQmlPropertyMap>
#include <QString>
#include <QVariant>

namespace rust::widgetstore {

inline QQmlPropertyMap* ws_create() {
    return QQmlPropertyMap::create();
}

inline QVariant ws_wrap(QQmlPropertyMap* map) {
    return QVariant::fromValue<QObject*>(map);
}

inline QQmlPropertyMap* ws_unwrap(const QVariant& variant) {
    return static_cast<QQmlPropertyMap*>(variant.value<QObject*>());
}

inline void ws_insert(QQmlPropertyMap* map, const QString& key, const QVariant& value) {
    if (map)
        map->insert(key, value);
}

inline void ws_seed(QQmlPropertyMap* map, const QMap<QString, QVariant>& props) {
    if (!map)
        return;
    for (auto it = props.constBegin(); it != props.constEnd(); ++it)
        map->insert(it.key(), it.value());
}

} 

#endif 
