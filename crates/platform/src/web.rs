//! The browser: messages to the page and the console, the page's address, and fetching files.

use std::collections::BTreeMap;

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// Send panics to the console and the page, which would otherwise show nothing.
pub fn report_panics() {
    std::panic::set_hook(Box::new(|info| status(&format!("crashed: {info}"))));
}

/// Show `msg` in the page's `#status` element, if it has one, and in the console. An empty `msg` clears it.
pub fn status(msg: &str) {
    if !msg.is_empty() {
        log(msg);
    }
    if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id("status")) {
        el.set_text_content(Some(msg));
    }
}

/// Write `msg` to the browser's console.
pub fn log(msg: &str) {
    web_sys::console::log_1(&msg.into());
}

/// Set the browser tab's title.
pub fn set_title(title: &str) {
    if let Some(d) = web_sys::window().and_then(|w| w.document()) {
        d.set_title(title);
    }
}

/// A parameter from the page address, such as `seed` in `?seed=3`.
pub fn query(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    web_sys::UrlSearchParams::new_with_str(&search).ok()?.get(name)
}

/// Fetch every url at once, then wait for them all. Each result is the body, `None` for a missing file (404), or
/// an error.
pub async fn fetch_all(urls: &[String]) -> Vec<Result<Option<Vec<u8>>, String>> {
    let Some(window) = web_sys::window() else { return urls.iter().map(|_| Err("no window".into())).collect() };
    let started: Vec<_> = urls.iter().map(|u| JsFuture::from(window.fetch_with_str(u))).collect();
    let mut out = Vec::new();
    for (url, request) in urls.iter().zip(started) {
        out.push(body(url, request).await);
    }
    out
}

async fn body(url: &str, request: JsFuture) -> Result<Option<Vec<u8>>, String> {
    let fail = |e: wasm_bindgen::JsValue| format!("{url}: {e:?}");
    let response: web_sys::Response = request.await.map_err(fail)?.dyn_into().map_err(fail)?;
    if response.status() == 404 {
        return Ok(None);
    }
    if !response.ok() {
        return Err(format!("{url}: HTTP {}", response.status()));
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(fail)?).await.map_err(fail)?;
    Ok(Some(js_sys::Uint8Array::new(&buffer).to_vec()))
}

/// Fetch `paths` from under `base` (a url ending in `/`) into memory, keyed by path. Missing files are left out.
pub async fn fetch_files(base: &str, paths: &[String]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let urls: Vec<String> = paths.iter().map(|p| format!("{base}{p}")).collect();
    let mut files = BTreeMap::new();
    for (path, got) in paths.iter().zip(fetch_all(&urls).await) {
        if let Some(bytes) = got? {
            files.insert(path.clone(), bytes);
        }
    }
    Ok(files)
}

/// The page's canvas with this id, if it has one.
pub fn canvas(id: &str) -> Option<web_sys::HtmlCanvasElement> {
    web_sys::window()?.document()?.get_element_by_id(id)?.dyn_into().ok()
}
