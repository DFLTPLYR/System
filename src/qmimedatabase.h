#pragma once

#include <QMimeDatabase>
#include <QString>

inline QString system_mime_type(const QString &path)
{
    return QMimeDatabase().mimeTypeForFile(path).name();
}