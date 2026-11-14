//! What this process's token says about its sandbox: whether it is an AppContainer, its
//! container SID and its capabilities (the self-sandbox check of Windows workers, and the
//! escape suites' probe).

use std::io;
use std::ptr::null_mut;

use windows_sys::Win32::Security::{
    GetTokenInformation, PSID, TOKEN_GROUPS, TOKEN_INFORMATION_CLASS, TOKEN_QUERY,
    TokenAppContainerSid, TokenCapabilities, TokenIsAppContainer,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::sid::string_of;

/// This process's token, as the suites and the self-sandbox check read it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenFacts {
    pub app_container: bool,
    pub container: Option<String>,
    pub capabilities: Vec<String>,
}

fn info(class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u64>> {
    let mut tok = null_mut();
    // SAFETY: the pseudo-handle of this process and a valid out-pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let tok = super::owned(tok)?;
    let mut len = 0u32;
    // SAFETY: a size query.
    unsafe { GetTokenInformation(super::raw(&tok), class, null_mut(), 0, &mut len) };
    let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
    // SAFETY: a buffer of at least `len` bytes.
    let ok = unsafe {
        GetTokenInformation(
            super::raw(&tok),
            class,
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(buf)
}

/// Reads this process's token.
pub fn current() -> io::Result<TokenFacts> {
    let is = info(TokenIsAppContainer)?;
    // SAFETY: the call wrote a DWORD.
    let app_container = unsafe { *is.as_ptr().cast::<u32>() } != 0;
    if !app_container {
        return Ok(TokenFacts {
            app_container,
            container: None,
            capabilities: vec![],
        });
    }
    let ac = info(TokenAppContainerSid)?;
    // SAFETY: the call wrote a TOKEN_APPCONTAINER_INFORMATION (one PSID) into `ac`.
    let container = unsafe { string_of(*ac.as_ptr().cast::<PSID>()) }?;
    let caps = info(TokenCapabilities)?;
    // SAFETY: the call wrote a TOKEN_GROUPS whose array and SIDs lie in `caps`.
    let capabilities = unsafe {
        let g = caps.as_ptr().cast::<TOKEN_GROUPS>();
        let n = (*g).GroupCount as usize;
        std::slice::from_raw_parts((*g).Groups.as_ptr(), n)
            .iter()
            .map(|a| string_of(a.Sid))
            .collect::<io::Result<Vec<_>>>()?
    };
    Ok(TokenFacts {
        app_container,
        container: Some(container),
        capabilities,
    })
}
