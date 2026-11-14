//! A hook handler's call ([`HookCall`]): a `plugin.hook` v1 job (7F Decision 20) names the
//! manifest's handler and carries the event's [`HookRequest`]; the handler answers a
//! [`HookReply`] (`allow`, `block {reason}` or `context {text}`: there is no `modify`, spec
//! §9.1).

use fin_plugin_api::protocol::{HookRequest, HookRun, PluginError};

use crate::command::protocol;

/// One hook job: the handler's name in the manifest (`contributes.hooks[].handler`) and the
/// event's request.
#[derive(Debug, Clone, PartialEq)]
pub struct HookCall {
    pub handler: String,
    pub request: HookRequest,
}

impl From<HookRun> for HookCall {
    fn from(r: HookRun) -> HookCall {
        HookCall {
            handler: r.handler,
            request: r.request,
        }
    }
}

impl HookCall {
    /// The answer to a handler this plugin does not implement (its manifest names one its code
    /// lacks): `PLUGIN_PROTOCOL {reason: unknownHook, handler}`. The host takes it as a failed
    /// hook (the event's `onTimeout`).
    pub fn unknown(&self) -> PluginError {
        protocol(
            "unknownHook",
            format!(
                "the plugin does not implement the hook handler {:?}",
                self.handler
            ),
        )
        .with("handler", self.handler.clone())
    }
}
