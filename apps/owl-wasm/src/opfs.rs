//! A file of the browser's own file system, as one file of the pool.
//!
//! The browser gives a worker a handle that reads and writes without
//! waiting (`FileSystemSyncAccessHandle`). The page opens them; this only
//! uses them.

use crate::pool::SlotStorage;
use std::io;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = "
export function owl_read(handle, buffer, at) { return handle.read(buffer, { at }); }
export function owl_write(handle, buffer, at) { return handle.write(buffer, { at }); }
export function owl_truncate(handle, length) { handle.truncate(length); }
export function owl_size(handle) { return handle.getSize(); }
export function owl_flush(handle) { handle.flush(); }
")]
extern "C" {
    #[wasm_bindgen(catch)]
    fn owl_read(handle: &JsValue, buffer: &mut [u8], at: f64) -> Result<f64, JsValue>;
    #[wasm_bindgen(catch)]
    fn owl_write(handle: &JsValue, buffer: &[u8], at: f64) -> Result<f64, JsValue>;
    #[wasm_bindgen(catch)]
    fn owl_truncate(handle: &JsValue, length: f64) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    fn owl_size(handle: &JsValue) -> Result<f64, JsValue>;
    #[wasm_bindgen(catch)]
    fn owl_flush(handle: &JsValue) -> Result<(), JsValue>;
}

struct Handle(JsValue);

// The engine's file system must be shareable between threads, and a value
// of the browser's is not. There is one thread here: this target has no
// others, and a worker runs one thing at a time.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

fn failed(what: &str, error: JsValue) -> io::Error {
    let said = js_sys::Reflect::get(&error, &JsValue::from_str("message"))
        .ok()
        .and_then(|message| message.as_string())
        .unwrap_or_else(|| format!("{error:?}"));
    io::Error::new(io::ErrorKind::Other, format!("{what}: {said}"))
}

impl SlotStorage for Handle {
    fn read_at(&self, buffer: &mut [u8], at: u64) -> io::Result<usize> {
        owl_read(&self.0, buffer, at as f64)
            .map(|count| count as usize)
            .map_err(|error| failed("reading a file of the browser's", error))
    }

    fn write_at(&self, buffer: &[u8], at: u64) -> io::Result<usize> {
        owl_write(&self.0, buffer, at as f64)
            .map(|count| count as usize)
            .map_err(|error| failed("writing a file of the browser's", error))
    }

    fn truncate(&self, length: u64) -> io::Result<()> {
        owl_truncate(&self.0, length as f64).map_err(|error| failed("cutting a file of the browser's", error))
    }

    fn size(&self) -> io::Result<u64> {
        owl_size(&self.0)
            .map(|size| size as u64)
            .map_err(|error| failed("measuring a file of the browser's", error))
    }

    fn flush(&self) -> io::Result<()> {
        owl_flush(&self.0).map_err(|error| failed("flushing a file of the browser's", error))
    }
}

/// The handles a page opened, as files of a pool.
pub fn slots(handles: js_sys::Array) -> Vec<Box<dyn SlotStorage>> {
    handles.iter().map(|handle| Box::new(Handle(handle)) as Box<dyn SlotStorage>).collect()
}
