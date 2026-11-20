//! wgctui: cliente TUI para WireGuard. Activa y desactiva varios túneles a la vez.

mod config;
mod finder;
mod menu;
mod theme;
mod ui;
mod wg;

use std::{
    collections::{HashMap, VecDeque},
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    widgets::ListState,
};

use config::{Config, Setting};
use finder::Finder;
use wg::{Msg, Tunnel, WG_DIR};

const REFRESH_EVERY: Duration = Duration::from_secs(1);
/// Muestras de velocidad que se guardan para la gráfica (una por refresco).
const HISTORY: usize = 60;

/// El barrido del banner necesita unos 60 fps. Con el sondeo fijo de 50 ms de
/// antes se dibujaba a 20 fps y la animación se notaba entrecortada.
const ANIMATION_TICK: Duration = Duration::from_millis(16);
/// Mientras gira el spinner basta con unos 12 fps.
const SPINNER_TICK: Duration = Duration::from_millis(80);
/// La onda infinita del texto de la cabecera no necesita tanta fluidez como el barrido.
const HEADER_TICK: Duration = Duration::from_millis(33);
/// Sin animación activa y visible que seguir, se dibuja menos para no gastar CPU.
const IDLE_TICK: Duration = Duration::from_millis(100);

/// Entradas del menú de Esc, en orden (la primera es la seleccionada al abrirlo).
pub const MENU_ITEMS: [&str; 3] = ["OPTIONS", "HELP", "QUIT"];

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Info,
    Ok,
    Err,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Overlay {
    None,
    Menu,
    Options,
    Help,
    Search,
}

pub struct App {
    pub tunnels: Vec<Tunnel>,
    pub state: ListState,
    pub scroll: u16,
    pub status: String,
    pub status_kind: Kind,
    pub status_at: Instant,
    pub samples: HashMap<String, (u64, u64, Instant)>,
    pub rates: HashMap<String, (f64, f64)>,
    pub history: HashMap<String, VecDeque<(f64, f64)>>,
    pub overlay: Overlay,
    pub menu_selected: usize,
    pub menu_opened: Option<Instant>,
    pub start: Instant,
    /// Ajustes guardados: cada animación se activa o desactiva por separado.
    pub config: Config,
    /// Dónde se guardan los ajustes (en los tests, un archivo temporal).
    config_path: PathBuf,
    /// Interruptor seleccionado en la pantalla OPTIONS.
    pub options_selected: usize,
    /// Cuándo arrancó el texto animado de la cabecera (el barrido vuelve a salir al reactivar).
    header_start: Instant,
    /// Lo que la última pantalla dibujada dejó ver: el menú grande (con banner) y el
    /// texto animado de la cabecera. Solo los usa el bucle para decidir la cadencia.
    pub menu_big: bool,
    pub header_fits: bool,
    pub finder: Finder,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// Id de la última instantánea pedida: sirve para descartar respuestas viejas.
    refresh_id: u64,
    last_refresh: Instant,
    quit: bool,
}

impl App {
    fn new() -> Self {
        let mut app = Self::with_config(Config::load());
        app.set_status("Cargando túneles…".into(), Kind::Info);
        app.refresh();
        app
    }

    /// Estado inicial con unos ajustes dados, sin tocar el disco ni lanzar hilos.
    fn with_config(config: Config) -> Self {
        let (tx, rx) = mpsc::channel();
        App {
            tunnels: Vec::new(),
            state: ListState::default(),
            scroll: 0,
            status: String::new(),
            status_kind: Kind::Info,
            status_at: Instant::now(),
            samples: HashMap::new(),
            rates: HashMap::new(),
            history: HashMap::new(),
            overlay: Overlay::None,
            menu_selected: 0,
            menu_opened: None,
            start: Instant::now(),
            config,
            config_path: PathBuf::from(config::CONFIG_PATH),
            options_selected: 0,
            header_start: Instant::now(),
            menu_big: false,
            header_fits: true,
            finder: Finder::default(),
            tx,
            rx,
            refresh_id: 0,
            last_refresh: Instant::now(),
            quit: false,
        }
    }

