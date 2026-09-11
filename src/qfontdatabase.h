#pragma once

#include <QFont>
#include <QFontDatabase>
#include <QGuiApplication>
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

inline void system_default_font(QString &family)
{
    family = QFontDatabase::systemFont(QFontDatabase::GeneralFont).family();
}

inline void system_set_application_font(const QString &family, int pointSize)
{
    auto *app = qGuiApp;
    if (!app)
        return;

    QString trimmed = family.trimmed();
    if (trimmed.isEmpty())
        return;

    QFont font(trimmed);
    if (pointSize > 0)
        font.setPointSize(pointSize);
    app->setFont(font);
}

inline bool system_font_is_monospace(const QString &family)
{
    // NOTE: QFont(family).fixedPitch() only reflects the flag set on that
    // QFont instance (false by default) — it does NOT query the font
    // database. QFontDatabase::isFixedPitch() is the correct query.
    return QFontDatabase::isFixedPitch(family);
}
