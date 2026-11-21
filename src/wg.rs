//! WireGuard operations: reading .conf files and live status, and enabling/disabling tunnels.

use std::{
    collections::HashMap, fs, io, os::unix::process::CommandExt, path::Path, process::Command,
    sync::mpsc::Sender, thread, time::Instant,
};

pub const WG_DIR: &str = "/etc/wireguard";

#[derive(Clone, Default)]
pub struct Peer {
    pub public_key: String,
    pub endpoint: String,
    pub allowed_ips: String,
    pub handshake: u64,
    pub rx: u64,
    pub tx: u64,
    pub keepalive: String,
}

#[derive(Clone, Default)]
struct Iface {
    public_key: String,
    listen_port: String,
    peers: Vec<Peer>,
}

pub struct Tunnel {
    pub name: String,
    pub active: bool,
    /// Some(true) = enabling, Some(false) = disabling.
    pub busy: Option<bool>,
    /// .conf contents with keys hidden.
    pub conf: String,
    pub address: String,
    pub endpoint: String,
    /// Only available when the tunnel is active.
    pub public_key: String,
    pub listen_port: String,
    pub peers: Vec<Peer>,
}

impl Tunnel {
    pub fn rx(&self) -> u64 {
        self.peers.iter().map(|p| p.rx).sum()
    }
    pub fn tx(&self) -> u64 {
        self.peers.iter().map(|p| p.tx).sum()
    }
    pub fn last_handshake(&self) -> u64 {
        self.peers.iter().map(|p| p.handshake).max().unwrap_or(0)
    }
}

/// Tunnel state when read, used to calculate speeds.
pub struct Snapshot {
    pub tunnels: Vec<Tunnel>,
    pub at: Instant,
}

/// Messages sent to the interface by worker threads.
pub enum Msg {
    /// A `wg-quick up/down` command finished.
    Toggled {
        name: String,
        up: bool,
        result: Result<(), String>,
    },
    /// A snapshot requested by `spawn_refresh` arrived (the ID discards stale responses).
    Snapshot(u64, io::Result<Snapshot>),
}

/// A tunnel is active if its network interface exists (`wg-quick` uses the .conf file name).
pub fn is_active(name: &str) -> bool {
    Path::new("/sys/class/net").join(name).exists()
}

/// Read the .conf file, hiding private and preshared keys.
fn read_redacted(path: &Path) -> String {
    match fs::read_to_string(path) {
        Ok(txt) => txt
            .lines()
            .map(|l| {
                let low = l.trim().to_lowercase();
                if low.starts_with("privatekey") {
                    "PrivateKey = (hidden)".to_string()
                } else if low.starts_with("presharedkey") {
                    "PresharedKey = (hidden)".to_string()
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Err(e) => format!("Could not read {}: {e}", path.display()),
    }
}

/// Extract the first Address and Endpoint from the .conf file.
fn parse_conf_summary(conf: &str) -> (String, String) {
    let (mut address, mut endpoint) = (String::new(), String::new());
    for line in conf.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            match k.trim().to_lowercase().as_str() {
                "address" if address.is_empty() => address = v.trim().to_string(),
                "endpoint" if endpoint.is_empty() => endpoint = v.trim().to_string(),
                _ => {}
            }
        }
    }
    (address, endpoint)
}

/// `wg show all dump`: one line per interface (5 fields) and one per peer (9 fields),
/// separated by tabs.
fn read_wg_dump() -> HashMap<String, Iface> {
    let mut map: HashMap<String, Iface> = HashMap::new();
    let Ok(out) = Command::new("wg").args(["show", "all", "dump"]).output() else {
        return map;
    };
    if !out.status.success() {
        return map;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() >= 9 {
            map.entry(f[0].to_string()).or_default().peers.push(Peer {
                public_key: f[1].into(),
                endpoint: f[3].into(),
                allowed_ips: f[4].into(),
                handshake: f[5].parse().unwrap_or(0),
                rx: f[6].parse().unwrap_or(0),
                tx: f[7].parse().unwrap_or(0),
                keepalive: f[8].into(),
            });
        } else if f.len() == 5 {
            let iface = map.entry(f[0].to_string()).or_default();
            iface.public_key = f[2].into();
            iface.listen_port = f[3].into();
        }
    }
    map
}

pub fn load_tunnels() -> io::Result<Vec<Tunnel>> {
    let dump = read_wg_dump();
    let mut list: Vec<Tunnel> = fs::read_dir(WG_DIR)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("conf"))
        .filter_map(|p| {
            let name = p.file_stem()?.to_str()?.to_string();
            let conf = read_redacted(&p);
            let (address, endpoint) = parse_conf_summary(&conf);
            let active = is_active(&name);
            let (public_key, listen_port, peers) = match dump.get(&name) {
                Some(i) if active => (i.public_key.clone(), i.listen_port.clone(), i.peers.clone()),
                _ => Default::default(),
            };
            Some(Tunnel {
                name,
                active,
                busy: None,
                conf,
                address,
                endpoint,
                public_key,
                listen_port,
                peers,
            })
        })
        .collect();
    list.sort_by_key(|a| a.name.to_lowercase());
    Ok(list)
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join(" | ")
}

/// Run `wg-quick up|down` on its own thread so multiple operations can be requested at once
/// without blocking the interface.
pub fn spawn_toggle(name: String, up: bool, tx: Sender<Msg>) {
    thread::spawn(move || {
        let action = if up { "up" } else { "down" };
        let result = match Command::new("wg-quick").args([action, name.as_str()]).output() {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(last_lines(&String::from_utf8_lossy(&o.stderr), 2)),
            Err(e) => Err(format!("Could not run wg-quick: {e}")),
        };
        let _ = tx.send(Msg::Toggled { name, up, result });
    });
}

/// Read the `.conf` files and run `wg show all dump` on a worker thread. Doing this on the draw
/// thread froze the menu animation whenever the status was refreshed.
pub fn spawn_refresh(id: u64, tx: Sender<Msg>) {
    thread::spawn(move || {
        let at = Instant::now();
        let result = load_tunnels().map(|tunnels| Snapshot { tunnels, at });
        let _ = tx.send(Msg::Snapshot(id, result));
    });
}

/// If we are not root, relaunch with sudo (it prompts for a password before opening the TUI).
pub fn ensure_root() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Could not locate the executable: {e}");
            std::process::exit(1);
        }
    };
    let err = Command::new("sudo")
        .arg(exe)
        .args(std::env::args().skip(1))
        .exec();
    eprintln!("Root privileges are required to read {WG_DIR} and use wg-quick. Run with sudo. ({err})");
    std::process::exit(1);
}

/// Local time in HH:MM:SS format (without additional dependencies).
pub fn clock() -> String {
    unsafe {
        let mut t: libc::time_t = 0;
        libc::time(&mut t);
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}
