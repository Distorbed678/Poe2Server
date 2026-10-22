//! Which build of the web admin the server serves: `build.json` next to `index.html`, written
//! by the web admin's build (`npm run build`, Poe2WebAdmin `vite.config.ts`), and a check
//! whether the sources beside the `dist` folder are newer than that build.

use std::{
    path::Path,
    time::{Duration, SystemTime},
};

use serde::Deserialize;

/// `dist/build.json`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BuildInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// RFC 3339 build time.
    #[serde(default)]
    pub built_at: String,
    #[serde(default)]
    pub commit: Option<String>,
    /// The checkout had uncommitted changes in the sources.
    #[serde(default)]
    pub dirty: bool,
}

/// Reads `<dir>/build.json`; `None` when the file is missing or unreadable (a build from before
/// the file existed).
pub fn read_build_info(dir: &Path) -> Option<BuildInfo> {
    let text = std::fs::read_to_string(dir.join("build.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// The newest source file next to a `dist` folder (`src/`, `public/`, `index.html`,
/// `package.json`, `vite.config.ts`, ...), when it is newer than `build.json` by more than a
/// second: the build is stale. `None` when the folder is not a web admin checkout (a release
/// package has no sources) or the build is current.
pub fn stale_sources(dist: &Path) -> Option<String> {
    let checkout = dist.parent()?;
    if !checkout.join("package.json").is_file() || !checkout.join("src").is_dir() {
        return None;
    }
    let built = std::fs::metadata(dist.join("build.json"))
        .or_else(|_| std::fs::metadata(dist.join("index.html")))
        .ok()?
        .modified()
        .ok()?;
    let mut newest: Option<(SystemTime, String)> = None;
    let mut visit = |p: &Path| {
        if let Ok(t) = std::fs::metadata(p).and_then(|m| m.modified())
            && newest.as_ref().is_none_or(|(n, _)| t > *n)
        {
            newest = Some((t, p.display().to_string()));
        }
    };
    for f in [
        "index.html",
        "package.json",
        "package-lock.json",
        "vite.config.ts",
        "svelte.config.js",
        "tsconfig.json",
    ] {
        visit(&checkout.join(f));
    }
    for d in ["src", "public"] {
        walk(&checkout.join(d), &mut visit);
    }
    let (t, file) = newest?;
    (t > built + Duration::from_secs(1)).then_some(file)
}

fn walk(dir: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, visit);
        } else {
            visit(&p);
        }
    }
}

/// Logs the build that is served and warns when it is stale or has no `build.json`.
pub fn log_served(dir: &Path) {
    match read_build_info(dir) {
        Some(b) => tracing::info!(
            dir = %dir.display(),
            name = %b.name,
            version = %b.version,
            built_at = %b.built_at,
            commit = b.commit.as_deref().unwrap_or("unknown"),
            dirty = b.dirty,
            "serving the web admin"
        ),
        None => tracing::warn!(
            dir = %dir.display(),
            "serving the web admin, but {}/build.json is missing: an old build (rebuild with `npm run build` in the web admin folder)",
            dir.display()
        ),
    }
    if let Some(newer) = stale_sources(dir) {
        tracing::warn!(
            dir = %dir.display(),
            newer = %newer,
            "the web admin sources are newer than the build being served: run `npm run build` in the web admin folder (`cargo xtask server run` does it)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/webadmin-tests")
            .join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_build_json_and_tolerates_missing() {
        let d = tmp("read");
        assert!(read_build_info(&d).is_none());
        std::fs::write(
            d.join("build.json"),
            r#"{"name":"poe2-web-admin","version":"0.1.0","built_at":"2026-10-22T04:34:09.973Z","commit":"abc","dirty":false}"#,
        )
        .unwrap();
        let b = read_build_info(&d).unwrap();
        assert_eq!(b.version, "0.1.0");
        assert_eq!(b.commit.as_deref(), Some("abc"));
        std::fs::write(d.join("build.json"), "not json").unwrap();
        assert!(read_build_info(&d).is_none());
    }

    #[test]
    fn stale_when_a_source_is_newer() {
        let root = tmp("stale");
        let dist = root.join("dist");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(root.join("package.json"), "{}").unwrap();
        std::fs::write(root.join("src/a.ts"), "1").unwrap();
        std::fs::write(dist.join("build.json"), "{}").unwrap();
        assert!(stale_sources(&dist).is_none());
        std::thread::sleep(Duration::from_millis(2100));
        std::fs::write(root.join("src/a.ts"), "2").unwrap();
        assert!(stale_sources(&dist).unwrap().ends_with("a.ts"));
    }

    #[test]
    fn a_package_without_sources_is_never_stale() {
        let root = tmp("package");
        let dist = root.join("web-admin");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("index.html"), "").unwrap();
        assert!(stale_sources(&dist).is_none());
    }
}
