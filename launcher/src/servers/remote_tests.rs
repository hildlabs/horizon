use super::*;
use std::path::{Path, PathBuf};

#[test]
fn the_command_runs_horizon_server_in_the_folder_there() {
    let args = ssh_args(
        "mc@vps-1",
        "/srv/minecraft",
        &["config", "set", "world.motd=Hi there"],
    )
    .unwrap();
    let (line, rest) = args.split_last().unwrap();
    assert_eq!(&rest[rest.len() - 3..], ["-T", "--", "mc@vps-1"]);
    assert!(rest.windows(2).any(|w| w == ["-o", "BatchMode=yes"]));
    assert_eq!(
        line,
        "PATH=\"$HOME/.local/bin:$PATH\" exec horizon-server --dir '/srv/minecraft' 'config' 'set' \
         'world.motd=Hi there'"
    );
    // A folder in the home folder: the command starts there.
    let args = ssh_args("box", "~/servers/smp", &["start"]).unwrap();
    assert!(args.last().unwrap().contains("--dir 'servers/smp' 'start'"));
}

#[test]
fn what_the_remote_shell_gets_stays_one_argument_each() {
    assert_eq!(quote("plain"), "'plain'");
    assert_eq!(quote("it's"), r"'it'\''s'");
    assert_eq!(quote("$(rm -rf ~); `x`"), "'$(rm -rf ~); `x`'");
    let args = ssh_args("box", "/srv/a b", &["cmd", "say it's $HOME"]).unwrap();
    assert!(args
        .last()
        .unwrap()
        .ends_with(r"--dir '/srv/a b' 'cmd' 'say it'\''s $HOME'"));
}

#[test]
fn only_a_computer_name_is_taken_as_the_host() {
    for host in ["mc@203.0.113.24", "vps-fra-1", "me@[2001:db8::1]"] {
        assert!(check_host(host).is_ok(), "{host}");
    }
    for host in ["", "-oProxyCommand=evil", "a b", "x\ny"] {
        assert!(check_host(host).is_err(), "{host:?}");
        assert!(ssh_args(host, "/srv", &["status"]).is_err());
    }
    assert!(ssh_args("box", "", &["status"]).is_err());
    assert!(ssh_args("box", "~", &["status"]).is_err());
}

#[test]
fn problems_say_what_to_do() {
    let says = |code, err: &str| explain("mc@vps", "/srv/mc", Some(code), err);
    assert!(says(255, "mc@vps: Permission denied (publickey).").contains("ssh-copy-id mc@vps"));
    assert!(says(255, "Host key verification failed.").contains("accept it"));
    assert!(says(
        255,
        "ssh: Could not resolve hostname vps: Name or service not known"
    )
    .contains("no computer called"));
    assert!(says(
        255,
        "ssh: connect to host vps port 22: Connection timed out"
    )
    .contains("Can't reach"));
    assert!(says(127, "sh: 1: exec: horizon-server: not found").contains("has no horizon-server"));
    assert!(says(3, NO_SERVER).contains("no server in /srv/mc"));
    assert_eq!(
        says(
            255,
            "kex_exchange_identification: read: Connection reset by peer"
        ),
        "Couldn't connect to mc@vps: kex_exchange_identification: read: Connection reset by peer"
    );
    assert_eq!(
        says(
            1,
            "horizon-server: the game is starting or stopping: try again in a moment"
        ),
        "The game is starting or stopping: try again in a moment"
    );
}

