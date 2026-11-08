//! A promise that is always settled. napi-rs's `JsDeferred` has no `Drop`: one dropped
//! unsettled (a callback the host never ran, a panic on a host thread, a thread that did not
//! start) leaves its promise pending forever and its threadsafe function holding the event
//! loop open. [`Settle`] rejects with `INTERNAL` when dropped unsettled.

use fin_core::CoreError;
use napi::JsDeferred;
use napi::bindgen_prelude::{Env, Error, Object, Result, ToNapiValue};

use crate::reject;

type Resolver<T> = Box<dyn FnOnce(Env) -> Result<T> + Send>;

pub(crate) struct Settle<T: ToNapiValue + 'static> {
    deferred: Option<JsDeferred<T, Resolver<T>>>,
    /// What the promise answers, for the `INTERNAL` message.
    what: &'static str,
}

impl<T: ToNapiValue + 'static> Settle<T> {
    pub(crate) fn new<'e>(env: &'e Env, what: &'static str) -> Result<(Settle<T>, Object<'e>)> {
        let (deferred, promise) = env.create_deferred::<T, Resolver<T>>()?;
        Ok((
            Settle {
                deferred: Some(deferred),
                what,
            },
            promise,
        ))
    }

    pub(crate) fn resolve(mut self, f: impl FnOnce(Env) -> Result<T> + Send + 'static) {
        if let Some(d) = self.deferred.take() {
            d.resolve(Box::new(f));
        }
    }

    pub(crate) fn reject(mut self, e: Error) {
        if let Some(d) = self.deferred.take() {
            d.reject(e);
        }
    }
}

impl<T: ToNapiValue + 'static> Drop for Settle<T> {
    fn drop(&mut self) {
        if let Some(d) = self.deferred.take() {
            d.reject(reject(
                CoreError::new("INTERNAL", format!("{} was never answered", self.what))
                    .with_details(serde_json::json!({ "reason": "unanswered" })),
            ));
        }
    }
}
