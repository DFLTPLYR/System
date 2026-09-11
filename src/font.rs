use std::fs;
use std::pin::Pin;
use std::thread;

use cxx_qt::Threading;
use cxx_qt_lib::{QList, QMap, QMapPair_QString_QVariant, QString, QStringList, QVariant};

#[cxx_qt::bridge]
mod font {
    extern "C++Qt" {
        include!("cxx-qt-lib/qstring.h");
        include!("cxx-qt-lib/qstringlist.h");
        include!("cxx-qt-lib/qlist.h");
        include!("cxx-qt-lib/qvariant.h");
        include!("cxx-qt-lib/qmap.h");
        type QString = cxx_qt_lib::QString;
        type QStringList = cxx_qt_lib::QStringList;
        type QVariant = cxx_qt_lib::QVariant;
        type QList_QVariant = cxx_qt_lib::QList<QVariant>;
        type QList_i32 = cxx_qt_lib::QList<i32>;
    }

    unsafe extern "C++" {
        include!("system/src/qfontdatabase.h");
        fn system_font_families(families: &mut QStringList);
        fn system_font_styles(family: &QString, styles: &mut QStringList);
        fn system_font_sizes(family: &QString, style: &QString, sizes: &mut QList_i32);
        fn system_default_font(family: &mut QString);
        fn system_set_application_font(family: &QString, pointSize: i32);
        fn system_font_is_monospace(family: &QString) -> bool;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QList_QVariant, list)]
        #[qproperty(QString, current)]
        #[qproperty(QString, app_font_family)]
        #[qproperty(i32, app_font_size)]
        type SysFont = super::FontRust;

        #[qinvokable]
        fn refresh(self: Pin<&mut Self>);

        #[qinvokable]
        fn apply(self: Pin<&mut Self>, family: QVariant, pointSize: i32, category: QString);
    }

    impl cxx_qt::Constructor<()> for SysFont {}
    impl cxx_qt::Threading for SysFont {}
}

pub struct FontRust {
    pub list: QList<QVariant>,
    pub current: QString,
    pub app_font_family: QString,
    pub app_font_size: i32,
}

impl Default for FontRust {
    fn default() -> Self {
        Self {
            list: QList::<QVariant>::default(),
            current: QString::default(),
            app_font_family: QString::default(),
            app_font_size: 12,
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
            let mut list = QList::<QVariant>::default();

            for family in families.iter() {
                let is_mono = font::system_font_is_monospace(&family);

                let category = if is_mono { "monospace" } else { "sans-serif" };
                let family_name = family.to_string();

                // One entry per font family, exposed to QML as a JS object:
                // { name, family, families, category, mono }
                // so QML can do:
                //   SysFont.list.filter(s => s.category === "monospace")
                //   SysFont.list.filter(s => s.family === "Inter")
                //   SysFont.list.filter(s => s.families === "Inter")
                let mut map = QMap::<QMapPair_QString_QVariant>::default();
                let q_name = QString::from(&family_name);
                let q_category = QString::from(category);
                map.insert_clone(
                    &QString::from("name"),
                    &QVariant::from(&q_name),
                );
                // `family` is the Qt term for the font name
                // (QFontDatabase::families()).
                map.insert_clone(&QString::from("family"), &QVariant::from(&q_name));
                // `families` alias so `s.families` filtering works too.
                map.insert_clone(
                    &QString::from("families"),
                    &QVariant::from(&q_name),
                );
                map.insert_clone(
                    &QString::from("category"),
                    &QVariant::from(&q_category),
                );
                map.insert_clone(
                    &QString::from("mono"),
                    &QVariant::from(&is_mono),
                );
                list.append(QVariant::from(&map));

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

            let persisted_sans = Self::persisted_preferred("sans-serif");

            let _ = qt_thread.queue(move |mut this| {
                let mut default_family = QString::default();
                font::system_default_font(&mut default_family);
                // After a reload the runtime app font is gone; restore the
                // user's persisted sans-serif choice so `current`,
                // `app_font_family`, and the actual default font stay in sync
                // instead of falling back to DejaVu Sans.
                if let Some(persisted) = persisted_sans {
                    let q = QString::from(&persisted);
                    font::system_set_application_font(&q, 12);
                    let _ = this.as_mut().set_current(q.clone());
                    let _ = this.as_mut().set_app_font_family(q);
                    let _ = this.as_mut().set_app_font_size(12);
                } else {
                    let _ = this.as_mut().set_current(default_family);
                }
                let _ = this.as_mut().set_list(list);
            });
        });
    }

