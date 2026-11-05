//! A server on another computer (a VPS, a home server), reached over SSH: the launcher runs the same
//! `horizon-server` commands there as `ssh HOST horizon-server --dir DIR ...`, with the system's `ssh` and the
//! player's own keys and `~/.ssh/config`. `BatchMode` keeps ssh from asking for a password, so the launcher
//! never sees or stores one. The other computer runs Linux or macOS (a POSIX shell runs the command there).

use super::{Api, Status};
use std::ffi::OsStr;
use std::process::{Command, Stdio};

/// The options every connection takes: never prompt, give up on a dead address in 10 s, and notice a
/// dropped connection within a minute.
const OPTIONS: [&str; 8] = [
    "-o",
    "BatchMode=yes",
    "-o",
    "ConnectTimeout=10",
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=4",
];

/// `install.sh` puts `horizon-server` in `~/.local/bin`, which a command run over ssh (no login shell)
/// often doesn't have on its PATH.
const PATH: &str = "PATH=\"$HOME/.local/bin:$PATH\"";

/// What the check prints when the folder holds no `server.toml`.
const NO_SERVER: &str = "horizon-server: no server in this folder";

/// Checks what the player typed as the computer: `user@host`, `host`, or a `Host` name from
/// `~/.ssh/config`. Nothing that ssh could read as an option or that holds spaces.
pub fn check_host(host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err(
            "Type the computer: user@address, or a Host name from your ~/.ssh/config.".into(),
        );
    }
    if host.starts_with('-') || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("{host:?} isn't a computer ssh can connect to."));
    }
    Ok(())
}

/// The server's folder there, as the remote shell gets it: `~/x` is written `x`, since a command run
/// over ssh starts in the home folder.
fn remote_dir(dir: &str) -> Result<&str, String> {
    let dir = dir.trim();
    let dir = dir.strip_prefix("~/").unwrap_or(dir);
    if dir.is_empty() || dir == "~" || dir.chars().any(char::is_control) {
        return Err("Type the server's folder on that computer, like /srv/minecraft.".into());
    }
    Ok(dir)
}

/// One argument for a POSIX shell, in single quotes.
pub fn quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// ssh's arguments for `horizon-server --dir <dir> <args>` on `host`.
pub fn ssh_args(host: &str, dir: &str, args: &[&str]) -> Result<Vec<String>, String> {
    check_host(host)?;
    let dir = remote_dir(dir)?;
    let mut line = format!("{PATH} exec horizon-server --dir {}", quote(dir));
    for arg in args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    let mut out: Vec<String> = OPTIONS.iter().map(|s| s.to_string()).collect();
    out.extend(["-T".into(), "--".into(), host.into(), line]);
    Ok(out)
}

/// `horizon-server --dir <dir> <args>` on `host`, through `ssh`.
pub fn command(host: &str, dir: &str, args: &[&str]) -> Result<Command, String> {
    command_with("ssh", host, dir, args)
}

fn command_with(
    ssh: impl AsRef<OsStr>,
    host: &str,
    dir: &str,
    args: &[&str],
) -> Result<Command, String> {
    let mut cmd = crate::process::command(ssh);
    cmd.args(ssh_args(host, dir, args)?);
    Ok(cmd)
}

/// Connects and asks the server there for its status, as the Add dialog does before it adds a server.
pub fn check(host: &str, dir: &str) -> Result<Status, String> {
    check_with("ssh", host, dir)
}

fn check_with(ssh: impl AsRef<OsStr>, host: &str, dir: &str) -> Result<Status, String> {
    let mut args = ssh_args(host, dir, &["status", "--json"])?;
    // `status` answers "stopped" for any folder: say so when there is no server in it at all.
    let folder = quote(remote_dir(dir)?);
    let line = args.pop().unwrap_or_default();
    args.push(format!(
        "[ -f {folder}/server.toml ] || {{ echo \"{NO_SERVER}\" >&2; exit 3; }}; {line}"
    ));
    let output = crate::process::command(ssh)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                "This computer has no ssh program. It comes with Windows 10 and later (Settings › \
                 Optional features › OpenSSH Client), macOS and Linux."
                    .to_string()
            }
            _ => format!("Couldn't run ssh: {e}"),
        })?;
    let out = String::from_utf8_lossy(&output.stdout);
    let err = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(explain(host, dir, output.status.code(), &err));
    }
    let value: serde_json::Value = out
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str(l).ok())
        .ok_or_else(|| format!("horizon-server on {host} answered something unexpected."))?;
    Ok(Status::parse(&value))
}

