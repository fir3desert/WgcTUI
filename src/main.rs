//! wgctui: cliente TUI para WireGuard. Activa y desactiva varios túneles a la vez.

mod menu;
mod theme;
mod ui;
mod wg;

use std::{
    collections::{HashMap, VecDeque},
    io,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    widgets::ListState,
};

use wg::{Msg, Tunnel, WG_DIR};

const REFRESH_EVERY: Duration = Duration::from_secs(1);
/// Muestras de velocidad que se guardan para la gráfica (una por refresco).
const HISTORY: usize = 60;

/// El barrido del banner necesita unos 60 fps. Con el sondeo fijo de 50 ms de
/// antes se dibujaba a 20 fps y la animación se notaba entrecortada.
const ANIMATION_TICK: Duration = Duration::from_millis(16);
/// Mientras gira el spinner basta con unos 12 fps.
const SPINNER_TICK: Duration = Duration::from_millis(80);
/// Sin animación que seguir, se dibuja menos para no gastar CPU.
const IDLE_TICK: Duration = Duration::from_millis(100);

/// Entradas del menú de Esc, en orden.
pub const MENU_ITEMS: [&str; 3] = ["RESUME", "HELP", "QUIT"];

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
    Help,
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
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// Id de la última instantánea pedida: sirve para descartar respuestas viejas.
    refresh_id: u64,
    last_refresh: Instant,
    quit: bool,
}

impl App {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let mut app = App {
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
            tx,
            rx,
            refresh_id: 0,
            last_refresh: Instant::now(),
            quit: false,
        };
        app.set_status("Cargando túneles…".into(), Kind::Info);
        app.refresh();
        app
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

    /// Cuánto lleva abierta la animación del banner (None = banner estático).
    pub fn menu_animation(&self) -> Option<Duration> {
        if self.overlay != Overlay::Menu {
            return None;
        }
        self.menu_opened
            .map(|opened| opened.elapsed())
            .filter(|elapsed| elapsed.as_millis() < u128::from(menu::ANIMATION_MS))
    }

    /// Cadencia del bucle: fluida mientras hay animación, pausada cuando no hay nada que ver.
    fn frame_timeout(&self) -> Duration {
        if self.menu_animation().is_some() {
            ANIMATION_TICK
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
            Overlay::Help => {
                // Cualquier tecla cierra la ayuda y vuelve al menú (sin repetir la animación).
                self.overlay = Overlay::Menu;
                self.menu_opened = None;
            }
            Overlay::None => self.list_key(code),
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
                0 => self.close_menu(),
                1 => self.overlay = Overlay::Help,
                _ => self.quit = true,
            },
            _ => {}
        }
    }

    fn list_key(&mut self, code: KeyCode) {
        match code {
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