    fn apply(mut self: Pin<&mut Self>, family: QVariant, point_size: i32, category: QString) {
        // `family` may be a plain string ("Inter"), a legacy JSON string
        // ('{"name":"Inter",...}'), or a full list entry object
        // ({name, family, families, category, mono}) when QML passes
        // `SysFont.list[i]` directly.
        let family_str = Self::extract_family_name(&family);
        if family_str.is_empty() {
            eprintln!("SysFont.apply: empty family name, ignoring");
            return;
        }
        let family_q = QString::from(&family_str);

        let category_str = category.to_string().to_lowercase();
        let generic = match category_str.as_str() {
            "monospace" | "mono" => "monospace",
            "serif" => "serif",
            _ => "sans-serif",
        };

        Self::update_fontconfig_alias(generic, &family_str);

        font::system_set_application_font(&family_q, point_size);
        let _ = self.as_mut().set_app_font_family(family_q.clone());
        let _ = self.as_mut().set_app_font_size(point_size);
        // `current` backs the QML preview (`font.family: ... || SysFont.current`)
        // and the onCurrentChanged logger. QFontDatabase::systemFont() never
        // reflects the user's choice, so publish it here.
        let _ = self.as_mut().set_current(family_q);
    }

    /// Pull a font family name out of an `apply()` argument.
    ///
    /// Accepts a plain family name, a legacy JSON-string entry
    /// (`{"name":...}` from the old `QList<QString>` model), or a full
    /// `SysFont.list` object (`{name, family, families, ...}` as a
    /// `QVariantMap`).
    fn extract_family_name(family: &QVariant) -> String {
        // QML object -> QVariantMap.
        if let Some(map) = family.value::<QMap<QMapPair_QString_QVariant>>() {
            for key in ["name", "family", "families"] {
                if let Some(field) = map.get(&QString::from(key)) {
                    if let Some(s) = field.value::<QString>() {
                        let s = s.to_string();
                        if !s.trim().is_empty() {
                            return s.trim().to_string();
                        }
                    }
                }
            }
            return String::new();
        }
        // Plain string or legacy JSON string.
        if let Some(s) = family.value::<QString>() {
            let s_str = s.to_string();
            let trimmed = s_str.trim();
            if trimmed.starts_with('{') {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                    for key in ["name", "family", "families"] {
                        if let Some(name) = v.get(key).and_then(|n| n.as_str()) {
                            if !name.trim().is_empty() {
                                // Old model stored the category in `family`,
                                // so prefer `name` (checked first) over it.
                                return name.trim().to_string();
                            }
                        }
                    }
                }
            }
            return trimmed.to_string();
        }
        String::new()
    }

    /// Best-effort read of the preferred family for `generic` from the user's
    /// fontconfig file. Understands both the current `<match>` format written
    /// by this plugin and legacy `<alias>` blocks (ours or other tools').
    fn persisted_preferred(generic: &str) -> Option<String> {
        let home = dirs::home_dir()?;
        let content = fs::read_to_string(home.join(".config").join("fontconfig").join("fonts.conf")).ok()?;
        Self::extract_preferred(&content, generic)
    }

    fn extract_preferred(content: &str, generic: &str) -> Option<String> {
        use regex::Regex;
        let esc = regex::escape(generic);
        // New format: <match ...><test ... name="family">...<string>GENERIC</string>...<edit ... name="family">...<string>PREF</string>
        if let Ok(match_re) = Regex::new(r"(?s)<match\b[^>]*>.*?</match>") {
            for m in match_re.find_iter(content) {
                let block = m.as_str();
                let test_re = Regex::new(&format!(
                    r#"(?s)<test\b[^>]*name\s*=\s*"family"[^>]*>.*?<string>\s*{esc}\s*</string>"#
                ));
                if !test_re.map(|re| re.is_match(block)).unwrap_or(false) {
                    continue;
                }
                let edit_re = Regex::new(r#"(?s)<edit\b[^>]*name\s*=\s*"family"[^>]*>.*?<string>\s*(.*?)\s*</string>"#);
                if let Ok(re) = edit_re {
                    if let Some(cap) = re.captures(block) {
                        let pref = cap[1].trim().to_string();
                        if !pref.is_empty() {
                            return Some(pref);
                        }
                    }
                }
            }
        }
        // Legacy format: <alias ...><family>GENERIC</family>...<prefer><family>PREF</family></prefer></alias>
        if let Ok(alias_re) = Regex::new(r"(?s)<alias\b[^>]*>.*?</alias>") {
            for m in alias_re.find_iter(content) {
                let block = m.as_str();
                let fam_re = Regex::new(&format!(r"<family>\s*{esc}\s*</family>"));
                if !fam_re.map(|re| re.is_match(block)).unwrap_or(false) {
                    continue;
                }
                let pref_re = Regex::new(r"(?s)<prefer>.*?<family>\s*(.*?)\s*</family>");
                if let Ok(re) = pref_re {
                    if let Some(cap) = re.captures(block) {
                        let pref = cap[1].trim().to_string();
                        if !pref.is_empty() {
                            return Some(pref);
                        }
                    }
                }
            }
        }
        None
    }

    fn update_fontconfig_alias(generic: &str, preferred: &str) {
        let generic = match generic {
            "monospace" | "serif" | "sans-serif" => generic,
            _ => "sans-serif",
        };
        let Some(home) = dirs::home_dir() else {
            eprintln!("Could not determine home directory");
            return;
        };

        let fontconfig_dir = home.join(".config").join("fontconfig");
        let conf_path = fontconfig_dir.join("fonts.conf");

        let content = fs::read_to_string(&conf_path).unwrap_or_default();

        let Some(new_content) = upsert_fontconfig_block(&content, generic, preferred) else {
            eprintln!("fonts.conf is malformed (no </fontconfig>), refusing to overwrite");
            return;
        };

        if fs::create_dir_all(&fontconfig_dir).is_err() {
            eprintln!("Failed to create fontconfig directory");
            return;
        }

        if fs::write(&conf_path, new_content).is_err() {
            eprintln!("Failed to write fonts.conf");
            return;
        }

        // No fc-cache call: fontconfig picks up config-file changes on its own
        // (it stats the files), and `fc-cache -fv` only rebuilds the font-file
        // cache while blocking the Qt GUI thread for seconds.
    }
}

