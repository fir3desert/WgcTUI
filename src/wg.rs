//! Todo lo que toca WireGuard: leer los .conf, el estado en vivo y activar/desactivar.

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
    /// Some(true) = activando, Some(false) = desactivando
    pub busy: Option<bool>,
    /// Contenido del .conf con las claves ocultas.
    pub conf: String,
    pub address: String,
    pub endpoint: String,
    /// Solo con el túnel activo.
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

/// Estado de los túneles en el instante en que se leyó, para calcular velocidades.
pub struct Snapshot {
    pub tunnels: Vec<Tunnel>,
    pub at: Instant,
}

/// Mensajes que los hilos de trabajo envían a la interfaz.
pub enum Msg {
    /// Terminó un `wg-quick up/down`.
    Toggled {
        name: String,
        up: bool,
        result: Result<(), String>,
    },
    /// Llegó una instantánea pedida con `spawn_refresh` (id para descartar viejas).
    Snapshot(u64, io::Result<Snapshot>),
}

/// Un túnel está activo si existe su interfaz de red (wg-quick usa el nombre del .conf).
pub fn is_active(name: &str) -> bool {
    Path::new("/sys/class/net").join(name).exists()
}

/// Lee el .conf ocultando claves privadas y preshared.
fn read_redacted(path: &Path) -> String {
    match fs::read_to_string(path) {
        Ok(txt) => txt
            .lines()
            .map(|l| {
                let low = l.trim().to_lowercase();
                if low.starts_with("privatekey") {
                    "PrivateKey = (oculta)".to_string()
                } else if low.starts_with("presharedkey") {
                    "PresharedKey = (oculta)".to_string()
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Err(e) => format!("No puedo leer {}: {e}", path.display()),
    }
}

/// Saca la primera Address y el primer Endpoint del .conf.
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

/// `wg show all dump`: una línea por interfaz (5 campos) y una por peer (9 campos),
/// separados por tabulador.
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

/// Lanza `wg-quick up|down` en su propio hilo: así puedes pedir varios a la vez
/// y la interfaz no se bloquea.
pub fn spawn_toggle(name: String, up: bool, tx: Sender<Msg>) {
    thread::spawn(move || {
        let action = if up { "up" } else { "down" };
        let result = match Command::new("wg-quick").args([action, name.as_str()]).output() {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(last_lines(&String::from_utf8_lossy(&o.stderr), 2)),
            Err(e) => Err(format!("no puedo ejecutar wg-quick: {e}")),
        };
        let _ = tx.send(Msg::Toggled { name, up, result });
    });
}

/// Lee los `.conf` y `wg show all dump` en un hilo. Hacerlo en el hilo de dibujo
/// congelaba la animación del menú cada vez que se refrescaba el estado.
pub fn spawn_refresh(id: u64, tx: Sender<Msg>) {
    thread::spawn(move || {
        let at = Instant::now();
        let result = load_tunnels().map(|tunnels| Snapshot { tunnels, at });
        let _ = tx.send(Msg::Snapshot(id, result));
    });
}

/// Si no somos root, nos relanzamos con sudo (pedirá la contraseña antes de abrir la TUI).
pub fn ensure_root() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("No puedo localizar el ejecutable: {e}");
            std::process::exit(1);
        }
    };
    let err = Command::new("sudo")
        .arg(exe)
        .args(std::env::args().skip(1))
        .exec();
    eprintln!("Hace falta root para leer {WG_DIR} y usar wg-quick. Ejecuta con sudo. ({err})");
    std::process::exit(1);
}

/// Hora local HH:MM:SS (sin dependencias extra).
pub fn clock() -> String {
    unsafe {
        let mut t: libc::time_t = 0;
        libc::time(&mut t);
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}
