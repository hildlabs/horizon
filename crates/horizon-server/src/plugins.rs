//! Plugins: native libraries in the server's `plugins/` folder, for an engine that loads them (a Paper-style
//! engine built on Horizon loads `plugins/*.so`, or `.dll` on Windows, by the `plugin.yml` each carries, and
//! loads a new or changed file while it runs). The supervisor only manages the files: it lists them with the
//! name and version from their `plugin.yml`, adds one, turns one off (renamed `<file>.disabled`) or on, and
//! deletes one. Reloading one now is the engine's `hotswap NAME` console command.
//!
//! Adding a library runs new code on the server, so only `horizon-server plugins add` does it (the server's own
//! user, like copying the file in). The control API and the panel list, turn off and on, reload and delete, as
//! they can't otherwise change what program runs.

use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::SystemTime,
};

/// The folder, under the server folder.
pub const DIR: &str = "plugins";

/// The extension of a library on this system.
pub const LIBRARY: &str = if cfg!(windows) {
    "dll"
} else if cfg!(target_os = "macos") {
    "dylib"
} else {
    "so"
};

/// Appended to a library's file name to turn it off: the engine loads only `*.LIBRARY`.
const OFF: &str = ".disabled";

/// The longest `plugin.yml` text looked at around its `main:` line.
const MANIFEST_MAX: usize = 64 << 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plugin {
    /// The file's name in `plugins/`.
    pub file: String,
    /// `plugin.yml`'s `name`, or the file's name without `lib` and its extension when none was found.
    pub name: String,
    pub version: Option<String>,
    pub enabled: bool,
    pub bytes: u64,
    /// Its data folder (`plugins/<name>/`) exists.
    pub data: bool,
}

impl Plugin {
    pub fn to_json(&self) -> Value {
        json!({
            "file": self.file,
            "name": self.name,
            "version": self.version,
            "enabled": self.enabled,
            "bytes": self.bytes,
            "data": self.data,
        })
    }
}

/// `plugin.yml`'s name and version per file, by path, size and time, so a library is read once.
type Manifests = HashMap<(PathBuf, u64, Option<SystemTime>), (Option<String>, Option<String>)>;
static MANIFESTS: LazyLock<Mutex<Manifests>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether `plugins/` exists: a server whose engine loads plugins has one.
pub fn has_folder(dir: &Path) -> bool {
    dir.join(DIR).is_dir()
}

/// The plugins, on and off, by name.
pub fn list(dir: &Path) -> Vec<Plugin> {
    let folder = dir.join(DIR);
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(enabled) = library(&file) else {
            continue;
        };
        let Some(meta) = entry.metadata().ok().filter(|m| m.is_file()) else {
            continue;
        };
        let (name, version) = manifest_of(&entry.path(), meta.len(), meta.modified().ok());
        let name = name.unwrap_or_else(|| stem(&file).to_owned());
        let data = folder.join(&name).is_dir();
        out.push(Plugin {
            file,
            name,
            version,
            enabled,
            bytes: meta.len(),
            data,
        });
    }
    out.sort_by(|a, b| (a.name.to_lowercase(), &a.file).cmp(&(b.name.to_lowercase(), &b.file)));
    out
}

/// `Some(on)` for a library's file name (`x.so`, or `x.so.disabled` when off); None for anything else.
fn library(file: &str) -> Option<bool> {
    if file.starts_with('.') {
        return None;
    }
    let lower = file.to_lowercase();
    let ext = format!(".{LIBRARY}");
    if lower.ends_with(&ext) && lower.len() > ext.len() {
        Some(true)
    } else if lower.ends_with(&format!("{ext}{OFF}")) && lower.len() > ext.len() + OFF.len() {
        Some(false)
    } else {
        None
    }
}

/// `libessentials.so` → `essentials`.
fn stem(file: &str) -> &str {
    let base = file.split('.').next().unwrap_or(file);
    if cfg!(windows) {
        base
    } else {
        base.strip_prefix("lib")
            .filter(|s| !s.is_empty())
            .unwrap_or(base)
    }
}

fn manifest_of(
    path: &Path,
    bytes: u64,
    modified: Option<SystemTime>,
) -> (Option<String>, Option<String>) {
    let key = (path.to_path_buf(), bytes, modified);
    if let Some(found) = MANIFESTS.lock().unwrap().get(&key) {
        return found.clone();
    }
    let found = std::fs::read(path)
        .map(|b| manifest(&b))
        .unwrap_or((None, None));
    MANIFESTS.lock().unwrap().insert(key, found.clone());
    found
}