/// XML-escape a font family name for embedding in fonts.conf.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Insert/replace the `<match>` block for `generic` in a fonts.conf document.
///
/// Removes any pre-existing block for the same generic (both the current
/// `<match>` style and legacy `<alias>` style) so repeated applies are
/// byte-identical instead of stacking duplicates. Returns `None` when the
/// existing content is non-empty but has no `</fontconfig>` (malformed) and
/// must not be overwritten.
fn upsert_fontconfig_block(content: &str, generic: &str, preferred: &str) -> Option<String> {
    use regex::Regex;
    // NOTE: a plain <alias><prefer> in the user config is loaded at
    // 50-user.conf, *before* NixOS 52-nixos-default-fonts.conf, so the
    // system DejaVu default wins and fc-match keeps returning DejaVu Sans.
    // A <match> with mode="prepend" binding="strong" overrides it
    // (verified: fc-match then resolves to the chosen family).
    let new_block = format!(
        "  <match target=\"pattern\">\n    <test qual=\"any\" name=\"family\"><string>{}</string></test>\n    <edit name=\"family\" mode=\"prepend\" binding=\"strong\"><string>{}</string></edit>\n  </match>",
        xml_escape(generic),
        xml_escape(preferred)
    );

    let mut cleaned = content.to_string();
    let esc = regex::escape(generic);
    if let Ok(re) = Regex::new(r"(?s)<match\b[^>]*>.*?</match>") {
        // Only drop match blocks that actually target this generic.
        if let Ok(test_re) = Regex::new(&format!(
            r#"(?s)<test\b[^>]*name\s*=\s*"family"[^>]*>.*?<string>\s*{esc}\s*</string>"#
        )) {
            cleaned = re
                .replace_all(&cleaned, |caps: &regex::Captures| {
                    if test_re.is_match(&caps[0]) {
                        String::new()
                    } else {
                        caps[0].to_string()
                    }
                })
                .into_owned();
        }
    }
    if let Ok(re) = Regex::new(r"(?s)<alias\b[^>]*>.*?</alias>") {
        if let Ok(fam_re) = Regex::new(&format!(r"<family>\s*{esc}\s*</family>")) {
            cleaned = re
                .replace_all(&cleaned, |caps: &regex::Captures| {
                    if fam_re.is_match(&caps[0]) {
                        String::new()
                    } else {
                        caps[0].to_string()
                    }
                })
                .into_owned();
        }
    }
    // Collapse runs of blank/whitespace-only lines left by removals so
    // repeated applies converge instead of stacking stray indent.
    if let Ok(re) = Regex::new(r"(?m)[ \t]+$") {
        cleaned = re.replace_all(&cleaned, "").into_owned();
    }
    if let Ok(re) = Regex::new(r"\n(?:[ \t]*\n){2,}") {
        cleaned = re.replace_all(&cleaned, "\n\n").into_owned();
    }

    if let Some(pos) = cleaned.rfind("</fontconfig>") {
        let mut result = cleaned[..pos].to_string();
        if !result.ends_with('\n') {
            result.push('\n');
        }
        // Keep one blank line before the block so repeated applies are
        // byte-identical (insert and re-insert converge).
        if !result.ends_with("\n\n") {
            result.push('\n');
        }
        result.push_str(&new_block);
        result.push('\n');
        result.push_str(&cleaned[pos..]);
        Some(result)
    } else if cleaned.trim().is_empty() {
        Some(format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n<fontconfig>\n{new_block}\n</fontconfig>\n"
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::font::SysFont;
    use super::upsert_fontconfig_block;
    use cxx_qt_lib::{QList, QMap, QMapPair_QString_QVariant, QString, QVariant};

    const LEGACY_CONF: &str = "<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n<fontconfig>\n     <alias>\n                    <family>sans-serif</family>\n                    <prefer><family>Fira Code</family></prefer>\n                </alias>\n    <alias binding=\"strong\">\n        <family>monospace</family>\n        <prefer><family>Fira Code</family></prefer>\n    </alias>\n</fontconfig>\n";

    #[test]
    fn replaces_legacy_alias_with_strong_match() {
        let out = upsert_fontconfig_block(LEGACY_CONF, "monospace", "Fira Code").unwrap();
        assert!(out.contains("mode=\"prepend\" binding=\"strong\""));
        assert!(out.contains("<string>monospace</string>"));
        // Legacy monospace alias is gone, unrelated sans-serif alias stays.
        assert!(!out.contains("<alias binding=\"strong\">"));
        assert!(out.contains("<family>sans-serif</family>"));
    }

    #[test]
    fn repeated_apply_is_idempotent() {
        let once = upsert_fontconfig_block(LEGACY_CONF, "monospace", "Fira Code").unwrap();
        let twice = upsert_fontconfig_block(&once, "monospace", "Fira Code").unwrap();
        assert_eq!(once, twice);
        let sans = upsert_fontconfig_block(&twice, "sans-serif", "Inter").unwrap();
        let sans2 = upsert_fontconfig_block(&sans, "sans-serif", "Inter").unwrap();
        assert_eq!(sans, sans2);
    }

    #[test]
    fn switching_family_replaces_not_stacks() {
        let once = upsert_fontconfig_block(LEGACY_CONF, "monospace", "Fira Code").unwrap();
        let switched = upsert_fontconfig_block(&once, "monospace", "JetBrainsMono Nerd Font").unwrap();
        assert_eq!(
            SysFont::extract_preferred(&switched, "monospace").as_deref(),
            Some("JetBrainsMono Nerd Font")
        );
        assert_eq!(switched.matches("</match>").count(), 1);
        // Idempotent after switch too.
        let again = upsert_fontconfig_block(&switched, "monospace", "JetBrainsMono Nerd Font").unwrap();
        assert_eq!(switched, again);
    }

    #[test]
    fn creates_document_from_empty() {
        let out = upsert_fontconfig_block("", "sans-serif", "Inter").unwrap();
        assert!(out.contains("<fontconfig>"));
        assert!(out.contains("Inter"));
    }

    #[test]
    fn refuses_malformed_without_closing_tag() {
        assert!(upsert_fontconfig_block("<fontconfig><oops>", "sans-serif", "Inter").is_none());
    }

    #[test]
    fn extract_reads_both_formats() {
        let new_fmt = upsert_fontconfig_block("", "sans-serif", "Inter").unwrap();
        assert_eq!(
            SysFont::extract_preferred(&new_fmt, "sans-serif").as_deref(),
            Some("Inter")
        );
        assert_eq!(
            SysFont::extract_preferred(LEGACY_CONF, "monospace").as_deref(),
            Some("Fira Code")
        );
        assert_eq!(SysFont::extract_preferred(LEGACY_CONF, "serif"), None);
    }

    fn font_entry_variant(name: &str, category: &str, mono: bool) -> QVariant {
        let mut map = QMap::<QMapPair_QString_QVariant>::default();
        let q_name = QString::from(name);
        let q_category = QString::from(category);
        map.insert_clone(&QString::from("name"), &QVariant::from(&q_name));
        map.insert_clone(&QString::from("family"), &QVariant::from(&q_name));
        map.insert_clone(&QString::from("families"), &QVariant::from(&q_name));
        map.insert_clone(
            &QString::from("category"),
            &QVariant::from(&q_category),
        );
        map.insert_clone(&QString::from("mono"), &QVariant::from(&mono));
        QVariant::from(&map)
    }

    #[test]
    fn extract_family_name_from_plain_string() {
        let v = QVariant::from(&QString::from("Inter"));
        assert_eq!(SysFont::extract_family_name(&v), "Inter");
    }

    #[test]
    fn extract_family_name_from_legacy_json_string() {
        let json = r#"{"family":"monospace","name":"Fira Code","mono":true}"#;
        let v = QVariant::from(&QString::from(json));
        assert_eq!(SysFont::extract_family_name(&v), "Fira Code");
    }

    #[test]
    fn extract_family_name_from_object() {
        let v = font_entry_variant("JetBrainsMono Nerd Font", "monospace", true);
        assert_eq!(
            SysFont::extract_family_name(&v),
            "JetBrainsMono Nerd Font"
        );
    }

    #[test]
    fn list_entry_is_real_qvariantmap() {
        // QML only auto-converts to JS objects when the stored type is
        // exactly QVariantMap (see C++ QJSEngine baseline).
        use cxx_qt_lib::QMetaTypeType;
        let v = font_entry_variant("Inter", "sans-serif", false);
        assert_eq!(v.type_id(), QMetaTypeType::QVariantMap);
        let list_var = QVariant::from(&{
            let mut l = QList::<QVariant>::default();
            l.append(font_entry_variant("Inter", "sans-serif", false));
            l
        });
        assert_eq!(list_var.type_id(), QMetaTypeType::QVariantList);
    }

    #[test]
    fn list_entry_exposes_filterable_fields() {
        let mut list = QList::<QVariant>::default();
        list.append(font_entry_variant("Inter", "sans-serif", false));
        list.append(font_entry_variant("Fira Code", "monospace", true));

        // Simulate QML: SysFont.list.filter(s => s.category === "monospace")
        let mut monospace = Vec::new();
        let mut by_name = Vec::new();
        for entry in list.iter() {
            let map = entry
                .value::<QMap<QMapPair_QString_QVariant>>()
                .expect("list entry should be a QVariantMap");
            let category = map
                .get(&QString::from("category"))
                .and_then(|v| v.value::<QString>())
                .map(|s| s.to_string())
                .unwrap_or_default();
            // `families` alias must match what the frontend filters on.
            let families = map
                .get(&QString::from("families"))
                .and_then(|v| v.value::<QString>())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let family = map
                .get(&QString::from("family"))
                .and_then(|v| v.value::<QString>())
                .map(|s| s.to_string())
                .unwrap_or_default();
            if category == "monospace" {
                monospace.push(families.clone());
            }
            if family == "Inter" {
                by_name.push(families);
            }
        }
        assert_eq!(monospace, vec!["Fira Code".to_string()]);
        assert_eq!(by_name, vec!["Inter".to_string()]);
    }
}
