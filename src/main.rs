//! wgctui: cliente TUI para WireGuard. Activa y desactiva varios túneles a la vez.

mod config;
mod finder;
mod menu;
mod stars;
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
use stars::Stars;
use wg::{Msg, Tunnel, WG_DIR};

/// Si una lectura de datos tarda más que esto, se da por perdida y se permite pedir otra.
const REFRESH_STUCK: Duration = Duration::from_secs(5);
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

/// Ajustes de la pantalla OPTIONS, agrupados en pestañas como en runetui.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionItem {
    /// Frecuencia de actualización de los datos (como `update_ms` de btop).
    UpdateInterval,
    /// Contador de estrellas de GitHub en la cabecera (y única petición de red).
    GithubStars,
    HeaderAnimation,
    BannerAnimation,
    MenuAnimation,
}

impl OptionItem {
    /// Se cambia con Enter y después con `+` / `-` (o directamente con `+` / `-`).
    pub fn is_adjustable(self) -> bool {
        matches!(self, OptionItem::UpdateInterval)
    }

    /// El interruptor de animación que gobierna, si es uno de ellos.
    pub fn setting(self) -> Option<Setting> {
        match self {
            OptionItem::HeaderAnimation => Some(Setting::Header),
            OptionItem::BannerAnimation => Some(Setting::Banner),
            OptionItem::MenuAnimation => Some(Setting::Menu),
            OptionItem::UpdateInterval | OptionItem::GithubStars => None,
        }
    }
}

pub struct OptionTab {
    pub name: &'static str,
    pub items: &'static [OptionItem],
}

