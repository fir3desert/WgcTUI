//! Estrellas de GitHub del repositorio de wgctui, arriba a la derecha de la cabecera.
//!
//! Es la única petición de red que hace wgctui. Se lanza una vez al arrancar en un hilo de
//! fondo con el `curl` del sistema, nunca bloquea la interfaz, se guarda en caché una hora y se
//! puede desactivar con la opción `GitHub stars` (OPTIONS → General) o con `show_stars = off`
//! en `/etc/wgctui.conf`. Si la petición falla (sin red, sin `curl`, límite de GitHub) la
//! cabecera enseña un guion.
//!
//! El programa corre siempre como root, así que la caché vive en /var/cache y no depende del
//! HOME que deje sudo.

use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Repositorio cuyas estrellas se muestran.
pub const REPO: &str = "fir3desert/WgcTUI";
/// Cuánto se fía de un valor en caché antes de volver a preguntar a GitHub.
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);
/// Archivo de caché: `"<estrellas> <segundos unix>"`.
const CACHE_PATH: &str = "/var/cache/wgctui/stars.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarsState {
    /// Desactivado en las opciones: no se muestra nada ni se hace ninguna petición.
    Off,
    /// La petición sigue en marcha y no hay nada en caché.
    Loading,
    /// No se pudo obtener el número.
    Unavailable,
    Count(u64),
}

#[derive(Debug)]
pub struct Stars {
    state: StarsState,
    receiver: Option<Receiver<Option<u64>>>,
}

