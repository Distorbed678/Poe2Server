//! `cargo xtask <command>`: repository automation of Poe2Server.
//!
//! `openapi [--check]` writes (or checks) `docs/openapi.json`, the OpenAPI document of the web
//! API that the web admin and the launcher generate their types from. CI runs the check.
//! The dev tooling (client deploy, build checks, running the server from the hub) is in the hub
//! repository, Poe2ModdingProject.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("openapi") => {
            let check = match args.next().as_deref() {
                None => false,
                Some("--check") => true,
                Some(other) => bail!("unknown argument {other}; usage: cargo xtask openapi [--check]"),
            };
            openapi(check)
        }
        _ => bail!("usage: cargo xtask openapi [--check]"),
    }
}

fn openapi(check: bool) -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let path = root.join("docs/openapi.json");
    let text = poe_web_api::openapi_json() + "\n";
    if check {
        let have = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
        if have != text {
            bail!("{} is out of date: run `cargo xtask openapi`", path.display());
        }
        println!("{} is current", path.display());
    } else {
        std::fs::write(&path, text).with_context(|| path.display().to_string())?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