pub const OPTION_TABS: [OptionTab; 2] = [
    OptionTab {
        name: "General",
        items: &[OptionItem::UpdateInterval, OptionItem::GithubStars],
    },
    OptionTab {
        name: "Animaciones",
        items: &[
            OptionItem::HeaderAnimation,
            OptionItem::BannerAnimation,
            OptionItem::MenuAnimation,
        ],
    },
];

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
    /// Pestaña y ajuste seleccionados en la pantalla OPTIONS.
    pub options_tab: usize,
    pub options_selected: usize,
    /// Estrellas de GitHub del repositorio (se piden en segundo plano; `Off` si están ocultas).
    pub stars: Stars,
    /// Esquina derecha de la cabecera, que el menú no oscurece (la última dibujada).
    pub header_keep: Option<ratatui::layout::Rect>,
    /// Se pulsó Enter en un ajuste numérico: `+` / `-` lo cambian hasta Enter o Esc.
    pub adjusting: bool,
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
    /// Desde cuándo espera la última lectura (None = ya llegó).
    refresh_since: Option<Instant>,
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
            options_tab: 0,
            options_selected: 0,
            adjusting: false,
            stars: Stars::start(config.show_stars),
            header_keep: None,
            header_start: Instant::now(),
            menu_big: false,
            header_fits: true,
            finder: Finder::default(),
            tx,
            rx,
            refresh_id: 0,
            refresh_since: None,
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
        self.refresh_since = Some(self.last_refresh);
    }

    /// ¿Hay una lectura en marcha? Con frecuencias de 100 ms no se apilan hilos si una tarda.
    fn refresh_in_flight(&self) -> bool {
        self.refresh_since.is_some_and(|t| t.elapsed() < REFRESH_STUCK)
    }

    /// ¿Toca la lectura periódica según la frecuencia de actualización elegida?
    fn refresh_due(&self) -> bool {
        self.last_refresh.elapsed() >= self.config.update_every() && !self.refresh_in_flight()
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
        // Ventana mínima entre dos muestras: 0,5 s como siempre, o la mitad de la frecuencia
        // elegida si es más corta (así 100 ms sigue dando velocidades y gráfica).
        let min_dt = (self.config.update_every().as_secs_f64() * 0.5).min(0.5);
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
                    if dt >= min_dt {
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
                self.refresh_since = None;
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
                    self.options_tab = 0;
                    self.options_selected = 0;
                    self.adjusting = false;
                }
                1 => self.overlay = Overlay::Help,
                _ => self.quit = true,
            },
            _ => {}
        }
    }

    /// Opciones, como en runetui: pestañas arriba, ajustes a la izquierda y la descripción del
    /// elegido a la derecha. `↑↓` mueven, `←→` o Tab cambian de pestaña, Enter o Espacio cambian
    /// el ajuste (en los numéricos, `+` / `-` hasta Enter o Esc). Esc vuelve al menú sin repetir
    /// su animación.
    fn options_key(&mut self, code: KeyCode) {
        if self.adjusting {
            match code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.adjusting = false,
                KeyCode::Char('+' | '=') | KeyCode::Right | KeyCode::Up => self.adjust_update(true),
                KeyCode::Char('-' | '_') | KeyCode::Left | KeyCode::Down => self.adjust_update(false),
                _ => {}
            }
            return;
        }
        let tabs = OPTION_TABS.len();
        let items = OPTION_TABS[self.options_tab].items.len();
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.overlay = Overlay::Menu;
                self.menu_opened = None;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.options_selected = (self.options_selected + items - 1) % items
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.options_selected = (self.options_selected + 1) % items
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                self.options_tab = (self.options_tab + tabs - 1) % tabs;
                self.options_selected = 0;
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                self.options_tab = (self.options_tab + 1) % tabs;
                self.options_selected = 0;
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_option(self.current_option()),
            // Como en btop, `+` / `-` también cambian la frecuencia sin pasar por Enter.
            KeyCode::Char('+' | '=') if self.current_option().is_adjustable() => {
                self.adjust_update(true)
            }
            KeyCode::Char('-' | '_') if self.current_option().is_adjustable() => {
                self.adjust_update(false)
            }
            _ => {}
        }
    }

    /// El ajuste seleccionado ahora mismo en la pantalla OPTIONS.
    pub fn current_option(&self) -> OptionItem {
        let items = OPTION_TABS[self.options_tab.min(OPTION_TABS.len() - 1)].items;
        items[self.options_selected.min(items.len() - 1)]
    }

    fn activate_option(&mut self, item: OptionItem) {
        match (item, item.setting()) {
            (_, Some(setting)) => self.toggle_setting(setting),
            (OptionItem::GithubStars, None) => self.toggle_stars(),
            (_, None) => self.adjusting = true,
        }
    }

    /// Muestra u oculta las estrellas. Al ocultarlas no se hace ninguna petición de red.
    fn toggle_stars(&mut self) {
        self.config.show_stars = !self.config.show_stars;
        self.stars.set_enabled(self.config.show_stars);
        self.save_config();
    }

    /// Cambia un interruptor de animación y guarda los ajustes.
    fn toggle_setting(&mut self, setting: Setting) {
        let on = self.config.toggle(setting);
        if setting == Setting::Header && on {
            self.header_start = Instant::now(); // el texto vuelve a empezar por el barrido
        }
        self.save_config();
    }

    /// Sube (más lenta) o baja (más rápida) la frecuencia de actualización un paso y guarda.
    fn adjust_update(&mut self, up: bool) {
        let before = self.config.update_ms;
        if self.config.step_update(up) != before {
            self.save_config();
        }
    }

    fn save_config(&mut self) {
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
            self.stars.poll();
            if self.refresh_due() {
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
            ..Config::default()
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

    /// Abre OPTIONS desde el menú de Esc.
    fn open_options(app: &mut App) {
        press(app, KeyCode::Esc);
        press(app, KeyCode::Enter);
        assert!(app.overlay == Overlay::Options);
    }

    fn switches(app: &App) -> (bool, bool, bool) {
        (
            app.config.header_animation,
            app.config.banner_animation,
            app.config.menu_animation,
        )
    }

    fn cleanup(app: &App) {
        let _ = std::fs::remove_file(&app.config_path);
    }

    #[test]
    fn options_has_a_general_tab_and_an_animations_tab() {
        assert_eq!(OPTION_TABS.len(), 2);
        assert_eq!(OPTION_TABS[0].name, "General");
        assert_eq!(
            OPTION_TABS[0].items,
            &[OptionItem::UpdateInterval, OptionItem::GithubStars]
        );
        assert_eq!(OPTION_TABS[1].name, "Animaciones");
        assert_eq!(
            OPTION_TABS[1].items,
            &[
                OptionItem::HeaderAnimation,
                OptionItem::BannerAnimation,
                OptionItem::MenuAnimation
            ]
        );
        assert!(OptionItem::UpdateInterval.is_adjustable());
        assert!(!OptionItem::BannerAnimation.is_adjustable());
    }

    #[test]
    fn options_open_on_the_first_item_of_the_first_tab() {
        let mut app = app_with(Config::default(), "options-open");
        open_options(&mut app);
        assert_eq!((app.options_tab, app.options_selected), (0, 0));
        assert_eq!(app.current_option(), OptionItem::UpdateInterval);
        assert!(!app.adjusting);
    }

    #[test]
    fn left_right_and_tab_switch_tabs_and_wrap() {
        let mut app = app_with(Config::default(), "options-tabs");
        open_options(&mut app);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.options_tab, 1);
        assert_eq!(app.current_option(), OptionItem::HeaderAnimation);
        press(&mut app, KeyCode::Right); // da la vuelta
        assert_eq!(app.options_tab, 0);
        press(&mut app, KeyCode::Left); // y hacia atrás
        assert_eq!(app.options_tab, 1);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.options_tab, 0);
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.options_tab, 1);
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.options_tab, 0);
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.options_tab, 1);
        // Cambiar de pestaña vuelve al primer ajuste y no cambia ningún interruptor.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Left);
        assert_eq!((app.options_tab, app.options_selected), (0, 0));
        assert_eq!(switches(&app), (true, true, true));
    }

    #[test]
    fn up_down_move_within_the_tab_and_wrap() {
        let mut app = app_with(Config::default(), "options-updown");
        open_options(&mut app);
        press(&mut app, KeyCode::Down); // General: intervalo y estrellas
        assert_eq!(app.current_option(), OptionItem::GithubStars);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.current_option(), OptionItem::UpdateInterval);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.current_option(), OptionItem::GithubStars);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.options_selected, 0);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.current_option(), OptionItem::BannerAnimation);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.current_option(), OptionItem::MenuAnimation);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.current_option(), OptionItem::HeaderAnimation);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.current_option(), OptionItem::MenuAnimation);
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.current_option(), OptionItem::BannerAnimation);
    }

    #[test]
    fn animations_tab_toggles_one_switch_at_a_time() {
        let mut app = app_with(Config::default(), "options-toggle");
        open_options(&mut app);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter); // Header
        assert_eq!(switches(&app), (false, true, true));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' ')); // Banner
        assert_eq!(switches(&app), (false, false, true));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter); // Menu
        assert_eq!(switches(&app), (false, false, false));
        press(&mut app, KeyCode::Enter); // otra vez: se reactiva
        assert_eq!(switches(&app), (false, false, true));
        // La frecuencia no se toca al cambiar animaciones.
        assert_eq!(app.config.update_ms, 1000);
        cleanup(&app);
    }

    #[test]
    fn esc_in_options_goes_back_to_the_menu_and_reopening_starts_over() {
        let mut app = app_with(Config::default(), "options-esc");
        open_options(&mut app);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Esc);
        assert!(app.overlay == Overlay::Menu);
        press(&mut app, KeyCode::Enter);
        assert_eq!((app.options_tab, app.options_selected), (0, 0));
        press(&mut app, KeyCode::Char('q'));
        assert!(app.overlay == Overlay::Menu);
    }

    #[test]
    fn enter_on_the_update_interval_starts_adjusting() {
        let mut app = app_with(Config::default(), "adjust-enter");
        open_options(&mut app);
        press(&mut app, KeyCode::Enter);
        assert!(app.adjusting);
        assert_eq!(app.config.update_ms, 1000); // solo entra en modo ajuste
        press(&mut app, KeyCode::Enter);
        assert!(!app.adjusting);
        press(&mut app, KeyCode::Char(' '));
        assert!(app.adjusting);
        press(&mut app, KeyCode::Esc);
        assert!(!app.adjusting);
        assert!(app.overlay == Overlay::Options, "Esc solo termina el ajuste");
    }

    #[test]
    fn plus_and_minus_change_the_update_interval_while_adjusting() {
        let mut app = app_with(Config::default(), "adjust-keys");
        open_options(&mut app);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.config.update_ms, 1500);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.config.update_ms, 2000);
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Char('='));
        assert_eq!(app.config.update_ms, 3000);
        press(&mut app, KeyCode::Char('-'));
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('_'));
        assert_eq!(app.config.update_ms, 1000);
        press(&mut app, KeyCode::Char('-'));
        assert_eq!(app.config.update_ms, 900);
        assert!(app.adjusting, "sigue ajustando hasta Enter o Esc");
        cleanup(&app);
    }

    #[test]
    fn plus_and_minus_work_without_pressing_enter_first_like_btop() {
        let mut app = app_with(Config::default(), "adjust-direct");
        open_options(&mut app);
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.config.update_ms, 1500);
        press(&mut app, KeyCode::Char('-'));
        press(&mut app, KeyCode::Char('-'));
        assert_eq!(app.config.update_ms, 900);
        assert!(!app.adjusting);
        // En una animación, + y - no hacen nada.
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('+'));
        press(&mut app, KeyCode::Char('-'));
        assert_eq!((app.config.update_ms, switches(&app)), (900, (true, true, true)));
        cleanup(&app);
    }

    #[test]
    fn the_update_interval_stays_between_100_and_20000() {
        let mut app = app_with(Config::default(), "adjust-limits");
        open_options(&mut app);
        press(&mut app, KeyCode::Enter);
        for _ in 0..30 {
            press(&mut app, KeyCode::Char('-'));
        }
        assert_eq!(app.config.update_ms, 100);
        for _ in 0..80 {
            press(&mut app, KeyCode::Char('+'));
        }
        assert_eq!(app.config.update_ms, 20_000);
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.config.update_ms, 20_000);
        cleanup(&app);
    }

    #[test]
    fn while_adjusting_the_navigation_keys_do_not_leave_the_setting() {
        let mut app = app_with(Config::default(), "adjust-locked");
        open_options(&mut app);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!((app.options_tab, app.options_selected), (0, 0));
        assert!(app.adjusting);
        cleanup(&app);
    }

    #[test]
    fn changes_are_saved_right_away_with_all_four_settings() {
        let mut app = app_with(Config::default(), "persist");
        open_options(&mut app);
        press(&mut app, KeyCode::Char('-')); // 900 ms
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter); // Banner off
        let text = std::fs::read_to_string(&app.config_path).unwrap();
        assert!(text.contains("update_ms = 900"));
        assert!(text.contains("header_animation = on"));
        assert!(text.contains("banner_animation = off"));
        assert!(text.contains("menu_animation = on"));
        assert_eq!(
            Config::load_from(&app.config_path),
            Config { update_ms: 900, banner_animation: false, ..Config::default() }
        );
        cleanup(&app);
    }

    #[test]
    fn a_failed_save_is_reported_but_the_setting_still_changes() {
        let mut app = app_with(Config::default(), "save-fail");
        app.config_path = PathBuf::from("/no/existe/wgctui.conf");
        app.overlay = Overlay::Options;
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.config.update_ms, 1500);
        assert!(matches!(app.visible_status(), Some((_, Kind::Err))));
        app.status.clear();
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter);
        assert!(!app.config.header_animation);
        assert!(matches!(app.visible_status(), Some((_, Kind::Err))));
    }

    #[test]
    fn a_step_that_changes_nothing_does_not_write_the_file() {
        let mut app = app_with(Config { update_ms: 100, ..Config::default() }, "no-write");
        app.overlay = Overlay::Options;
        press(&mut app, KeyCode::Char('-'));
        assert!(!app.config_path.exists());
    }

    #[test]
    fn reenabling_the_header_restarts_its_sweep() {
        let mut app = app_with(Config::default(), "header-restart");
        app.header_start = ago(30);
        assert!(!app.header_animation().is_some_and(menu::header_sweeping));
        open_options(&mut app);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter); // off
        assert!(app.header_animation().is_none());
        press(&mut app, KeyCode::Enter); // on
        assert!(app.header_animation().is_some_and(menu::header_sweeping));
        cleanup(&app);
    }

    // ------------------------------------------------------------- estrellas

    fn select_stars(app: &mut App) {
        open_options(app);
        press(app, KeyCode::Down);
        assert_eq!(app.current_option(), OptionItem::GithubStars);
    }

    #[test]
    fn stars_are_on_by_default_and_start_loading() {
        let app = app_with(Config::default(), "stars-default");
        assert!(app.config.show_stars);
        assert_ne!(app.stars.state(), stars::StarsState::Off);
    }

    #[test]
    fn a_config_with_stars_off_starts_without_them() {
        let app = app_with(Config { show_stars: false, ..Config::default() }, "stars-off-start");
        assert_eq!(app.stars.state(), stars::StarsState::Off);
    }

    #[test]
    fn the_stars_option_hides_and_shows_them_and_is_saved() {
        let mut app = app_with(Config::default(), "stars-toggle");
        select_stars(&mut app);
        press(&mut app, KeyCode::Enter);
        assert!(!app.config.show_stars);
        assert_eq!(app.stars.state(), stars::StarsState::Off);
        let text = std::fs::read_to_string(&app.config_path).unwrap();
        assert!(text.contains("show_stars = off"), "{text}");
        assert_eq!(Config::load_from(&app.config_path), Config { show_stars: false, ..Config::default() });
        press(&mut app, KeyCode::Char(' '));
        assert!(app.config.show_stars);
        assert_ne!(app.stars.state(), stars::StarsState::Off);
        assert!(std::fs::read_to_string(&app.config_path).unwrap().contains("show_stars = on"));
        cleanup(&app);
    }

    #[test]
    fn the_stars_option_does_not_touch_the_other_settings() {
        let mut app = app_with(Config::default(), "stars-independent");
        select_stars(&mut app);
        press(&mut app, KeyCode::Enter);
        assert_eq!(switches(&app), (true, true, true));
        assert_eq!(app.config.update_ms, 1000);
        assert!(!app.adjusting, "no es un ajuste numérico");
        // Y al revés: cambiar animaciones o frecuencia no toca las estrellas.
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Char('-'));
        assert!(!app.config.show_stars);
        assert_eq!(app.stars.state(), stars::StarsState::Off);
        cleanup(&app);
    }

    #[test]
    fn plus_and_minus_do_nothing_on_the_stars_option() {
        let mut app = app_with(Config::default(), "stars-plusminus");
        select_stars(&mut app);
        press(&mut app, KeyCode::Char('+'));
        press(&mut app, KeyCode::Char('-'));
        assert_eq!(app.config.update_ms, 1000);
        assert!(app.config.show_stars);
    }

    #[test]
    fn the_stars_poll_in_the_loop_picks_up_the_count() {
        let mut app = app_with(Config::default(), "stars-poll");
        app.stars.set_for_test(stars::StarsState::Count(7));
        app.stars.poll(); // sin hilo de fondo no cambia nada
        assert_eq!(app.stars.state(), stars::StarsState::Count(7));
    }

    #[test]
    fn toggling_the_stars_does_not_change_how_fast_the_loop_runs() {
        let mut app = app_with(all_off(), "stars-rate");
        assert_eq!(app.frame_timeout(), IDLE_TICK);
        select_stars(&mut app);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.frame_timeout(), IDLE_TICK);
        cleanup(&app);
    }

    // ------------------------------------------------- frecuencia de actualización

    #[test]
    fn the_data_is_read_every_second_by_default() {
        let mut app = app_with(Config::default(), "due-default");
        assert!(!app.refresh_due());
        app.last_refresh = ago(2);
        assert!(app.refresh_due());
    }

    #[test]
    fn the_update_interval_decides_when_a_refresh_is_due() {
        let mut app = app_with(Config { update_ms: 20_000, ..Config::default() }, "due-slow");
        app.last_refresh = ago(5);
        assert!(!app.refresh_due(), "con 20000 ms, 5 s no bastan");
        app.last_refresh = ago(21);
        assert!(app.refresh_due());

        let mut app = app_with(Config { update_ms: 100, ..Config::default() }, "due-fast");
        app.last_refresh = Instant::now()
            .checked_sub(Duration::from_millis(150))
            .unwrap();
        assert!(app.refresh_due(), "con 100 ms, 150 ms ya tocan");
        app.last_refresh = Instant::now();
        assert!(!app.refresh_due());
    }

    #[test]
    fn changing_the_interval_applies_at_once() {
        let mut app = app_with(Config { update_ms: 20_000, ..Config::default() }, "due-change");
        app.last_refresh = ago(3);
        assert!(!app.refresh_due());
        app.overlay = Overlay::Options;
        for _ in 0..50 {
            press(&mut app, KeyCode::Char('-'));
        }
        assert_eq!(app.config.update_ms, 100);
        assert!(app.refresh_due());
        cleanup(&app);
    }

    #[test]
    fn a_slow_read_is_not_stacked_with_another_one() {
        let mut app = app_with(Config { update_ms: 100, ..Config::default() }, "due-flight");
        app.last_refresh = ago(2);
        app.refresh_since = Some(ago(1)); // todavía en marcha
        assert!(!app.refresh_due());
        app.refresh_since = Some(ago(30)); // se dio por perdida
        assert!(app.refresh_due());
        app.refresh_since = None; // ya llegó
        assert!(app.refresh_due());
    }

    #[test]
    fn the_reply_of_the_current_read_frees_the_next_one() {
        let mut app = app_with(Config { update_ms: 100, ..Config::default() }, "due-reply");
        app.refresh_id = 7;
        app.refresh_since = Some(Instant::now());
        app.last_refresh = ago(2);
        // Una respuesta vieja no cuenta.
        app.on_msg(Msg::Snapshot(6, Err(io::Error::other("vieja"))));
        assert!(app.refresh_in_flight());
        app.on_msg(Msg::Snapshot(7, Ok(wg::Snapshot { tunnels: Vec::new(), at: Instant::now() })));
        assert!(!app.refresh_in_flight());
        assert!(app.refresh_due());
    }

    fn tunnel_with_counters(rx: u64, tx: u64) -> Tunnel {
        Tunnel {
            name: "wg0".into(),
            active: true,
            busy: None,
            conf: String::new(),
            address: String::new(),
            endpoint: String::new(),
            public_key: String::new(),
            listen_port: String::new(),
            peers: vec![wg::Peer { rx, tx, ..wg::Peer::default() }],
        }
    }

    #[test]
    fn speeds_are_measured_even_with_a_100_ms_interval() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(100);

        let mut fast = app_with(Config { update_ms: 100, ..Config::default() }, "rate-fast");
        fast.tunnels = vec![tunnel_with_counters(0, 0)];
        fast.update_rates(t0);
        fast.tunnels = vec![tunnel_with_counters(1000, 500)];
        fast.update_rates(t1);
        let (down, up) = fast.rates["wg0"];
        assert!((down - 10_000.0).abs() < 1.0 && (up - 5_000.0).abs() < 1.0, "{down} {up}");
        assert_eq!(fast.history["wg0"].len(), 1);

        // Con 1000 ms (lo de siempre), 100 ms entre muestras es demasiado poco y se espera.
        let mut normal = app_with(Config::default(), "rate-normal");
        normal.tunnels = vec![tunnel_with_counters(0, 0)];
        normal.update_rates(t0);
        normal.tunnels = vec![tunnel_with_counters(1000, 500)];
        normal.update_rates(t1);
        assert!(normal.rates.is_empty());
        normal.update_rates(t0 + Duration::from_millis(600));
        assert!(normal.rates.contains_key("wg0"));
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