impl Stars {
    /// Empieza a buscar el número si `enabled`. Un valor en caché reciente se usa tal cual; uno
    /// viejo se muestra mientras se pide el nuevo.
    pub fn start(enabled: bool) -> Self {
        if !enabled {
            return Self::off();
        }
        // Los tests nunca tocan la red ni la caché real.
        if cfg!(test) {
            return Self { state: StarsState::Loading, receiver: None };
        }
        let cached = read_cache(Path::new(CACHE_PATH), now());
        if let Some((count, true)) = cached {
            return Self { state: StarsState::Count(count), receiver: None };
        }
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let count = fetch_count(REPO);
            if let Some(count) = count {
                write_cache(Path::new(CACHE_PATH), count, now());
            }
            let _ = sender.send(count);
        });
        Self {
            state: cached.map_or(StarsState::Loading, |(count, _)| StarsState::Count(count)),
            receiver: Some(receiver),
        }
    }

    pub fn off() -> Self {
        Self { state: StarsState::Off, receiver: None }
    }

    /// Recoge el resultado de la petición de fondo, si ya llegó.
    pub fn poll(&mut self) {
        let Some(receiver) = &self.receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(Some(count)) => {
                self.state = StarsState::Count(count);
                self.receiver = None;
            }
            Ok(None) | Err(mpsc::TryRecvError::Disconnected) => {
                // Mejor un número viejo en caché que cambiarlo por un guion.
                if !matches!(self.state, StarsState::Count(_)) {
                    self.state = StarsState::Unavailable;
                }
                self.receiver = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    pub fn state(&self) -> StarsState {
        self.state
    }

    /// Activa o desactiva la función desde la pantalla de opciones.
    pub fn set_enabled(&mut self, enabled: bool) {
        if enabled && self.state == StarsState::Off {
            *self = Self::start(true);
        } else if !enabled {
            *self = Self::off();
        }
    }

    #[cfg(test)]
    pub fn set_for_test(&mut self, state: StarsState) {
        self.state = state;
    }
}

/// Texto de la esquina de la cabecera: `★ 42`, `★ 1.2k`… `None` si está desactivado.
pub fn label(state: StarsState) -> Option<String> {
    match state {
        StarsState::Off => None,
        StarsState::Loading => Some("★ …".to_string()),
        StarsState::Unavailable => Some("★ —".to_string()),
        StarsState::Count(count) => Some(format!("★ {}", compact(count))),
    }
}

fn compact(count: u64) -> String {
    if count < 1_000 {
        return count.to_string();
    }
    let tenths = count / 100;
    let (whole, fraction) = (tenths / 10, tenths % 10);
    if fraction == 0 || whole >= 100 {
        format!("{whole}k")
    } else {
        format!("{whole}.{fraction}k")
    }
}

/// Saca `stargazers_count` de la respuesta JSON de la API de GitHub.
pub fn parse_star_count(json: &str) -> Option<u64> {
    let key = "\"stargazers_count\"";
    let after = &json[json.find(key)? + key.len()..];
    let after = after.trim_start().strip_prefix(':')?.trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn fetch_count(repo: &str) -> Option<u64> {
    let url = format!("https://api.github.com/repos/{repo}");
    let output = Command::new("curl")
        .args([
            "--silent",
            "--fail",
            "--location",
            "--max-time",
            "6",
            "--header",
            "Accept: application/vnd.github+json",
            "--header",
            "User-Agent: wgctui",
            &url,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_star_count(&String::from_utf8_lossy(&output.stdout))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Valor en caché como `(estrellas, es_reciente)`.
fn read_cache(path: &Path, now: u64) -> Option<(u64, bool)> {
    let text = fs::read_to_string(path).ok()?;
    let mut parts = text.split_whitespace();
    let count: u64 = parts.next()?.parse().ok()?;
    let saved: u64 = parts.next()?.parse().ok()?;
    Some((count, now.saturating_sub(saved) < CACHE_TTL.as_secs()))
}

fn write_cache(path: &Path, count: u64, now: u64) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, format!("{count} {now}\n"));
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wgctui-stars-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn it_points_at_the_wgctui_repository() {
        assert_eq!(REPO, "fir3desert/WgcTUI");
    }

    #[test]
    fn parses_the_count_from_the_api_answer() {
        let json = r#"{"id":1,"name":"WgcTUI","stargazers_count": 128,"watchers_count":128}"#;
        assert_eq!(parse_star_count(json), Some(128));
        assert_eq!(parse_star_count("{\n  \"stargazers_count\":0\n}"), Some(0));
    }

    #[test]
    fn rejects_answers_without_a_count() {
        assert_eq!(parse_star_count(""), None);
        assert_eq!(parse_star_count(r#"{"message":"Not Found"}"#), None);
        assert_eq!(parse_star_count(r#"{"stargazers_count":"many"}"#), None);
    }

    #[test]
    fn counts_are_shortened() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1_000), "1k");
        assert_eq!(compact(1_234), "1.2k");
        assert_eq!(compact(15_900), "15.9k");
        assert_eq!(compact(120_400), "120k");
    }

    #[test]
    fn labels_cover_every_state() {
        assert_eq!(label(StarsState::Off), None);
        assert_eq!(label(StarsState::Loading).as_deref(), Some("★ …"));
        assert_eq!(label(StarsState::Unavailable).as_deref(), Some("★ —"));
        assert_eq!(label(StarsState::Count(42)).as_deref(), Some("★ 42"));
    }

    #[test]
    fn cache_round_trips_and_expires() {
        let dir = temp_dir("cache");
        let path = dir.join("sub").join("stars.txt");
        assert_eq!(read_cache(&path, 1_000), None);
        write_cache(&path, 77, 1_000);
        assert_eq!(read_cache(&path, 1_000 + 60), Some((77, true)));
        assert_eq!(read_cache(&path, 1_000 + CACHE_TTL.as_secs() + 1), Some((77, false)));
        fs::write(&path, "garbage").unwrap();
        assert_eq!(read_cache(&path, 1_000), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_request_keeps_the_cached_count() {
        let (sender, receiver) = mpsc::channel();
        let mut stars = Stars { state: StarsState::Count(9), receiver: Some(receiver) };
        sender.send(None).unwrap();
        stars.poll();
        assert_eq!(stars.state(), StarsState::Count(9));

        let (sender, receiver) = mpsc::channel();
        let mut stars = Stars { state: StarsState::Loading, receiver: Some(receiver) };
        sender.send(None).unwrap();
        stars.poll();
        assert_eq!(stars.state(), StarsState::Unavailable);

        let (sender, receiver) = mpsc::channel();
        let mut stars = Stars { state: StarsState::Loading, receiver: Some(receiver) };
        sender.send(Some(5)).unwrap();
        stars.poll();
        assert_eq!(stars.state(), StarsState::Count(5));
    }

    #[test]
    fn a_dropped_worker_means_unavailable() {
        let (sender, receiver) = mpsc::channel::<Option<u64>>();
        let mut stars = Stars { state: StarsState::Loading, receiver: Some(receiver) };
        drop(sender);
        stars.poll();
        assert_eq!(stars.state(), StarsState::Unavailable);
    }

    #[test]
    fn disabled_means_nothing_is_shown_or_fetched() {
        let stars = Stars::start(false);
        assert_eq!(stars.state(), StarsState::Off);
        assert!(stars.receiver.is_none());
    }

    #[test]
    fn switching_on_and_off_follows_the_option() {
        let mut stars = Stars::off();
        stars.set_enabled(true);
        assert_ne!(stars.state(), StarsState::Off);
        stars.set_for_test(StarsState::Count(3));
        stars.set_enabled(true); // ya activado: no pierde el número
        assert_eq!(stars.state(), StarsState::Count(3));
        stars.set_enabled(false);
        assert_eq!(stars.state(), StarsState::Off);
    }
}