/// The check, with a stand-in for ssh that runs the remote line in a local shell.
#[cfg(unix)]
#[test]
fn the_check_reads_the_status_there() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("horizon-remote-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bin = root.join("bin");
    let server = root.join("srv");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&server).unwrap();
    // ssh: the last argument is the remote line.
    let ssh = root.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nfor a; do line=$a; done\nPATH={}:/usr/bin:/bin; export PATH\nexec sh -c \"$line\"\n",
            bin.display()
        ),
    )
    .unwrap();
    let program = bin.join("horizon-server");
    std::fs::write(
        &program,
        "#!/bin/sh\necho '{\"phase\":\"running\",\"sample\":{\"game\":{\"players\":[{\"name\":\"Alex\"}],\"max_players\":20}}}'\n",
    )
    .unwrap();
    for f in [&ssh, &program] {
        std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let dir = server.to_string_lossy();
    let err = check_with(&ssh, "box", &dir).unwrap_err();
    assert!(err.contains("There's no server in"), "{err}");
    std::fs::write(server.join("server.toml"), "").unwrap();
    let status = check_with(&ssh, "box", &dir).unwrap();
    assert_eq!(status.phase, "running");
    assert_eq!(status.players[0].name, "Alex");
    assert_eq!(status.max_players, Some(20));
    std::fs::remove_file(&program).unwrap();
    let err = check_with(&ssh, "box", &dir).unwrap_err();
    assert!(err.contains("has no horizon-server"), "{err}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn lists_written_before_remote_servers_still_read() {
    let old: super::super::Entry = serde_json::from_str(r#"{"name":"A","dir":"/x"}"#).unwrap();
    assert_eq!(old.host, None);
    assert!(!serde_json::to_string(&old).unwrap().contains("host"));
}

#[test]
fn the_tunnel_forwards_a_port_here_to_the_api_there() {
    let args = tunnel_args("mc@vps", 50123, "127.0.0.1:8131").unwrap();
    assert!(args
        .windows(2)
        .any(|w| w == ["-o", "ExitOnForwardFailure=yes"]));
    assert!(args
        .windows(2)
        .any(|w| w == ["-L", "127.0.0.1:50123:127.0.0.1:8131"]));
    assert_eq!(&args[args.len() - 2..], ["--", "mc@vps"]);
    assert!(args.contains(&"-N".to_string()));
    for target in ["", "-oProxyCommand=x", "a b:1"] {
        assert!(tunnel_args("mc@vps", 1, target).is_err(), "{target:?}");
    }
    assert_eq!(
        split_url("http://127.0.0.1:8130/login?code=abc"),
        Some(("127.0.0.1:8130", "/login?code=abc"))
    );
    assert_eq!(split_url("ftp://x"), None);
}

/// A stand-in for ssh: `cat` lines run in a local shell, a port forward fails as `refused` says.
#[cfg(unix)]
fn fake_ssh(root: &Path, refused: bool) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let ssh = root.join("ssh");
    let forward = if refused {
        "echo 'channel 0: open failed: connect failed: Connection refused' >&2; echo 'Error: forward failed' >&2; exit 255"
    } else {
        "sleep 30"
    };
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nfor a; do [ \"$a\" = -N ] && {{ {forward}; }}; line=$a; done\nexec sh -c \"$line\"\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    ssh
}

#[cfg(unix)]
#[test]
fn the_api_there_is_found_through_its_api_json() {
    let root = std::env::temp_dir().join(format!("horizon-remote-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("srv/.horizon")).unwrap();
    let dir = root.join("srv").to_string_lossy().into_owned();
    // Not running: no api.json.
    let ssh = fake_ssh(&root, true);
    assert!(api_with(&ssh, "box", &dir).unwrap().is_none());
    // Running, but the forward fails: said as a connection problem, not a crash.
    std::fs::write(
        root.join("srv/.horizon/api.json"),
        r#"{"address":"127.0.0.1:8131","token":"t0k"}"#,
    )
    .unwrap();
    let err = api_with(&ssh, "box", &dir).err().unwrap();
    assert!(err.contains("The server on box isn't answering"), "{err}");
    // A forward that never comes up gives up after the wait.
    let ssh = fake_ssh(&root, false);
    let start = std::time::Instant::now();
    let err = Tunnel::open_with(
        &ssh,
        "box",
        "127.0.0.1:8131",
        std::time::Duration::from_millis(500),
    )
    .err()
    .unwrap();
    assert!(err.contains("Can't reach box"), "{err}");
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_page_keys_a_remote_server_apart_from_any_folder_here() {
    let local = super::super::Entry {
        name: "Here".into(),
        dir: "/srv/minecraft".into(),
        host: None,
    };
    let there = super::super::Entry {
        host: Some("mc@vps-key-test".into()),
        ..local.clone()
    };
    assert_eq!(local.key(), Path::new("/srv/minecraft"));
    assert_ne!(there.key(), local.key());
    assert_eq!(
        find(&there.key()),
        Some(("mc@vps-key-test".to_string(), "/srv/minecraft".to_string()))
    );
    assert_eq!(find(&local.key()), None);
    // Nothing answers yet: neither running nor said to be unreachable before a try.
    assert!(unreachable(&there.key()).is_none());
    // A file there is read in the background: the first ask doesn't wait for SSH.
    let start = std::time::Instant::now();
    assert_eq!(read(&there.key(), "ops.json"), None);
    assert!(start.elapsed() < std::time::Duration::from_millis(200));
}