/// What went wrong, in plain words, from ssh's exit code and what it or the remote shell printed.
pub fn explain(host: &str, dir: &str, code: Option<i32>, stderr: &str) -> String {
    let has = |s: &str| stderr.contains(s);
    if has(NO_SERVER) {
        return format!(
            "There's no server in {dir} on {host}. Check the folder, or set one up there with \
             horizon-server init."
        );
    }
    if has("Permission denied") {
        return format!(
            "{host} didn't accept your SSH key. Set one up once with ssh-copy-id {host} (or your \
             host's control panel), then try again."
        );
    }
    if has("Host key verification failed") || has("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        return format!(
            "ssh doesn't know {host}'s key yet, or it changed. Connect once in a terminal with \
             ssh {host} to check and accept it."
        );
    }
    if has("Could not resolve hostname") {
        return format!("There's no computer called {host}. Check the address.");
    }
    if has("forward failed") || has("open failed") {
        return format!(
            "The server on {host} isn't answering. It may have just stopped: try again in a moment."
        );
    }
    if has("timed out") || has("Connection refused") || has("No route to host") {
        return format!("Can't reach {host}. It may be off, or its SSH port closed.");
    }
    if code == Some(127) || has("command not found") {
        return format!(
            "{host} has no horizon-server. Install it there first, as the hosting guide says."
        );
    }
    if code == Some(255) {
        let said = stderr.lines().rev().find(|l| !l.trim().is_empty());
        return format!(
            "Couldn't connect to {host}{}",
            said.map_or_else(|| ".".to_string(), |s| format!(": {}", s.trim()))
        );
    }
    let said = stderr
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("horizon-server: "))
        .map(str::to_string)
        .unwrap_or_else(|| format!("horizon-server on {host} failed in {dir}."));
    super::capitalised(&said)
}

// ----- The control API, through a port forward ----------------------------------------------------

