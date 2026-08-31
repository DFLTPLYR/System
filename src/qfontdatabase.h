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

    QFont font = app->font();
    font.setFamily(family);
    if (pointSize > 0)
        font.setPointSize(pointSize);
    app->setFont(font);
}