    pub fn set_status(&mut self, text: String, kind: Kind) {
        self.status = text;
        self.status_kind = kind;
        self.status_at = Instant::now();
    }

    /// El mensaje de estado mientras no haya caducado.
    pub fn visible_status(&self) -> Option<(&str, Kind)> {
        let ttl = if self.status_kind == Kind::Err {
            Duration::from_secs(15)
        } else {
            Duration::from_secs(6)
        };
        if self.status.is_empty() || self.status_at.elapsed() > ttl {
            None
        } else {
            Some((self.status.as_str(), self.status_kind))
        }
    }

    pub fn spinner(&self) -> &'static str {
        const F: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        F[((self.start.elapsed().as_millis() / 80) % 10) as usize]
    }

    pub fn selected(&self) -> Option<&Tunnel> {
        self.state.selected().and_then(|i| self.tunnels.get(i))
    }

    /// Cuánto lleva abierto el menú, mientras dure la animación del banner (barrido de luz
    /// y onda). `None`: banner estático y ya terminado (animación desactivada o acabada).
    pub fn banner_animation(&self) -> Option<Duration> {
        if !self.config.banner_animation || self.overlay != Overlay::Menu {
            return None;
        }
        self.menu_opened
            .map(|opened| opened.elapsed())
            .filter(|elapsed| elapsed.as_millis() < u128::from(menu::ANIMATION_MS))
    }

    /// Cuánto lleva abierto el menú, mientras dure su transición de aparición.
    /// `None`: el menú ya está del todo (o aparece de golpe, con la animación desactivada).
    pub fn menu_appear(&self) -> Option<Duration> {
        if !self.config.menu_animation || self.overlay != Overlay::Menu {
            return None;
        }
        self.menu_opened
            .map(|opened| opened.elapsed())
            .filter(|elapsed| elapsed.as_millis() < u128::from(menu::APPEAR_MS))
    }

    /// Cuánto lleva el texto de la cabecera (`None` = quieto, animación desactivada).
    pub fn header_animation(&self) -> Option<Duration> {
        self.config
            .header_animation
            .then(|| self.header_start.elapsed())
    }

    /// ¿Hay un texto de cabecera animado que se esté viendo?
    fn header_visible(&self) -> bool {
        self.config.header_animation && self.header_fits
    }

    /// Cadencia del bucle: rápida solo mientras haya una animación activa y visible;
    /// con todas desactivadas (o fuera de pantalla) vuelve al reposo.
    fn frame_timeout(&self) -> Duration {
        let banner = self.menu_big && self.banner_animation().is_some();
        let appearing = self.menu_appear().is_some();
        let header_sweep = self.header_visible()
            && self.header_animation().is_some_and(menu::header_sweeping);
        if banner || appearing || header_sweep {
            ANIMATION_TICK
        } else if self.header_visible() {
            HEADER_TICK // la onda del texto de la cabecera no se acaba nunca
        } else if self.visible_status().is_some()
            || self.tunnels.iter().any(|t| t.busy.is_some())
        {
            SPINNER_TICK
        } else {
            IDLE_TICK
        }
    }

    fn select(&mut self, idx: usize) {
        self.state.select(Some(idx));
        self.scroll = 0;
    }

    /// Pide los datos en un hilo. `wg show all dump` y las lecturas de `/etc/wireguard`
    /// bloquean: hacerlo en el hilo de dibujo congelaba la animación cada segundo.
    fn refresh(&mut self) {
        self.refresh_id += 1;
        wg::spawn_refresh(self.refresh_id, self.tx.clone());
        self.last_refresh = Instant::now();
    }

    /// Coloca una instantánea ya cargada, conservando selección y operaciones en curso.
    fn apply_snapshot(&mut self, snap: wg::Snapshot) {
        let selected = self.selected().map(|t| t.name.clone());
        let old_idx = self.state.selected();
        let busy: HashMap<String, bool> = self
            .tunnels
            .iter()
            .filter_map(|t| t.busy.map(|b| (t.name.clone(), b)))
            .collect();

        let mut list = snap.tunnels;
        for t in &mut list {
            if let Some(b) = busy.get(&t.name) {
                t.busy = Some(*b);
            }
        }
        self.tunnels = list;
        self.update_rates(snap.at);

        let idx = selected
            .and_then(|n| self.tunnels.iter().position(|t| t.name == n))
            .or(if self.tunnels.is_empty() { None } else { Some(0) });
        self.state.select(idx);
        if idx != old_idx {
            self.scroll = 0;
        }

        if self.tunnels.is_empty() {
            self.set_status(format!("No hay archivos .conf en {WG_DIR}"), Kind::Err);
        }
    }

    /// Velocidad de bajada/subida comparando contadores entre instantáneas, más el
    /// historial que alimenta la gráfica. `now` es cuando se tomaron los contadores.
    fn update_rates(&mut self, now: Instant) {
        let mut alive: Vec<String> = Vec::new();
        for t in &self.tunnels {
            if !t.active {
                continue;
            }
            alive.push(t.name.clone());
            let (rx, tx) = (t.rx(), t.tx());
            match self.samples.get(&t.name).copied() {
                Some((prx, ptx, pt)) => {
                    let dt = now.duration_since(pt).as_secs_f64();
                    if dt >= 0.5 {
                        let rate = (
                            rx.saturating_sub(prx) as f64 / dt,
                            tx.saturating_sub(ptx) as f64 / dt,
                        );
                        self.rates.insert(t.name.clone(), rate);
                        let hist = self.history.entry(t.name.clone()).or_default();
                        hist.push_back(rate);
                        while hist.len() > HISTORY {
                            hist.pop_front();
                        }
                        self.samples.insert(t.name.clone(), (rx, tx, now));
                    }
                }
                None => {
                    self.samples.insert(t.name.clone(), (rx, tx, now));
                }
            }
        }
        self.samples.retain(|k, _| alive.contains(k));
        self.rates.retain(|k, _| alive.contains(k));
        self.history.retain(|k, _| alive.contains(k));
    }

    fn move_sel(&mut self, delta: i32) {
        let n = self.tunnels.len() as i32;
        if n == 0 {
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as i32;
        self.select((cur + delta).rem_euclid(n) as usize);
    }

    fn toggle(&mut self) {
        let Some(i) = self.state.selected() else { return };
        let Some(t) = self.tunnels.get_mut(i) else { return };
        if t.busy.is_some() {
            return; // ya hay una operación en curso para este túnel
        }
        let up = !t.active;
        t.busy = Some(up);
        let name = t.name.clone();
        self.set_status(
            format!("{} {}…", if up { "Activando" } else { "Desactivando" }, name),
            Kind::Info,
        );
        wg::spawn_toggle(name, up, self.tx.clone());
    }

    fn on_msg(&mut self, m: Msg) {
        match m {
            Msg::Toggled { name, up, result } => {
                if let Some(t) = self.tunnels.iter_mut().find(|t| t.name == name) {
                    t.busy = None;
                    t.active = wg::is_active(&name);
                }
                match result {
                    Ok(()) => self.set_status(
                        format!("{} {}", name, if up { "activada" } else { "desactivada" }),
                        Kind::Ok,
                    ),
                    Err(e) => self.set_status(
                        format!(
                            "Error al {} {}: {e}",
                            if up { "activar" } else { "desactivar" },
                            name
                        ),
                        Kind::Err,
                    ),
                }
                self.refresh();
            }
            Msg::Snapshot(id, result) => {
                // Si ya pedimos otra instantánea, esta quedó obsoleta.
                if id != self.refresh_id {
                    return;
                }
                match result {
                    Ok(snap) => self.apply_snapshot(snap),
                    Err(e) => self.set_status(format!("No puedo leer {WG_DIR}: {e}"), Kind::Err),
                }
            }
        }
    }

    // ------------------------------------------------------------ teclado

    fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        if code == KeyCode::Char('c') && mods.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        match self.overlay {
            Overlay::Menu => self.menu_key(code),
            Overlay::Options => self.options_key(code),
            Overlay::Search => self.search_key(code, mods),
            Overlay::Help => {
                // Cualquier tecla cierra la ayuda y vuelve al menú (sin repetir la animación).
                self.overlay = Overlay::Menu;
                self.menu_opened = None;
            }
            Overlay::None => self.list_key(code, mods),
        }
    }

    fn open_menu(&mut self) {
        self.overlay = Overlay::Menu;
        self.menu_selected = 0;
        self.menu_opened = Some(Instant::now());
    }

    fn close_menu(&mut self) {
        self.overlay = Overlay::None;
        self.menu_opened = None;
    }

    fn menu_key(&mut self, code: KeyCode) {
        let n = MENU_ITEMS.len();
        match code {
            KeyCode::Esc => self.close_menu(),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.menu_selected = (self.menu_selected + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => self.menu_selected = (self.menu_selected + 1) % n,
            KeyCode::Enter | KeyCode::Char(' ') => match self.menu_selected {
                0 => {
                    self.overlay = Overlay::Options;
                    self.options_selected = 0;
                }
                1 => self.overlay = Overlay::Help,
                _ => self.quit = true,
            },
            _ => {}
        }
    }

    /// Opciones: un interruptor por animación. Esc vuelve al menú sin repetir su animación.
    fn options_key(&mut self, code: KeyCode) {
        let n = Setting::ALL.len();
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.overlay = Overlay::Menu;
                self.menu_opened = None;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.options_selected = (self.options_selected + n - 1) % n
            }
            KeyCode::Down | KeyCode::Char('j') => self.options_selected = (self.options_selected + 1) % n,
            KeyCode::Enter
            | KeyCode::Char(' ')
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Char('h' | 'l') => self.toggle_setting(),
            _ => {}
        }
    }

    /// Cambia el interruptor seleccionado y guarda los tres ajustes.
    fn toggle_setting(&mut self) {
        let setting = Setting::ALL[self.options_selected.min(Setting::ALL.len() - 1)];
        let on = self.config.toggle(setting);
        if setting == Setting::Header && on {
            self.header_start = Instant::now(); // el texto vuelve a empezar por el barrido
        }
        if let Err(e) = self.config.save_to(&self.config_path) {
            self.set_status(
                format!("No pude guardar {}: {e}", self.config_path.display()),
                Kind::Err,
            );
        }
    }

    // ------------------------------------------------------------ búsqueda

    fn open_search(&mut self) {
        self.finder.open();
        self.overlay = Overlay::Search;
    }

    /// Teclas del buscador, como las del buscador de archivos de runetui.
    fn search_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        let alt = mods.contains(KeyModifiers::ALT);
        let hits = self.finder.hits(&self.tunnels);
        let last = hits.len().saturating_sub(1);
        let selected = self.finder.selected.min(last);
        match (code, ctrl) {
            (KeyCode::Esc, _) => self.overlay = Overlay::None,
            (KeyCode::Enter, _) => {
                if let Some(hit) = hits.get(selected) {
                    self.select(hit.index);
                }
                self.overlay = Overlay::None;
            }
            (KeyCode::Down, _) | (KeyCode::Char('n'), true) => {
                self.finder.selected = (selected + 1).min(last)
            }
            (KeyCode::Up, _) | (KeyCode::Char('p'), true) => {
                self.finder.selected = selected.saturating_sub(1)
            }
            (KeyCode::PageDown, _) => self.finder.selected = (selected + 10).min(last),
            (KeyCode::PageUp, _) => self.finder.selected = selected.saturating_sub(10),
            (KeyCode::Home, _) => self.finder.selected = 0,
            (KeyCode::End, _) => self.finder.selected = last,
            (KeyCode::Backspace, _) => self.finder.backspace(),
            (KeyCode::Char('u'), true) => self.finder.clear_query(),
            (KeyCode::Char('w'), true) => self.finder.delete_word(),
            // Cualquier otro carácter es texto, `q` incluida: Esc cierra el buscador.
            (KeyCode::Char(c), false) if !alt => self.finder.type_char(c),
            _ => {}
        }
    }

    fn list_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        if code == KeyCode::Char('p') && mods.contains(KeyModifiers::CONTROL) {
            self.open_search();
            return;
        }
        match code {
            KeyCode::Char('/') => self.open_search(),
            KeyCode::Esc => self.open_menu(),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Home | KeyCode::Char('g') => {
                if !self.tunnels.is_empty() {
                    self.select(0);
                }
            }
            KeyCode::End | KeyCode::Char('G') => {
                if !self.tunnels.is_empty() {
                    self.select(self.tunnels.len() - 1);
                }
            }
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(5),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(5),
            KeyCode::Enter | KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('r') => self.refresh(),
            _ => {}
        }
    }

    fn run(mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.quit {
            terminal.draw(|f| ui::draw(&mut self, f))?;

            // El timeout se calcula después de dibujar: si esta tecla abrió el menú,
            // el siguiente fotograma ya corre a 60 fps.
            let timeout = self.frame_timeout();
            if event::poll(timeout)? {
                if let Event::Key(k) = event::read()? {
                    if k.kind == KeyEventKind::Press {
                        self.on_key(k.code, k.modifiers);
                    }
                }
            }
            while let Ok(msg) = self.rx.try_recv() {
                self.on_msg(msg);
            }
            if self.last_refresh.elapsed() >= REFRESH_EVERY {
                self.refresh();
            }
        }
        Ok(())
    }
}

