//! The spawner's Windows arm (plan 7C Decision 3, Task 9): a `worker` or `plugin` process is
//! created by the job host in its AppContainer, through `PlatformCommand.attributes`
//! (`PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES`: the container SID and the plan's
//! capabilities) and `PlatformCommand.desktop` (a private desktop per container, kept for the
//! spawner's life). The host keeps its own `HANDLE_LIST` and parent broker, places the process
//! in its Jobs before it runs, and the leaf Job gets the container UI limits. A worker still
//! gets its self-sandbox request, which on Windows checks its token (fail closed); its leaf
//! allows one active process (S9 point 4: every `CreateProcess` from inside then fails with
//! `ERROR_NOT_ENOUGH_QUOTA`).
//!
//! The grants of the request's paths are the ACL broker's (Task 10), made by the jobs runner
//! for the capability the plan names, before the start.

use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::{Arc, Mutex};

use fin_jobhost::spawner::{PlatformCommand, UiLimits, WinAttribute};
use windows_sys::Win32::System::Threading::PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES;

use crate::windows::apply::Container;
use crate::windows::desktop::PrivateDesktop;
use crate::windows::job::UI_LIMITS;
use crate::windows::plan::ContainerPlan;

/// The private desktops of the containers this spawner has started, one each.
#[derive(Default)]
pub struct Desktops(Mutex<HashMap<String, Arc<PrivateDesktop>>>);

impl std::fmt::Debug for Desktops {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Desktops")
    }
}

impl Desktops {
    fn of(&self, container: &Container) -> Result<String, String> {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(d) = map.get(&container.name) {
            return Ok(d.name().to_owned());
        }
        let d = PrivateDesktop::new(&container.sid.string())
            .map_err(|e| format!("the private desktop: {e}"))?;
        let name = d.name().to_owned();
        map.insert(container.name.clone(), Arc::new(d));
        Ok(name)
    }
}

/// Puts `cmd` in the plan's container.
pub fn contain(
    plan: &ContainerPlan,
    desktops: &Desktops,
    cmd: &mut PlatformCommand,
) -> Result<(), String> {
    let container = Container::of(plan).map_err(|e| format!("the container: {e}"))?;
    let value = container
        .security_capabilities()
        .map_err(|e| format!("the capabilities: {e}"))?;
    cmd.attributes.push(WinAttribute {
        attribute: PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
        value,
    });
    cmd.desktop = Some(desktops.of(&container)?.into());
    // Processes of one container belong to different jobs and calls (review B1).
    let own = plan.owner().ok_or("the plan names no owner capability")?;
    let user = crate::windows::sid::current_user().map_err(|e| format!("the user: {e}"))?;
    cmd.dacl = Some(fin_jobhost::process_dacl::sddl(&user.string(), &own.sid));
    cmd.job_limits.ui = Some(UiLimits(UI_LIMITS));
    private_temp(plan, cmd)?;
    // A Node plugin host needs the flags Node needs in a container (S9 point 8a).
    if crate::windows::launcher::is_node(cmd.program.as_os_str()) {
        let flags = crate::windows::launcher::NODE_FLAGS
            .iter()
            .map(OsString::from);
        cmd.args.splice(0..0, flags);
    }
    Ok(())
}

/// `LOCALAPPDATA` in the process's private temp and `TEMP`, `TMP`, `TMPDIR` at the folder
/// Windows derives from it (S9 point 7), made before the start.
fn private_temp(plan: &ContainerPlan, cmd: &mut PlatformCommand) -> Result<(), String> {
    let (Some(lad), Some(temp)) = (&plan.local_app_data, plan.temp()) else {
        return Ok(());
    };
    std::fs::create_dir_all(&temp).map_err(|e| format!("{}: {e}", temp.display()))?;
    let set = [
        ("LOCALAPPDATA", lad.clone()),
        ("TEMP", temp.clone()),
        ("TMP", temp.clone()),
        ("TMPDIR", temp),
    ];
    cmd.env.retain(|(k, _)| {
        !set.iter()
            .any(|(n, _)| k.to_string_lossy().eq_ignore_ascii_case(n))
    });
    cmd.env.extend(
        set.into_iter()
            .map(|(k, v)| (OsString::from(k), v.into_os_string())),
    );
    Ok(())
}
