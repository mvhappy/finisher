//! The command line of `codex app-server` as Decision 4 builds it:
//! `app-server --listen stdio:// --strict-config -c k=v ... --enable F ... --disable F ...`.
//! An unknown flag is a clap-style usage error (exit 2), as the CLI.

#[derive(Debug, Default)]
pub struct Argv {
    pub version: bool,
    pub app_server: bool,
    pub listen: String,
    pub strict: bool,
    /// `-c key=value` pairs, the value as given (TOML text).
    pub config: Vec<(String, String)>,
    pub enable: Vec<String>,
    pub disable: Vec<String>,
}

/// A usage error: the message for stderr (exit 2).
pub type Usage = String;

pub fn parse(args: &[String]) -> Result<Argv, Usage> {
    let mut a = Argv {
        listen: "stdio://".into(),
        ..Argv::default()
    };
    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        // `-ck=v` is `-c k=v`.
        let (flag, inline) = match flag {
            f if f.starts_with("-c") && f.len() > 2 && !f.starts_with("--") => {
                ("-c", Some(arg[2..].to_string()))
            }
            f => (f, inline),
        };
        let mut value = |name: &str| -> Result<String, Usage> {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or_else(|| {
                    format!("error: a value is required for '{name}' but none was supplied")
                })
        };
        match flag {
            "--version" | "-V" => a.version = true,
            "app-server" if !a.app_server => a.app_server = true,
            "--strict-config" => a.strict = true,
            "--listen" => a.listen = value("--listen <URL>")?,
            "--enable" => a.enable.push(value("--enable <FEATURE>")?),
            "--disable" => a.disable.push(value("--disable <FEATURE>")?),
            "-c" | "--config" => {
                let v = value("--config <key=value>")?;
                let (k, val) = v.split_once('=').ok_or_else(|| {
                    format!(
                        "error: invalid value '{v}' for '--config <key=value>': expected key=value"
                    )
                })?;
                a.config
                    .push((k.trim().to_string(), val.trim().to_string()));
            }
            f if f.starts_with('-') => {
                return Err(format!(
                    "error: unexpected argument '{f}' found\n\nUsage: codex [OPTIONS] [COMMAND]"
                ));
            }
            other => {
                return Err(format!(
                    "error: unrecognized subcommand '{other}'\n\nfake-codex implements `app-server` only"
                ));
            }
        }
    }
    if !a.version && !a.app_server {
        return Err(
            "error: fake-codex implements `app-server` only\n\nUsage: codex app-server [OPTIONS]"
                .into(),
        );
    }
    Ok(a)
}