fn main() -> io::Result<()> {
    wg::ensure_root();
    let mut terminal = ratatui::init();
    let result = App::new().run(&mut terminal);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: KeyModifiers = KeyModifiers::NONE;

    fn all_off() -> Config {
        Config {
            header_animation: false,
            banner_animation: false,
            menu_animation: false,
        }
    }

    /// App de prueba: ajustes en un archivo temporal propio (nunca toca /etc).
    fn app_with(config: Config, tag: &str) -> App {
        let mut app = App::with_config(config);
        app.config_path =
            std::env::temp_dir().join(format!("wgctui-app-{}-{tag}.conf", std::process::id()));
        app
    }

    fn ago(secs: u64) -> Instant {
        Instant::now()
            .checked_sub(Duration::from_secs(secs))
            .expect("el reloj monotónico lleva más de unos segundos en marcha")
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(code, NONE);
    }

    #[test]
    fn esc_menu_has_only_options_help_quit_in_that_order() {
        assert_eq!(MENU_ITEMS, ["OPTIONS", "HELP", "QUIT"]);
        let mut app = app_with(Config::default(), "menu-items");
        press(&mut app, KeyCode::Esc);
        assert!(app.overlay == Overlay::Menu);
        assert_eq!(MENU_ITEMS[app.menu_selected], "OPTIONS");
    }

    #[test]
    fn esc_inside_the_menu_still_closes_it() {
        let mut app = app_with(Config::default(), "menu-esc");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);
        assert!(app.overlay == Overlay::None);
        assert!(app.menu_opened.is_none());
    }

    #[test]
    fn menu_entries_open_the_right_screens() {
        let mut app = app_with(Config::default(), "menu-enter");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter); // OPTIONS
        assert!(app.overlay == Overlay::Options);
        press(&mut app, KeyCode::Esc); // vuelve al menú
        assert!(app.overlay == Overlay::Menu);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter); // HELP
        assert!(app.overlay == Overlay::Help);
        press(&mut app, KeyCode::Char('x')); // cualquier tecla vuelve
        assert!(app.overlay == Overlay::Menu);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter); // QUIT
        assert!(app.quit);
    }

    #[test]
    fn menu_selection_wraps_around() {
        let mut app = app_with(Config::default(), "menu-wrap");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Up);
        assert_eq!(MENU_ITEMS[app.menu_selected], "QUIT");
        press(&mut app, KeyCode::Down);
        assert_eq!(MENU_ITEMS[app.menu_selected], "OPTIONS");
    }

    #[test]
    fn options_keys_move_and_toggle_one_switch_at_a_time() {
        let mut app = app_with(Config::default(), "options-keys");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.options_selected, 0);

        // Header: Enter.
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            (app.config.header_animation, app.config.banner_animation, app.config.menu_animation),
            (false, true, true)
        );
        // Banner: ↓ y Espacio.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(
            (app.config.header_animation, app.config.banner_animation, app.config.menu_animation),
            (false, false, true)
        );
        // Menu: ↓ y ←, luego → lo vuelve a activar.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Left);
        assert!(!app.config.menu_animation);
        press(&mut app, KeyCode::Right);
        assert!(app.config.menu_animation);
        // ↑ sube y ↓ da la vuelta.
        press(&mut app, KeyCode::Down);
        assert_eq!(app.options_selected, 0);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.options_selected, 2);
        let _ = std::fs::remove_file(&app.config_path);
    }

    #[test]
    fn options_start_on_the_first_switch_each_time() {
        let mut app = app_with(Config::default(), "options-first");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.options_selected, 1);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.options_selected, 0);
    }

    #[test]
    fn toggling_saves_all_three_settings() {
        let mut app = app_with(Config::default(), "persist");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Down); // Banner
        press(&mut app, KeyCode::Enter);
        let text = std::fs::read_to_string(&app.config_path).unwrap();
        assert!(text.contains("header_animation = on"));
        assert!(text.contains("banner_animation = off"));
        assert!(text.contains("menu_animation = on"));
        assert_eq!(
            Config::load_from(&app.config_path),
            Config { banner_animation: false, ..Config::default() }
        );
        let _ = std::fs::remove_file(&app.config_path);
    }

    #[test]
    fn a_failed_save_is_reported_but_the_switch_still_changes() {
        let mut app = app_with(Config::default(), "save-fail");
        app.config_path = PathBuf::from("/no/existe/wgctui.conf");
        app.overlay = Overlay::Options;
        press(&mut app, KeyCode::Enter);
        assert!(!app.config.header_animation);
        assert!(matches!(app.visible_status(), Some((_, Kind::Err))));
    }

    #[test]
    fn reenabling_the_header_restarts_its_sweep() {
        let mut app = app_with(Config::default(), "header-restart");
        app.header_start = ago(30);
        assert!(!app.header_animation().is_some_and(menu::header_sweeping));
        app.overlay = Overlay::Options;
        press(&mut app, KeyCode::Enter); // off
        assert!(app.header_animation().is_none());
        press(&mut app, KeyCode::Enter); // on
        assert!(app.header_animation().is_some_and(menu::header_sweeping));
        let _ = std::fs::remove_file(&app.config_path);
    }

    // ------------------------------------------------- animaciones independientes

    #[test]
    fn menu_animations_only_exist_while_the_menu_is_open() {
        let mut app = app_with(Config::default(), "closed");
        assert!(app.banner_animation().is_none());
        assert!(app.menu_appear().is_none());
        press(&mut app, KeyCode::Esc);
        assert!(app.banner_animation().is_some());
        assert!(app.menu_appear().is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.banner_animation().is_none());
        assert!(app.menu_appear().is_none());
    }

    #[test]
    fn banner_switch_does_not_touch_the_menu_transition() {
        let mut app = app_with(Config { banner_animation: false, ..Config::default() }, "ind-banner");
        press(&mut app, KeyCode::Esc);
        assert!(app.banner_animation().is_none());
        assert!(app.menu_appear().is_some());
        assert!(app.header_animation().is_some());
    }

    #[test]
    fn menu_switch_does_not_touch_the_banner() {
        let mut app = app_with(Config { menu_animation: false, ..Config::default() }, "ind-menu");
        press(&mut app, KeyCode::Esc);
        assert!(app.menu_appear().is_none());
        assert!(app.banner_animation().is_some());
        assert!(app.header_animation().is_some());
    }

    #[test]
    fn header_switch_does_not_touch_the_menu() {
        let mut app = app_with(Config { header_animation: false, ..Config::default() }, "ind-header");
        assert!(app.header_animation().is_none());
        press(&mut app, KeyCode::Esc);
        assert!(app.menu_appear().is_some());
        assert!(app.banner_animation().is_some());
    }

    #[test]
    fn menu_animations_end_on_their_own() {
        let mut app = app_with(Config::default(), "ends");
        press(&mut app, KeyCode::Esc);
        app.menu_opened = Some(ago(1));
        assert!(app.menu_appear().is_none()); // 240 ms
        assert!(app.banner_animation().is_some()); // 3,5 s
        app.menu_opened = Some(ago(10));
        assert!(app.banner_animation().is_none());
    }

    #[test]
    fn coming_back_from_options_or_help_does_not_replay_the_menu_animations() {
        let mut app = app_with(Config::default(), "no-replay");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter); // OPTIONS
        press(&mut app, KeyCode::Esc);
        assert!(app.overlay == Overlay::Menu);
        assert!(app.banner_animation().is_none() && app.menu_appear().is_none());
    }

    // ------------------------------------------------------------ rendimiento

    #[test]
    fn everything_off_goes_back_to_the_idle_cadence() {
        let mut app = app_with(all_off(), "idle");
        assert_eq!(app.frame_timeout(), IDLE_TICK);
        press(&mut app, KeyCode::Esc);
        app.menu_big = true;
        assert_eq!(app.frame_timeout(), IDLE_TICK);
    }

    #[test]
    fn header_alone_sweeps_fast_then_waves_at_a_medium_rate() {
        let mut app = app_with(Config { header_animation: true, ..all_off() }, "hdr-rate");
        assert_eq!(app.frame_timeout(), ANIMATION_TICK); // barrido inicial
        app.header_start = ago(30);
        assert_eq!(app.frame_timeout(), HEADER_TICK); // onda infinita
    }

    #[test]
    fn hidden_header_does_not_keep_the_loop_fast() {
        let mut app = app_with(Config { header_animation: true, ..all_off() }, "hdr-hidden");
        app.header_fits = false;
        assert_eq!(app.frame_timeout(), IDLE_TICK);
    }

    #[test]
    fn banner_only_runs_fast_while_visible() {
        let mut app = app_with(Config { banner_animation: true, ..all_off() }, "banner-rate");
        assert_eq!(app.frame_timeout(), IDLE_TICK); // menú cerrado
        press(&mut app, KeyCode::Esc);
        app.menu_big = false; // menú compacto: no hay banner que ver
        assert_eq!(app.frame_timeout(), IDLE_TICK);
        app.menu_big = true;
        assert_eq!(app.frame_timeout(), ANIMATION_TICK);
        app.menu_opened = Some(ago(10)); // animación terminada
        assert_eq!(app.frame_timeout(), IDLE_TICK);
    }

    #[test]
    fn menu_transition_runs_fast_only_while_it_lasts() {
        let mut app = app_with(Config { menu_animation: true, ..all_off() }, "appear-rate");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.frame_timeout(), ANIMATION_TICK);
        app.menu_opened = Some(ago(1));
        assert_eq!(app.frame_timeout(), IDLE_TICK);
    }

    #[test]
    fn spinners_do_not_depend_on_the_animation_settings() {
        let mut app = app_with(all_off(), "spinner");
        app.set_status("Activando wg0…".into(), Kind::Info);
        assert_eq!(app.frame_timeout(), SPINNER_TICK);
        let spinner = app.spinner();
        assert!(!spinner.is_empty());
        let on = app_with(Config::default(), "spinner-on");
        assert_eq!(on.spinner().chars().count(), spinner.chars().count());
    }
}