/// The `name` and `version` of the `plugin.yml` a library carries: the text around a `main:` line (a
/// `plugin.yml` names its entry point), up to the bytes that aren't text.
pub fn manifest(bytes: &[u8]) -> (Option<String>, Option<String>) {
    let text = |b: u8| b == b'\n' || b == b'\r' || b == b'\t' || b >= 0x20;
    let mut from = 0;
    while let Some(at) = find(&bytes[from..], b"main:").map(|i| from + i) {
        from = at + 1;
        if at > 0 && bytes[at - 1] != b'\n' {
            continue;
        }
        let start = bytes[..at]
            .iter()
            .rposition(|&b| !text(b))
            .map_or(0, |i| i + 1)
            .max(at.saturating_sub(MANIFEST_MAX));
        let end = bytes[at..]
            .iter()
            .position(|&b| !text(b))
            .map_or(bytes.len(), |i| at + i)
            .min(at + MANIFEST_MAX);
        let yml = String::from_utf8_lossy(&bytes[start..end]);
        // Other strings can sit right before it with nothing between: its first line starts at its key.
        let first = |l: &str, key: &str| l.rfind(&format!("{key}:")).unwrap_or(l.len());
        let key = |key: &str| {
            yml.lines()
                .enumerate()
                .map(|(i, l)| if i == 0 { &l[first(l, key)..] } else { l })
                .filter_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
                .map(|v| v.trim().trim_matches(['"', '\'']).trim())
                .find(|v| !v.is_empty() && v.len() <= 64)
                .map(str::to_owned)
        };
        if let Some(name) = key("name") {
            return (Some(name), key("version"));
        }
    }
    (None, None)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// One plugin, by its name or its file's name (any case).
pub fn find_plugin(dir: &Path, name: &str) -> Result<Plugin, String> {
    let all = list(dir);
    let name = name.trim();
    let mut found = all
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(name) || p.file.eq_ignore_ascii_case(name));
    match (found.next(), found.next()) {
        (Some(p), None) => Ok(p.clone()),
        (Some(_), Some(_)) => Err(format!(
            "more than one file is called {name}: name it by its file (`horizon-server plugins` lists them)"
        )),
        (None, _) => Err(format!(
            "no plugin called {name} (`horizon-server plugins` lists them)"
        )),
    }
}

/// Turns a plugin on or off by renaming its file; the engine loads or unloads it.
pub fn set_enabled(dir: &Path, name: &str, on: bool) -> Result<Plugin, String> {
    let plugin = find_plugin(dir, name)?;
    if plugin.enabled == on {
        return Ok(plugin);
    }
    let folder = dir.join(DIR);
    let to = if on {
        plugin.file[..plugin.file.len() - OFF.len()].to_owned()
    } else {
        format!("{}{OFF}", plugin.file)
    };
    if folder.join(&to).exists() {
        return Err(format!(
            "{to} is in the way in {DIR}/: delete one of the two copies first"
        ));
    }
    std::fs::rename(folder.join(&plugin.file), folder.join(&to))
        .map_err(|e| format!("couldn't rename {}: {e}", plugin.file))?;
    Ok(Plugin {
        file: to,
        enabled: on,
        ..plugin
    })
}

/// Deletes a plugin's library; its data folder stays.
pub fn remove(dir: &Path, name: &str) -> Result<Plugin, String> {
    let plugin = find_plugin(dir, name)?;
    std::fs::remove_file(dir.join(DIR).join(&plugin.file))
        .map_err(|e| format!("couldn't delete {}: {e}", plugin.file))?;
    Ok(plugin)
}

/// What [`add`] did.
#[derive(Debug, PartialEq, Eq)]
pub struct Added {
    pub plugin: Plugin,
    /// The files it replaced: the same file, or another copy of the same plugin (on or off).
    pub replaced: Vec<String>,
}

/// Puts a library into `plugins/` as `file`: written beside it first, then renamed, so the engine never sees
/// half a file. Another copy of the same plugin (by `plugin.yml`'s name, or the same file turned off) is
/// deleted, as the engine refuses two plugins of one name.
pub fn add(dir: &Path, file: &str, bytes: &[u8]) -> Result<Added, String> {
    if file.contains(['/', '\\']) || library(file) != Some(true) {
        return Err(format!(
            "{file} isn't a plugin library: a plugin is a .{LIBRARY} file on this system"
        ));
    }
    let (name, version) = manifest(bytes);
    let folder = dir.join(DIR);
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let others: Vec<Plugin> = list(dir)
        .into_iter()
        .filter(|p| {
            p.file.eq_ignore_ascii_case(file)
                || p.file.eq_ignore_ascii_case(&format!("{file}{OFF}"))
                || name
                    .as_ref()
                    .is_some_and(|n| p.name.eq_ignore_ascii_case(n))
        })
        .collect();
    let part = folder.join(format!(".{file}.part"));
    std::fs::write(&part, bytes).map_err(|e| format!("couldn't write {file}: {e}"))?;
    let mut replaced = Vec::new();
    for other in others.iter().filter(|o| !o.file.eq_ignore_ascii_case(file)) {
        if let Err(e) = std::fs::remove_file(folder.join(&other.file)) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("couldn't delete the old {}: {e}", other.file));
        }
        replaced.push(other.file.clone());
    }
    if let Some(same) = others.iter().find(|o| o.file.eq_ignore_ascii_case(file)) {
        replaced.push(same.file.clone());
    }
    std::fs::rename(&part, folder.join(file)).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("couldn't put {file} in place: {e}")
    })?;
    let name = name.unwrap_or_else(|| stem(file).to_owned());
    let data = folder.join(&name).is_dir();
    Ok(Added {
        plugin: Plugin {
            file: file.to_owned(),
            name,
            version,
            enabled: true,
            bytes: bytes.len() as u64,
            data,
        },
        replaced,
    })
}

