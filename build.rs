use cxx_qt_build::{CxxQtBuilder, PluginType, QmlModule};
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ver_script = out_dir.join("qt-plugin-exports.map");

    // Version script that makes the Qt plugin entry points GLOBAL
    std::fs::write(
        &ver_script,
        "{\n  global:\n    qt_plugin_instance;\n    qt_plugin_query_metadata_v2;\n};\n",
    )
    .unwrap();
    println!(
        "cargo::rustc-link-arg=-Wl,--version-script={}",
        ver_script.display()
    );

    CxxQtBuilder::new_qml_module(QmlModule::new("System").plugin_type(PluginType::Dynamic))
        .file("src/colorscheme.rs")
        .file("src/hardware.rs")
        .file("src/weather.rs")
        .build();
}
