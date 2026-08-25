#pragma once

#ifndef WIDGETS_HELPER_H
#define WIDGETS_HELPER_H

#include <QObject>
#include <QQmlPropertyMap>
#include <QString>
#include <QVariant>

namespace rust::widgetstore {

// Creates a QtObject whose dynamic keys are real, enumerable QML properties.
inline QObject* ws_create() {
    return QQmlPropertyMap::create();
}

// Wraps a QObject so it can cross the cxx boundary / reach QML.
inline QVariant ws_wrap(QObject* object) {
    return QVariant::fromValue<QObject*>(object);
}

// Writes a dynamic property onto any QObject handed over from QML. Uses
// QQmlPropertyMap::insert() when possible so the key becomes a proper
// property (visible/bindable); falls back to setProperty() otherwise.
inline void ws_set_property(const QVariant& target, const QString& key, const QVariant& value) {
    QObject* object = target.value<QObject*>();
    if (!object || key.isEmpty())
        return;
    if (auto* map = qobject_cast<QQmlPropertyMap*>(object))
        map->insert(key, value);
    else
        object->setProperty(key.toUtf8().constData(), value);
}

} // namespace rust::widgetstore

#endif // WIDGETS_HELPER_H