/// The command that reloads a plugin now, changed or not, on an engine that has it.
pub fn reload_command(plugin: &Plugin) -> String {
    format!("hotswap {}", plugin.name)
}

/// `horizon-server plugins ...`.
pub fn command(dir: &Path, args: &[String]) -> Result<(), String> {
    let name = |what: &str| {
        args.get(1)
            .filter(|n| !n.trim().is_empty())
            .map(String::as_str)
            .ok_or_else(|| format!("plugins {what} needs a plugin's name"))
    };
    match args.first().map(String::as_str) {
        None | Some("list" | "--json") => print_list(dir, args.iter().any(|a| a == "--json")),
        Some("add") => add_command(dir, &args[1..]),
        Some("on") => {
            let p = set_enabled(dir, name("on")?, true)?;
            println!("Turned {} on ({})", p.name, p.file);
            Ok(())
        }
        Some("off") => {
            let p = set_enabled(dir, name("off")?, false)?;
            println!("Turned {} off ({})", p.name, p.file);
            Ok(())
        }
        Some("remove") => {
            let p = remove(dir, name("remove")?)?;
            println!(
                "Deleted {}{}",
                p.file,
                if p.data {
                    format!(" (its data in {DIR}/{}/ stays)", p.name)
                } else {
                    String::new()
                }
            );
            Ok(())
        }
        Some("reload") => {
            let plugin = find_plugin(dir, name("reload")?)?;
            let client = crate::api::Client::open(dir)?
                .ok_or("the server isn't running: it loads its plugins when it starts")?;
            let reply = client.call(
                "POST",
                "/api/v1/command",
                Some(&json!({ "line": reload_command(&plugin) })),
            )?;
            for line in reply["output"].as_array().into_iter().flatten() {
                println!("{}", line.as_str().unwrap_or_default());
            }
            Ok(())
        }
        Some(other) => Err(format!(
            "plugins: unknown command {other} (list, add, on, off, remove, reload)"
        )),
    }
}

fn print_list(dir: &Path, as_json: bool) -> Result<(), String> {
    let all = list(dir);
    if as_json {
        let plugins: Vec<Value> = all.iter().map(Plugin::to_json).collect();
        println!(
            "{}",
            json!({ "folder": has_folder(dir), "library": LIBRARY, "plugins": plugins })
        );
        return Ok(());
    }
    if all.is_empty() {
        println!("No plugins in {}/{DIR}.", dir.display());
    }
    for p in all {
        println!(
            "{:<4} {:<24} {:<12} {}",
            if p.enabled { "on" } else { "off" },
            p.name,
            p.version.as_deref().unwrap_or("-"),
            p.file
        );
    }
    Ok(())
}

