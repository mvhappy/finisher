//! The `FontManager` class: the server-level font calls (`fonts.catalog`, `fonts.status`,
//! `fonts.fetch`, the `fonts.autoFetch` switch). Engines opened on the same server data
//! share its manager.

use std::sync::Arc;

use fin_core::CoreError;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Deserialize;
use serde_json::json;

use super::Manager;
use crate::{encode, guarded, reject};

/// `fonts.fetch`: what to fetch. Every list may be empty; `pinned` names an open project
/// whose catalog pins are fetched.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FetchReq {
    #[serde(default)]
    pub(crate) sha256: Vec<String>,
    #[serde(default)]
    pub(crate) entries: Vec<String>,
    #[serde(default)]
    pub(crate) keys: Vec<String>,
    #[serde(default)]
    pub(crate) pinned: Option<String>,
}

/// A server data folder's font manager.
#[napi]
pub struct FontManager {
    manager: Arc<Manager>,
}

/// Runs `f` on its own thread (a download may take minutes; the libuv pool is not held)
/// and settles the promise with its JSON.
fn off_thread<'e>(
    env: &'e Env,
    f: impl FnOnce() -> Result<String> + Send + 'static,
) -> Result<Object<'e>> {
    let (deferred, promise) = env.create_deferred::<String, _>()?;
    std::thread::Builder::new()
        .name("fin-node-fonts-call".to_string())
        .spawn(move || match guarded(f, || {}) {
            Ok(s) => deferred.resolve(move |_env| Ok(s)),
            Err(e) => deferred.reject(e),
        })
        .map_err(|e| {
            reject(CoreError::new(
                "INTERNAL",
                format!("cannot start a font thread: {e}"),
            ))
        })?;
    Ok(promise)
}

#[napi]
impl FontManager {
    /// The manager of `server_data` (made on first use; shared with its engines).
    #[napi(factory, catch_unwind)]
    pub fn open(server_data: String) -> FontManager {
        FontManager {
            manager: super::for_server(std::path::Path::new(&server_data)),
        }
    }

    /// `fonts.autoFetch`: background fetching on or off, at run time. `FIN_FONTS_OFFLINE=1`
    /// keeps it off whatever this says.
    #[napi(catch_unwind)]
    pub fn set_auto_fetch(&self, on: bool) {
        self.manager.set_auto_fetch(on);
    }

    /// Whether background fetching is on now.
    #[napi(getter, catch_unwind)]
    pub fn auto_fetch(&self) -> bool {
        self.manager.auto_fetch()
    }

    /// The `FontCatalog` JSON this server uses.
    #[napi(catch_unwind)]
    pub fn catalog(&self) -> Result<String> {
        encode(self.manager.catalog())
    }

    /// Resolves to the `FontsStatus` JSON.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn status<'e>(&self, env: &'e Env) -> Result<Object<'e>> {
        let m = self.manager.clone();
        off_thread(env, move || encode(&m.status()))
    }

    /// Fetches now (`FetchReq` JSON), waiting for the downloads; resolves to the
    /// `FontsStatus` JSON, or rejects with the first `FONT_FETCH_FAILED` (or
    /// `FONT_NOT_IN_CATALOG`, `PROJECT_NOT_FOUND`, `FONTS_DAMAGED`).
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn fetch<'e>(&self, env: &'e Env, req_json: String) -> Result<Object<'e>> {
        let req: FetchReq = serde_json::from_str(&req_json).map_err(|e| {
            reject(
                CoreError::new(
                    "INVALID_OP",
                    format!("fonts fetch request does not decode: {e}"),
                )
                .with_details(json!({ "reason": e.to_string() }))
                .with_fix("send {sha256?, entries?, keys?, pinned?}"),
            )
        })?;
        let m = self.manager.clone();
        off_thread(env, move || {
            let shas = m.targets(&req).map_err(reject)?;
            m.fetch_now(&shas).map_err(reject)?;
            encode(&m.status())
        })
    }
}
