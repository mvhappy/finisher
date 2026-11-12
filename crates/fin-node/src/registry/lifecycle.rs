//! `Registry`'s lifecycle helpers (8A plan Task 9b): what `finisherd` reads around an approval
//! (the staged plan), drops (a plan nobody will approve) and applies after a refresh (the
//! installed versions a `disable` advisory revokes). Same worker thread and promise discipline
//! as the plan calls.

use fin_core::CoreError;
use fin_registry::registry::Registry as Reg;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::{Deserialize, Serialize};

use super::{Registry, bad_request, core, on_worker};
use crate::{encode, reject};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstalledVersion {
    id: String,
    version: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Revocation {
    id: String,
    version: String,
    /// The advisory's id.
    advisory: String,
}

#[napi]
impl Registry {
    /// The staged `InstallPlan` JSON of `plan_id`; rejects with `PLAN_EXPIRED {reason: missing}`
    /// once it was committed, discarded or swept.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn staged_plan<'e>(&self, env: &'e Env, plan_id: String) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            encode(&inner.store.plan(&plan_id).map_err(|e| reject(core(e)))?)
        })
    }

    /// Every version directory in the store, readable or not: `[{id, version, dir}]` JSON
    /// (`dir` as `control.db` records it). `finisherd` reconciles its rows with it at start.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn installed_dirs<'e>(&self, env: &'e Env) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            let out: Vec<serde_json::Value> = inner
                .store
                .installed_dirs()
                .into_iter()
                .map(|(id, version, dir)| serde_json::json!({ "id": id, "version": version, "dir": dir }))
                .collect();
            encode(&out)
        })
    }

    /// Deletes the plan `plan_id` and its staged files; resolves to whether there was one.
    #[napi(catch_unwind, ts_return_type = "Promise<boolean>")]
    pub fn discard_plan<'e>(&self, env: &'e Env, plan_id: String) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || Ok(inner.store.discard(&plan_id)))
    }

    /// The versions of `installed_json` (`[{id, version}]`) that a `disable` advisory in the
    /// trusted metadata of registry `id` applies to: `[{id, version, advisory}]` JSON.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn revocations<'e>(
        &self,
        env: &'e Env,
        id: String,
        installed_json: String,
    ) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            let installed: Vec<InstalledVersion> = serde_json::from_str(&installed_json)
                .map_err(|e| bad_request("the installed versions", e))?;
            let reg = Reg::open(&inner.base, &id).map_err(|e| {
                reject(CoreError {
                    code: e.code.into(),
                    message: e.message,
                    details: serde_json::Value::Object(e.details),
                    fix: None,
                })
            })?;
            let pairs: Vec<(String, String)> =
                installed.into_iter().map(|i| (i.id, i.version)).collect();
            let out: Vec<Revocation> = reg
                .revocations(&pairs)
                .into_iter()
                .map(|(id, version, advisory)| Revocation {
                    id,
                    version,
                    advisory,
                })
                .collect();
            encode(&out)
        })
    }
}

/// Each permission's risk within the set (`low`, `medium` or `high`, in order), as a plan shows
/// it (sp8b Task 11: the developer-mode approval screen, which has no plan).
// napi registers the export; a `cargo test` build has no caller.
#[cfg_attr(test, allow(dead_code))]
#[napi(catch_unwind)]
pub fn permission_risks(permissions_json: String) -> Result<Vec<String>> {
    let perms: Vec<fin_plugin_api::Permission> =
        serde_json::from_str(&permissions_json).map_err(|e| bad_request("permissions", e))?;
    Ok(fin_plugin_api::risks(&perms)
        .into_iter()
        .map(|(_, r)| {
            match r {
                fin_plugin_api::Risk::Low => "low",
                fin_plugin_api::Risk::Medium => "medium",
                fin_plugin_api::Risk::High => "high",
            }
            .to_owned()
        })
        .collect())
}
