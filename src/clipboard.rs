use std::io::Read;
use std::pin::Pin;
use std::sync::Mutex;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QList, QString};
use once_cell::sync::Lazy;

use wl_clipboard_rs::copy::{MimeType as CopyMime, Options as CopyOptions, Source};
use wl_clipboard_rs::paste::{get_contents, ClipboardType, Error as PasteError, MimeType as PasteMime, Seat};
use wl_clipboard_rs::utils::{is_primary_selection_supported, PrimarySelectionCheckError};

static CLIPBOARD: Lazy<Mutex<ClipboardState>> = Lazy::new(|| {
    Mutex::new(ClipboardState::new())
});

const MAX_HISTORY: usize = 50;

#[derive(Clone)]
struct ClipboardEntry {
    content: String,
    pinned: bool,
}

struct ClipboardState {
    history: Vec<ClipboardEntry>,
    last_known: String,
}

impl ClipboardState {
    fn new() -> Self {
        Self {
            history: Vec::with_capacity(MAX_HISTORY),
            last_known: String::new(),
        }
    }

    fn poll(&mut self) {
        let current = paste_text();
        if let Some(ref text) = current {
            if *text != self.last_known {
                self.last_known = text.clone();
                self.push_internal(text.clone());
            }
        }
    }

    fn push_internal(&mut self, content: String) {
        if let Some(pos) = self.history.iter().position(|e| e.content == content) {
            let entry = self.history.remove(pos);
            self.history.insert(0, entry);
            return;
        }
        self.history.insert(0, ClipboardEntry { content, pinned: false });
        while self.history.len() > MAX_HISTORY {
            if let Some(pos) = self.history.iter().rposition(|e| !e.pinned) {
                self.history.remove(pos);
            } else {
                break;
            }
        }
    }
}

fn paste_text() -> Option<String> {
    match get_contents(ClipboardType::Regular, Seat::Unspecified, PasteMime::Text) {
        Ok((mut pipe, _)) => {
            let mut contents = vec![];
            pipe.read_to_end(&mut contents).ok()?;
            String::from_utf8(contents).ok()
        }
        Err(PasteError::NoSeats)
        | Err(PasteError::ClipboardEmpty)
        | Err(PasteError::NoMimeType) => None,
        Err(_) => None,
    }
}

fn copy_text(text: &str) -> Result<(), String> {
    let opts = CopyOptions::new();
    opts.copy(
        Source::Bytes(text.as_bytes().to_vec().into()),
        CopyMime::Autodetect,
    )
    .map_err(|e| format!("{}", e))
}

fn history_json() -> QString {
    let state = CLIPBOARD.lock().unwrap();
    let entries: Vec<serde_json::Value> = state
        .history
        .iter()
        .map(|e| {
            serde_json::json!({
                "content": e.content,
                "pinned": e.pinned,
            })
        })
        .collect();
    QString::from(&serde_json::to_string(&entries).unwrap_or_default())
}

fn history_count() -> i32 {
    CLIPBOARD.lock().unwrap().history.len() as i32
}

fn history_at(index: i32) -> QString {
    let state = CLIPBOARD.lock().unwrap();
    state
        .history
        .get(index as usize)
        .map(|e| QString::from(&e.content))
        .unwrap_or_default()
}

fn is_pinned(index: i32) -> bool {
    let state = CLIPBOARD.lock().unwrap();
    state
        .history
        .get(index as usize)
        .map(|e| e.pinned)
        .unwrap_or(false)
}

fn copy_to_clipboard(text: &str) -> bool {
    let mut state = CLIPBOARD.lock().unwrap();
    state.push_internal(text.to_string());
    state.last_known = text.to_string();
    drop(state);
    copy_text(text).is_ok()
}

fn delete_entry(index: i32) -> bool {
    let mut state = CLIPBOARD.lock().unwrap();
    if index >= 0 && (index as usize) < state.history.len() {
        state.history.remove(index as usize);
        true
    } else {
        false
    }
}

fn pin_entry(index: i32, pinned: bool) -> bool {
    let mut state = CLIPBOARD.lock().unwrap();
    if let Some(entry) = state.history.get_mut(index as usize) {
        entry.pinned = pinned;
        true
    } else {
        false
    }
}

fn clear_history() {
    let mut state = CLIPBOARD.lock().unwrap();
    state.history.retain(|e| e.pinned);
}

fn swap_entries(i: i32, j: i32) -> bool {
    let mut state = CLIPBOARD.lock().unwrap();
    let len = state.history.len();
    if i >= 0 && (i as usize) < len && j >= 0 && (j as usize) < len {
        state.history.swap(i as usize, j as usize);
        true
    } else {
        false
    }
}

fn refresh() {
    let mut state = CLIPBOARD.lock().unwrap();
    state.poll();
}

fn primary_supported() -> i32 {
    match is_primary_selection_supported() {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(PrimarySelectionCheckError::NoSeats) => -1,
        Err(PrimarySelectionCheckError::MissingProtocol) => -2,
        Err(_) => -3,
    }
}

#[cxx_qt::bridge]
mod clipboard {
    extern "C++Qt" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qlist.h");
        type QList_QString = cxx_qt_lib::QList<QString>;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, history_json)]
        #[qproperty(i32, history_count)]
        type Clipboard = super::ClipboardRust;
    }

    impl cxx_qt::Constructor<()> for Clipboard {}
    impl cxx_qt::Threading for Clipboard {}
}

pub struct ClipboardRust {
    pub history_json: QString,
    pub history_count: i32,
}

impl Default for ClipboardRust {
    fn default() -> Self {
        refresh();
        Self {
            history_json: history_json(),
            history_count: history_count(),
        }
    }
}

impl cxx_qt::Initialize for clipboard::Clipboard {
    fn initialize(self: Pin<&mut Self>) {
        refresh();
        let _ = self.as_mut().set_history_json(history_json());
        let _ = self.as_mut().set_history_count(history_count());
    }
}