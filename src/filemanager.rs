use std::pin::Pin;
use std::thread;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
mod file_manager {
    extern "C++Qt" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    unsafe extern "C++" {
        include!("system/src/qmimedatabase.h");
        fn system_mime_type(path: &QString) -> QString;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        type FileManager = super::FileManagerRust;

        #[qinvokable]
        fn open(self: Pin<&mut Self>);

        #[qsignal]
        fn output(self: Pin<&mut Self>, path: QString, mime_type: QString);
    }

    impl cxx_qt::Constructor<()> for FileManager {}
    impl cxx_qt::Threading for FileManager {}
}

pub struct FileManagerRust;

impl Default for FileManagerRust {
    fn default() -> Self {
        Self
    }
}

impl cxx_qt::Initialize for file_manager::FileManager {
    fn initialize(self: Pin<&mut Self>) {}
}

impl file_manager::FileManager {
    fn open(self: Pin<&mut Self>) {
        let qt_thread = self.qt_thread();

        thread::spawn(move || {
            let Some(path) = rfd::FileDialog::new()
                .set_title("Pick a file")
                .pick_file()
            else {
                return;
            };
            let path = path.to_string_lossy().to_string();
            if !std::path::Path::new(&path).is_file() {
                return;
            }
            let mime_type = file_manager::system_mime_type(&QString::from(&path)).to_string();

            let _ = qt_thread.queue(move |mut this| {
                this.as_mut()
                    .output(QString::from(&path), QString::from(mime_type));
            });
        });
    }
}