/// `plugins add FILE`, or `plugins add - --name FILE` for a library on stdin (as over ssh).
fn add_command(dir: &Path, args: &[String]) -> Result<(), String> {
    use std::io::Read;
    let Some(source) = args.first() else {
        return Err(format!("plugins add needs a .{LIBRARY} file"));
    };
    let named = args
        .iter()
        .position(|a| a == "--name")
        .map(|i| args.get(i + 1).cloned().ok_or("--name needs a file name"))
        .transpose()?;
    let (file, bytes) = if source == "-" {
        let file = named.ok_or("plugins add - reads stdin: give its file name with --name")?;
        let mut bytes = Vec::new();
        std::io::stdin()
            .read_to_end(&mut bytes)
            .map_err(|e| format!("couldn't read stdin: {e}"))?;
        (file, bytes)
    } else {
        let path = Path::new(source);
        let bytes = std::fs::read(path).map_err(|e| format!("{source}: {e}"))?;
        let file = named.unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        (file, bytes)
    };
    let added = add(dir, &file, &bytes)?;
    let p = &added.plugin;
    let version = p
        .version
        .as_deref()
        .map(|v| format!(" {v}"))
        .unwrap_or_default();
    if added.replaced.is_empty() {
        println!("Added {}{version} ({})", p.name, p.file);
    } else {
        println!(
            "Replaced {} with {}{version} ({})",
            added.replaced.join(", "),
            p.name,
            p.file
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library as a build leaves it: code around the `plugin.yml` text it carries.
    fn library_with(yml: &str) -> Vec<u8> {
        let mut bytes = vec![0x7f, b'E', b'L', b'F', 0, 1, 2, 0xff];
        bytes.extend_from_slice(b"name: not this, it's a string in the code\0\x01");
        bytes.extend_from_slice(yml.as_bytes());
        bytes.extend_from_slice(&[0, 0x90, 0x90, 0]);
        bytes
    }

    fn folder(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("horizon-plugins-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(DIR)).unwrap();
        dir
    }

    #[test]
    fn the_name_and_version_come_from_the_plugin_yml_inside() {
        let yml = "name: Essentials\nversion: '2.21.0'\nmain: essentials\ncommands:\n  home:\n    \
                   description: Go home\n    name: not-top-level\n";
        assert_eq!(
            manifest(&library_with(yml)),
            (Some("Essentials".into()), Some("2.21.0".into()))
        );
        // No plugin.yml: nothing, and the list falls back to the file's name.
        assert_eq!(manifest(b"\x7fELF\0main: x\0"), (None, None));
        // Packed between other strings, as a build may leave it.
        assert_eq!(
            manifest(b"\0src/lib.rsname: Heal\nversion: 1.0\nmain: heal\nsrc/lib.rs\0"),
            (Some("Heal".into()), Some("1.0".into()))
        );
        assert_eq!(
            stem(&format!("libtebex.{LIBRARY}")),
            if cfg!(windows) { "libtebex" } else { "tebex" }
        );
    }

    #[test]
    fn only_libraries_count() {
        assert_eq!(library(&format!("a.{LIBRARY}")), Some(true));
        assert_eq!(library(&format!("a.{LIBRARY}.disabled")), Some(false));
        for other in ["config.yml", ".a.so.part", "a.jar", "notes.txt"] {
            if other.ends_with(LIBRARY) {
                continue;
            }
            assert_eq!(library(other), None, "{other}");
        }
        assert_eq!(library(&format!(".a.{LIBRARY}.part")), None);
    }

    #[test]
    fn add_turn_off_and_on_and_delete() {
        let dir = folder("cycle");
        let file = format!("libheal.{LIBRARY}");
        let v1 = library_with("name: Heal\nversion: 1.0\nmain: heal\n");
        let added = add(&dir, &file, &v1).unwrap();
        assert_eq!(added.plugin.name, "Heal");
        assert!(added.replaced.is_empty());
        std::fs::create_dir_all(dir.join(DIR).join("Heal")).unwrap();
        let listed = list(&dir);
        assert_eq!(listed.len(), 1);
        assert!(listed[0].enabled && listed[0].data);
        assert_eq!(listed[0].version.as_deref(), Some("1.0"));

        let off = set_enabled(&dir, "heal", false).unwrap();
        assert_eq!(off.file, format!("{file}.disabled"));
        assert!(!dir.join(DIR).join(&file).exists());
        assert!(!list(&dir)[0].enabled);
        // A new build of a plugin that is off replaces the old copy and is on.
        let v2 = library_with("name: Heal\nversion: 1.1\nmain: heal\n");
        let added = add(&dir, &file, &v2).unwrap();
        assert_eq!(added.replaced, vec![format!("{file}.disabled")]);
        let listed = list(&dir);
        assert_eq!(listed.len(), 1);
        assert!(listed[0].enabled);
        assert_eq!(listed[0].version.as_deref(), Some("1.1"));
        // The same plugin under another file name replaces it too.
        let renamed = format!("heal-1.2.{LIBRARY}");
        let added = add(
            &dir,
            &renamed,
            &library_with("name: Heal\nversion: 1.2\nmain: heal\n"),
        )
        .unwrap();
        assert_eq!(added.replaced, vec![file.clone()]);
        assert_eq!(list(&dir).len(), 1);

        let gone = remove(&dir, "Heal").unwrap();
        assert_eq!(gone.file, renamed);
        assert!(list(&dir).is_empty());
        // Its data folder stays.
        assert!(dir.join(DIR).join("Heal").is_dir());
        assert!(find_plugin(&dir, "Heal").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn add_takes_only_a_library_file_name() {
        let dir = folder("names");
        for bad in [
            "../evil.so".to_string(),
            "a/b.so".into(),
            "notes.txt".into(),
            format!(".hidden.{LIBRARY}"),
            format!("a.{LIBRARY}.disabled"),
        ] {
            assert!(add(&dir, &bad, b"x").is_err(), "{bad}");
        }
        assert!(list(&dir).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
