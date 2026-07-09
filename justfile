QML_DIR := "$HOME/.local/share/qt6/qml"
out := QML_DIR + "/System"

build:
    cargo build --release
    mkdir -p {{out}}
    cp target/release/libsystem.so {{out}}/libSystem.so
    cp target/cxxqt/qml_modules/System/qmldir {{out}}/qmldir
    cp target/cxxqt/qml_modules/System/plugin.qmltypes {{out}}/plugin.qmltypes
