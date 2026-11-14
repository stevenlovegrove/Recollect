//! The engine, for a browser. It holds at most one open workspace and does no I/O: the worker that
//! owns it reads the files, hands them over, and writes back whatever each call says to. Everything
//! goes in and out as JSON text, and an error is JSON too: `{"kind": .., "message": ..}`.

use recollect_core::{Error, Files, Workspace, dispatch, new_files, sample_files};
use recollect_systems::samples::SAMPLES;
use serde::Serialize;
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

fn fail(e: Error) -> JsValue {
    JsValue::from_str(&json!({ "kind": e.kind(), "message": e.to_string() }).to_string())
}

fn nothing_open() -> JsValue {
    JsValue::from_str(&json!({ "kind": "nothing_open", "message": "no workspace is open" }).to_string())
}

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, JsValue> {
    serde_json::from_str(text).map_err(|e| fail(Error::Refused(format!("that isn't JSON this understands: {e}"))))
}

fn text<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("it serialises")
}

#[derive(Serialize)]
struct SampleInfo {
    name: &'static str,
    title: &'static str,
    about: &'static str,
    group: &'static str,
    systems: &'static [&'static str],
}

/// The samples there are.
#[wasm_bindgen]
pub fn catalogue() -> String {
    let samples: Vec<SampleInfo> = SAMPLES.iter().map(|s| SampleInfo { name: s.name, title: s.title, about: s.about, group: s.group, systems: s.systems }).collect();
    text(&json!({ "samples": samples }))
}

/// The files of a new, empty workspace.
#[wasm_bindgen(js_name = newFiles)]
pub fn new_workspace_files(name: &str) -> Result<String, JsValue> {
    Ok(text(&new_files(name).map_err(fail)?))
}

/// The files of a fresh copy of a sample.
#[wasm_bindgen(js_name = sampleFiles)]
pub fn sample_workspace_files(name: &str) -> Result<String, JsValue> {
    Ok(text(&sample_files(name).ok_or_else(|| fail(Error::NotFound(format!("sample {name}"))))?))
}

/// Which version of the samples a working copy has to say it is, or it's made again.
#[wasm_bindgen(js_name = sampleFormat)]
pub fn sample_format() -> String {
    recollect_core::SAMPLE_FORMAT.to_string()
}

#[wasm_bindgen]
#[derive(Default)]
pub struct Engine {
    open: Option<Workspace>,
}

#[wasm_bindgen]
impl Engine {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Engine {
        Engine::default()
    }

    /// Open a workspace from its files, in place of whatever's open. Returns its name, what needs
    /// attention, and the files to write straight away, such as one for a node only an edge mentioned.
    pub fn open(&mut self, id: &str, location: &str, sample: Option<String>, files: &str) -> Result<String, JsValue> {
        self.open = None;
        let files: Files = parse(files)?;
        let (mut ws, problems) = Workspace::load(id, location, sample, files).map_err(fail)?;
        let writes = ws.take_writes();
        let name = ws.config.name.clone();
        self.open = Some(ws);
        Ok(text(&json!({ "problems": problems, "writes": writes, "name": name })))
    }

    pub fn close(&mut self) {
        self.open = None;
    }

    #[wasm_bindgen(js_name = isOpen)]
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// One of `recollect_core::METHODS`, with its arguments. Returns `{value, effect, writes}`.
    pub fn call(&mut self, method: &str, args: &str) -> Result<String, JsValue> {
        let ws = self.open.as_mut().ok_or_else(nothing_open)?;
        let args: Value = parse(args)?;
        let (value, effect) = dispatch(ws, method, args, "you").map_err(fail)?;
        let writes = ws.take_writes();
        Ok(text(&json!({ "value": value, "effect": effect, "writes": writes })))
    }

    /// Files changed underneath it: `{path: text or null}`. Returns `{changed, writes}`.
    #[wasm_bindgen(js_name = filesChanged)]
    pub fn files_changed(&mut self, changes: &str) -> Result<String, JsValue> {
        let ws = self.open.as_mut().ok_or_else(nothing_open)?;
        let changed = ws.files_changed(parse(changes)?);
        let writes = ws.take_writes();
        Ok(text(&json!({ "changed": changed, "writes": writes })))
    }

    /// Every file of the open workspace as it's understood now.
    pub fn files(&self) -> Result<String, JsValue> {
        Ok(text(self.open.as_ref().ok_or_else(nothing_open)?.files()))
    }
}