/// A port forward to the other computer, `ssh -N -L 127.0.0.1:<port>:<target>`, ended when dropped.
pub struct Tunnel {
    child: std::process::Child,
    /// Where it listens here: `127.0.0.1:<port>`.
    pub local: std::net::SocketAddr,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Tunnel {
    /// Forwards a free port here to `target` (`address:port`, as seen from `host`).
    pub fn open(host: &str, target: &str) -> Result<Self, String> {
        Self::open_with("ssh", host, target, std::time::Duration::from_secs(15))
    }

    fn open_with(
        ssh: impl AsRef<OsStr>,
        host: &str,
        target: &str,
        wait: std::time::Duration,
    ) -> Result<Self, String> {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map_err(|e| e.to_string())?
            .port();
        Self::open_at(ssh, host, port, target, wait)
    }

    /// [`Tunnel::open`] from a given port here.
    fn open_at(
        ssh: impl AsRef<OsStr>,
        host: &str,
        port: u16,
        target: &str,
        wait: std::time::Duration,
    ) -> Result<Self, String> {
        let local = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let mut child = crate::process::command(ssh)
            .args(tunnel_args(host, port, target)?)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Couldn't run ssh: {e}"))?;
        let start = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut e, &mut err);
                }
                return Err(explain(host, target, status.code(), &err));
            }
            let up =
                std::net::TcpStream::connect_timeout(&local, std::time::Duration::from_millis(200));
            if up.is_ok() {
                return Ok(Self { child, local });
            }
            if start.elapsed() > wait {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "Can't reach {host}. It may be off, or its SSH port closed."
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// ssh's arguments for a port forward from `127.0.0.1:<port>` here to `target` there.
pub fn tunnel_args(host: &str, port: u16, target: &str) -> Result<Vec<String>, String> {
    check_host(host)?;
    if target.is_empty() || target.starts_with('-') || target.chars().any(char::is_whitespace) {
        return Err(format!("{target:?} isn't an address to forward to."));
    }
    let mut out: Vec<String> = OPTIONS.iter().map(|s| s.to_string()).collect();
    out.extend([
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-N".into(),
        "-T".into(),
        "-L".into(),
        format!("127.0.0.1:{port}:{target}"),
        "--".into(),
        host.into(),
    ]);
    Ok(out)
}

/// The running server's control API on `host`, through a tunnel that lives as long as it is kept:
/// `None` when the server there isn't running (no `.horizon/api.json`).
pub fn api(host: &str, dir: &str) -> Result<Option<(Api, Tunnel)>, String> {
    api_with("ssh", host, dir)
}

fn api_with(
    ssh: impl AsRef<OsStr> + Copy,
    host: &str,
    dir: &str,
) -> Result<Option<(Api, Tunnel)>, String> {
    let folder = quote(remote_dir(dir)?);
    let mut args = ssh_args(host, dir, &[])?;
    args.pop();
    args.push(format!(
        "cat {folder}/.horizon/api.json 2>/dev/null || echo '{{}}'"
    ));
    let output = crate::process::command(ssh)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Couldn't run ssh: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(explain(host, dir, output.status.code(), &err));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null);
    let (Some(address), Some(token)) = (v["address"].as_str(), v["token"].as_str()) else {
        return Ok(None);
    };
    let tunnel = Tunnel::open_with(ssh, host, address, std::time::Duration::from_secs(15))?;
    // The server answers only to its own address as the Host, which the tunnel's end here isn't.
    let api = Api {
        address: tunnel.local.to_string(),
        token: token.to_string(),
        host: Some(address.to_string()),
    };
    Ok(Some((api, tunnel)))
}

/// A one-time web panel link that opens here: the panel's port forwarded too, and the link pointed at it.
pub fn panel_link(host: &str, api: &Api) -> Result<(String, Tunnel), String> {
    let url = api.panel_link()?;
    let (target, rest) =
        split_url(&url).ok_or_else(|| format!("the panel gave a link I can't open: {url}"))?;
    // The browser sends the address it opens as the Host, and the panel takes only its own: the
    // tunnel listens here on the panel's port there, so `127.0.0.1:<port>` is the same address.
    let port = target
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse::<u16>().ok())
        .ok_or_else(|| format!("the panel gave a link I can't open: {url}"))?;
    if std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
        return Err(format!(
            "Port {port} is in use on this computer (maybe by another server's panel), so the \
             panel there can't open here. Stop what uses it and try again."
        ));
    }
    let tunnel = Tunnel::open_at(
        "ssh",
        host,
        port,
        target,
        std::time::Duration::from_secs(15),
    )?;
    Ok((format!("http://{}{rest}", tunnel.local), tunnel))
}

/// `http://<address:port><rest>` → (`address:port`, `<rest>`).
fn split_url(url: &str) -> Option<(&str, &str)> {
    let after = url.strip_prefix("http://")?;
    let at = after.find('/').unwrap_or(after.len());
    Some((&after[..at], &after[at..]))
}

// ----- Remote servers on the Servers page -----------------------------------------------------------

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The remote servers the page has listed, by the path it keys them with ([`key`]).
static KEYS: Mutex<Vec<(PathBuf, String, String)>> = Mutex::new(Vec::new());

/// What the page keys a remote server with, where it keys a local one with its folder: `ssh://HOST:DIR`.
/// Every function of `servers` that takes a folder takes this too, and reaches the server over SSH.
pub fn key(host: &str, dir: &str) -> PathBuf {
    let key = PathBuf::from(format!("ssh://{host}:{dir}"));
    let mut keys = KEYS.lock().unwrap();
    if !keys.iter().any(|(k, _, _)| *k == key) {
        keys.push((key.clone(), host.to_string(), dir.to_string()));
    }
    key
}

/// The computer and folder behind a [`key`]; `None` for a folder on this computer.
pub fn find(key: &Path) -> Option<(String, String)> {
    KEYS.lock()
        .unwrap()
        .iter()
        .find(|(k, _, _)| k == key)
        .map(|(_, h, d)| (h.clone(), d.clone()))
}

/// A remote server's API tunnel, kept while the launcher runs, and the last time it couldn't connect.
struct Connection {
    open: Option<(Api, Tunnel)>,
    failed: Option<(Instant, String)>,
}

static CONNECTIONS: Mutex<Option<HashMap<PathBuf, Connection>>> = Mutex::new(None);
static PANELS: Mutex<Option<HashMap<PathBuf, Tunnel>>> = Mutex::new(None);

/// After a failed connection, the next try waits this long (the page asks every few seconds).
const RETRY_AFTER: Duration = Duration::from_secs(20);

