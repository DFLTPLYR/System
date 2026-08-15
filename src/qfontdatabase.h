#pragma once

#include <QFontDatabase>
#include <QList>
#include <QStringList>

inline void system_font_families(QStringList &families)
{
    families = QFontDatabase::families();
}

inline void system_font_styles(const QString &family, QStringList &styles)
{
    styles = QFontDatabase::styles(family);
}

inline void system_font_sizes(const QString &family, const QString &style, QList<int> &sizes)
{
    sizes = QFontDatabase::smoothSizes(family, style);
}