/// The API of the server behind `key`, through its tunnel (opened on the first call and kept):
/// `None` when it isn't running, or when its computer can't be reached ([`unreachable`] says why).
pub fn open_api(key: &Path) -> Option<Api> {
    let (host, dir) = find(key)?;
    let mut all = CONNECTIONS.lock().unwrap();
    let all = all.get_or_insert_with(HashMap::new);
    let conn = all.entry(key.to_path_buf()).or_insert(Connection {
        open: None,
        failed: None,
    });
    if let Some((api, tunnel)) = &mut conn.open {
        if tunnel.child.try_wait().ok().flatten().is_none() {
            return Some(api.clone());
        }
        conn.open = None;
    }
    if conn
        .failed
        .as_ref()
        .is_some_and(|(at, _)| at.elapsed() < RETRY_AFTER)
    {
        return None;
    }
    match api(&host, &dir) {
        Ok(Some((api, tunnel))) => {
            conn.failed = None;
            conn.open = Some((api.clone(), tunnel));
            Some(api)
        }
        Ok(None) => {
            conn.failed = None;
            None
        }
        Err(e) => {
            conn.failed = Some((Instant::now(), e));
            None
        }
    }
}

/// Why the computer behind `key` couldn't be reached last time, if it couldn't.
pub fn unreachable(key: &Path) -> Option<String> {
    CONNECTIONS
        .lock()
        .unwrap()
        .as_ref()?
        .get(key)?
        .failed
        .as_ref()
        .map(|(_, e)| e.clone())
}

/// Tries the connection again at the next call, instead of waiting.
pub fn retry(key: &Path) {
    if let Some(all) = CONNECTIONS.lock().unwrap().as_mut() {
        all.remove(key);
    }
}

/// A one-time web panel link for the server behind `key`, through a panel tunnel kept while the
/// launcher runs (the browser keeps using it).
pub fn open_panel(key: &Path) -> Result<String, String> {
    let (host, _) = find(key).ok_or("not a server on another computer")?;
    let api = open_api(key)
        .ok_or_else(|| unreachable(key).unwrap_or_else(|| "Start the server first.".to_string()))?;
    let (url, tunnel) = panel_link(&host, &api)?;
    PANELS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(key.to_path_buf(), tunnel);
    Ok(url)
}

/// Files read from the servers' folders there, and when.
/// A file there (its server's key and name), when it was read, and what it held.
type Files = HashMap<(PathBuf, String), (Instant, Option<String>)>;

static FILES: Mutex<Option<Files>> = Mutex::new(None);

/// How long a file read there is shown before it is read again.
const FILE_FRESH: Duration = Duration::from_secs(5);

/// A file of the server's folder there (`ssh HOST cat DIR/FILE`), without waiting: what was read
/// last, while a fresh copy is read in the background once it is older than a few seconds. The
/// page asks for lists while it draws, and SSH takes too long for that.
pub fn read(key: &Path, file: &str) -> Option<String> {
    let id = (key.to_path_buf(), file.to_string());
    let mut files = FILES.lock().unwrap();
    let files = files.get_or_insert_with(HashMap::new);
    let known = files.get(&id).cloned();
    if known
        .as_ref()
        .is_none_or(|(at, _)| at.elapsed() > FILE_FRESH)
    {
        // Mark it as being read, so one draw after another doesn't start more reads.
        let shown = known.as_ref().and_then(|(_, t)| t.clone());
        files.insert(id.clone(), (Instant::now(), shown));
        if let Some((host, dir)) = find(key) {
            std::thread::spawn(move || {
                let text = read_now(&host, &dir, &id.1);
                if let Some(files) = FILES.lock().unwrap().as_mut() {
                    files.insert(id, (Instant::now(), text));
                }
            });
        }
    }
    known.and_then(|(_, text)| text)
}

fn read_now(host: &str, dir: &str, file: &str) -> Option<String> {
    let folder = quote(remote_dir(dir).ok()?);
    let mut args = ssh_args(host, dir, &[]).ok()?;
    args.pop();
    args.push(format!("cat {folder}/{}", quote(file)));
    let out = crate::process::command("ssh")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Ends every tunnel, when the launcher quits (the servers there keep running).
pub fn close_all() {
    CONNECTIONS.lock().unwrap().take();
    PANELS.lock().unwrap().take();
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
