//! wgctui: a WireGuard TUI client for enabling and disabling multiple tunnels.

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

/// A data read is considered stuck after this delay, allowing another read to be requested.
const REFRESH_STUCK: Duration = Duration::from_secs(5);
/// Speed samples retained for the graph (one per refresh).
const HISTORY: usize = 60;

/// The banner sweep needs about 60 fps. The previous fixed 50 ms polling interval rendered
/// at 20 fps and made the animation look choppy.
const ANIMATION_TICK: Duration = Duration::from_millis(16);
/// About 12 fps is enough while the spinner is active.
const SPINNER_TICK: Duration = Duration::from_millis(80);
/// The header's endless wave does not need to be as smooth as the sweep.
const HEADER_TICK: Duration = Duration::from_millis(33);
/// Draw less often when there is no active, visible animation to save CPU.
const IDLE_TICK: Duration = Duration::from_millis(100);

/// Esc-menu entries, in order (the first is selected when the menu opens).
pub const MENU_ITEMS: [&str; 3] = ["OPTIONS", "HELP", "QUIT"];

/// OPTIONS-screen settings, grouped into tabs like runetui.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionItem {
    /// Data update interval (like btop's `update_ms`).
    UpdateInterval,
    /// GitHub star count in the header (and the only network access).
    GithubStars,
    HeaderAnimation,
    BannerAnimation,
    MenuAnimation,
}

impl OptionItem {
    /// Adjust with `Enter` followed by `+` / `-`, or with `+` / `-` directly.
    pub fn is_adjustable(self) -> bool {
        matches!(self, OptionItem::UpdateInterval)
    }

    /// The animation switch controlled by this option, if any.
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
        name: "Animations",
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
    /// Persisted settings; each animation can be enabled or disabled independently.
    pub config: Config,
    /// Where settings are stored (a temporary file in tests).
    config_path: PathBuf,
    /// Selected tab and setting on the OPTIONS screen.
    pub options_tab: usize,
    pub options_selected: usize,
    /// Repository's GitHub stars (fetched in the background; `Off` when hidden).
    pub stars: Stars,
    /// Right corner of the header, which the menu does not dim (from the last draw).
    pub header_keep: Option<ratatui::layout::Rect>,
    /// `Enter` was pressed on a numeric setting; `+` / `-` adjust it until `Enter` or `Esc`.
    pub adjusting: bool,
    /// When the animated header text started (the sweep restarts when re-enabled).
    header_start: Instant,
    /// What was visible in the last rendered frame: the large menu (with banner) and the
    /// animated header text. The loop uses these only to determine its cadence.
    pub menu_big: bool,
    pub header_fits: bool,
    pub finder: Finder,
    /// Tunnel selected when the finder opened (`Esc` returns to it).
    pub search_origin: Option<usize>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// ID of the last requested snapshot, used to discard stale responses.
    refresh_id: u64,
    /// When the last read was requested (`None` = it has arrived).
    refresh_since: Option<Instant>,
    last_refresh: Instant,
    quit: bool,
}

impl App {
    fn new() -> Self {
        let mut app = Self::with_config(Config::load());
        app.set_status("Loading tunnels…".into(), Kind::Info);
        app.refresh();
        app
    }

    /// Initial state with the given settings, without touching disk or spawning threads.
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
            search_origin: None,
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

    /// The status message while it has not expired.
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

    /// How long the menu has been open while the banner animation (light sweep and wave) is
    /// running. `None`: the banner is static and finished (animation disabled or complete).
    pub fn banner_animation(&self) -> Option<Duration> {
        if !self.config.banner_animation || self.overlay != Overlay::Menu {
            return None;
        }
        self.menu_opened
            .map(|opened| opened.elapsed())
            .filter(|elapsed| elapsed.as_millis() < u128::from(menu::ANIMATION_MS))
    }

    /// How long the menu has been open while its entrance transition is running.
    /// `None`: the menu is fully visible (or appeared instantly with animation disabled).
    pub fn menu_appear(&self) -> Option<Duration> {
        if !self.config.menu_animation || self.overlay != Overlay::Menu {
            return None;
        }
        self.menu_opened
            .map(|opened| opened.elapsed())
            .filter(|elapsed| elapsed.as_millis() < u128::from(menu::APPEAR_MS))
    }

    /// Elapsed time for the header text (`None` = static, animation disabled).
    pub fn header_animation(&self) -> Option<Duration> {
        self.config
            .header_animation
            .then(|| self.header_start.elapsed())
    }

    /// Is animated header text currently visible?
    fn header_visible(&self) -> bool {
        self.config.header_animation && self.header_fits
    }

    /// Loop cadence: fast only while an animation is active and visible; returns to idle when
    /// all animations are disabled or off-screen.
    fn frame_timeout(&self) -> Duration {
        let banner = self.menu_big && self.banner_animation().is_some();
        let appearing = self.menu_appear().is_some();
        let header_sweep = self.header_visible()
            && self.header_animation().is_some_and(menu::header_sweeping);
        if banner || appearing || header_sweep {
            ANIMATION_TICK
        } else if self.header_visible() {
            HEADER_TICK // The header text's wave never ends.
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

    /// Request data on a worker thread. `wg show all dump` and reads from `/etc/wireguard`
    /// block; doing this on the draw thread froze the animation every second.
    fn refresh(&mut self) {
        self.refresh_id += 1;
        wg::spawn_refresh(self.refresh_id, self.tx.clone());
        self.last_refresh = Instant::now();
        self.refresh_since = Some(self.last_refresh);
    }

    /// Is a read in progress? At 100 ms intervals, do not stack threads if a read is slow.
    fn refresh_in_flight(&self) -> bool {
        self.refresh_since.is_some_and(|t| t.elapsed() < REFRESH_STUCK)
    }

    /// Is a periodic read due according to the selected update interval?
    fn refresh_due(&self) -> bool {
        self.last_refresh.elapsed() >= self.config.update_every() && !self.refresh_in_flight()
    }

    /// Apply a loaded snapshot while preserving the selection and in-progress operations.
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
            self.set_status(format!("No .conf files found in {WG_DIR}"), Kind::Err);
        }
    }

    /// Download/upload speed calculated from counters in successive snapshots, plus the
    /// history used by the graph. `now` is when the counters were sampled.
    fn update_rates(&mut self, now: Instant) {
        // Minimum time between samples: 0.5 s as before, or half the selected interval if
        // shorter (so 100 ms still produces speed values and a graph).
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
            return; // An operation is already in progress for this tunnel.
        }
        let up = !t.active;
        t.busy = Some(up);
        let name = t.name.clone();
        self.set_status(
            format!("{} {}…", if up { "Enabling" } else { "Disabling" }, name),
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
                        format!("{} {}", name, if up { "enabled" } else { "disabled" }),
                        Kind::Ok,
                    ),
                    Err(e) => self.set_status(
                        format!(
                            "Error {} {}: {e}",
                            if up { "enabling" } else { "disabling" },
                            name
                        ),
                        Kind::Err,
                    ),
                }
                self.refresh();
            }
            Msg::Snapshot(id, result) => {
                // This snapshot is stale if a newer one has already been requested.
                if id != self.refresh_id {
                    return;
                }
                self.refresh_since = None;
                match result {
                    Ok(snap) => self.apply_snapshot(snap),
                    Err(e) => self.set_status(format!("Could not read {WG_DIR}: {e}"), Kind::Err),
                }
            }
        }
    }

    // ------------------------------------------------------------ keyboard

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
                // Any key closes help and returns to the menu without replaying its animation.
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

    /// Options, like runetui: tabs at the top, settings on the left, and the selected setting's
    /// description on the right. `↑↓` move, `←→` or `Tab` switch tabs, and `Enter` or `Space`
    /// toggle a setting (`+` / `-` adjust numeric settings until `Enter` or `Esc`). `Esc`
    /// returns to the menu without replaying its animation.
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
            // As in btop, `+` / `-` also adjust the interval without pressing Enter first.
            KeyCode::Char('+' | '=') if self.current_option().is_adjustable() => {
                self.adjust_update(true)
            }
            KeyCode::Char('-' | '_') if self.current_option().is_adjustable() => {
                self.adjust_update(false)
            }
            _ => {}
        }
    }

    /// The setting currently selected on the OPTIONS screen.
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

    /// Show or hide the stars. No network request is made while they are hidden.
    fn toggle_stars(&mut self) {
        self.config.show_stars = !self.config.show_stars;
        self.stars.set_enabled(self.config.show_stars);
        self.save_config();
    }

    /// Toggle an animation switch and save the settings.
    fn toggle_setting(&mut self, setting: Setting) {
        let on = self.config.toggle(setting);
        if setting == Setting::Header && on {
            self.header_start = Instant::now(); // Restart the header sweep.
        }
        self.save_config();
    }

    /// Increase (slower) or decrease (faster) the update interval by one step and save it.
    fn adjust_update(&mut self, up: bool) {
        let before = self.config.update_ms;
        if self.config.step_update(up) != before {
            self.save_config();
        }
    }

    fn save_config(&mut self) {
        if let Err(e) = self.config.save_to(&self.config_path) {
            self.set_status(
                format!("Could not save {}: {e}", self.config_path.display()),
                Kind::Err,
            );
        }
    }

    // ------------------------------------------------------------ search

    fn open_search(&mut self) {
        self.finder.open();
        self.search_origin = self.state.selected();
        self.overlay = Overlay::Search;
    }

    /// Move the selection to the tunnel matching the query; if the query is empty or has no
    /// matches, return to the tunnel that was selected when search opened.
    fn search_follow(&mut self) {
        let hits = self.finder.hits(&self.tunnels);
        if self.finder.query.trim().is_empty() || hits.is_empty() {
            self.search_restore();
            return;
        }
        let sel = self.finder.selected.min(hits.len() - 1);
        self.finder.selected = sel;
        self.select(hits[sel].index);
    }

    fn search_restore(&mut self) {
        if let Some(origin) = self.search_origin.filter(|o| *o < self.tunnels.len()) {
            if self.state.selected() != Some(origin) {
                self.select(origin);
            }
        }
    }

    /// Search in the status bar, like runetui's `/`: typing jumps to the best tunnel; `↑/↓`
    /// (or `Tab`, `Ctrl+N` / `Ctrl+P`) cycle through matches; `Enter` keeps the selection and
    /// `Esc` cancels and returns to the previous tunnel.
    fn search_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        let alt = mods.contains(KeyModifiers::ALT);
        let count = self.finder.hits(&self.tunnels).len();
        let last = count.saturating_sub(1);
        let selected = self.finder.selected.min(last);
        match (code, ctrl) {
            (KeyCode::Esc, _) => {
                self.search_restore();
                self.overlay = Overlay::None;
            }
            (KeyCode::Enter, _) => {
                if count == 0 {
                    self.search_restore();
                }
                self.overlay = Overlay::None;
            }
            (KeyCode::Down | KeyCode::Tab, _) | (KeyCode::Char('n'), true) => {
                if count > 0 {
                    self.finder.selected = (selected + 1) % count;
                    self.search_follow();
                }
            }
            (KeyCode::Up | KeyCode::BackTab, _) | (KeyCode::Char('p'), true) => {
                if count > 0 {
                    self.finder.selected = (selected + count - 1) % count;
                    self.search_follow();
                }
            }
            (KeyCode::Backspace, _) => {
                self.finder.backspace();
                self.search_follow();
            }
            (KeyCode::Char('u'), true) => {
                self.finder.clear_query();
                self.search_follow();
            }
            (KeyCode::Char('w'), true) => {
                self.finder.delete_word();
                self.search_follow();
            }
            // Any other character is query text, including `q`; `Esc` closes the search.
            (KeyCode::Char(c), false) if !alt => {
                self.finder.type_char(c);
                self.search_follow();
            }
            _ => {}
        }
    }

    fn list_key(&mut self, code: KeyCode, _mods: KeyModifiers) {
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

            // Calculate the timeout after drawing: if this key opened the menu, the next frame
            // already runs at 60 fps.
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

    /// Test app: settings use a dedicated temporary file (never touches /etc).
    fn app_with(config: Config, tag: &str) -> App {
        let mut app = App::with_config(config);
        app.config_path =
            std::env::temp_dir().join(format!("wgctui-app-{}-{tag}.conf", std::process::id()));
        app
    }

    fn ago(secs: u64) -> Instant {
        Instant::now()
            .checked_sub(Duration::from_secs(secs))
            .expect("the monotonic clock has been running for more than a few seconds")
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
        press(&mut app, KeyCode::Esc); // Return to the menu.
        assert!(app.overlay == Overlay::Menu);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter); // HELP
        assert!(app.overlay == Overlay::Help);
        press(&mut app, KeyCode::Char('x')); // Any key returns.
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

    /// Open OPTIONS from the Esc menu.
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
        assert_eq!(OPTION_TABS[1].name, "Animations");
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
        press(&mut app, KeyCode::Right);
        assert_eq!(app.options_tab, 0);
        press(&mut app, KeyCode::Right); // Wrap around.
        assert_eq!(app.options_tab, 1);
        press(&mut app, KeyCode::Left); // And go back.
        assert_eq!(app.options_tab, 0);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.options_tab, 1);
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.options_tab, 0);
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.options_tab, 1);
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.options_tab, 0);
        // Switching tabs returns to the first setting and does not change any switches.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Left);
        assert_eq!((app.options_tab, app.options_selected), (1, 0));
        assert_eq!(switches(&app), (true, true, true));
    }

    #[test]
    fn up_down_move_within_the_tab_and_wrap() {
        let mut app = app_with(Config::default(), "options-updown");
        open_options(&mut app);
        press(&mut app, KeyCode::Down); // General: interval and stars.
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
        press(&mut app, KeyCode::Enter); // Header.
        assert_eq!(switches(&app), (false, true, true));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' ')); // Banner.
        assert_eq!(switches(&app), (false, false, true));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter); // Menu.
        assert_eq!(switches(&app), (false, false, false));
        press(&mut app, KeyCode::Enter); // Toggle it back on.
        assert_eq!(switches(&app), (false, false, true));
        // Changing animations does not affect the update interval.
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
        assert_eq!(app.config.update_ms, 1000); // Only enter adjustment mode.
        press(&mut app, KeyCode::Enter);
        assert!(!app.adjusting);
        press(&mut app, KeyCode::Char(' '));
        assert!(app.adjusting);
        press(&mut app, KeyCode::Esc);
        assert!(!app.adjusting);
        assert!(app.overlay == Overlay::Options, "Esc only ends adjustment");
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
        assert!(app.adjusting, "adjustment continues until Enter or Esc");
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
        // `+` and `-` have no effect on an animation setting.
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
        press(&mut app, KeyCode::Char('-')); // 900 ms.
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter); // Turn the banner off.
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
        app.config_path = PathBuf::from("/no/such/wgctui.conf");
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
        press(&mut app, KeyCode::Enter); // Off.
        assert!(app.header_animation().is_none());
        press(&mut app, KeyCode::Enter); // On.
        assert!(app.header_animation().is_some_and(menu::header_sweeping));
        cleanup(&app);
    }

    // ------------------------------------------------------------- stars

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
        assert!(!app.adjusting, "this is not a numeric setting");
        // Conversely, changing animations or the interval does not affect the stars.
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
        app.stars.poll(); // Without a background thread, nothing changes.
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

    // ------------------------------------------------- update interval

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
        assert!(!app.refresh_due(), "5 s is not enough at 20000 ms");
        app.last_refresh = ago(21);
        assert!(app.refresh_due());

        let mut app = app_with(Config { update_ms: 100, ..Config::default() }, "due-fast");
        app.last_refresh = Instant::now()
            .checked_sub(Duration::from_millis(150))
            .unwrap();
        assert!(app.refresh_due(), "150 ms is enough at a 100 ms interval");
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
        app.refresh_since = Some(ago(1)); // Still in progress.
        assert!(!app.refresh_due());
        app.refresh_since = Some(ago(30)); // Considered stuck.
        assert!(app.refresh_due());
        app.refresh_since = None; // It has arrived.
        assert!(app.refresh_due());
    }

    #[test]
    fn the_reply_of_the_current_read_frees_the_next_one() {
        let mut app = app_with(Config { update_ms: 100, ..Config::default() }, "due-reply");
        app.refresh_id = 7;
        app.refresh_since = Some(Instant::now());
        app.last_refresh = ago(2);
        // A stale response does not count.
        app.on_msg(Msg::Snapshot(6, Err(io::Error::other("stale"))));
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

        // With the default 1000 ms interval, 100 ms between samples is too short.
        let mut normal = app_with(Config::default(), "rate-normal");
        normal.tunnels = vec![tunnel_with_counters(0, 0)];
        normal.update_rates(t0);
        normal.tunnels = vec![tunnel_with_counters(1000, 500)];
        normal.update_rates(t1);
        assert!(normal.rates.is_empty());
        normal.update_rates(t0 + Duration::from_millis(600));
        assert!(normal.rates.contains_key("wg0"));
    }

    // ------------------------------------------------- independent animations

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

    // ------------------------------------------------------------ performance

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
        assert_eq!(app.frame_timeout(), HEADER_TICK); // Endless wave.
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
        assert_eq!(app.frame_timeout(), IDLE_TICK); // Menu closed.
        press(&mut app, KeyCode::Esc);
        app.menu_big = false; // Compact menu: no banner is visible.
        assert_eq!(app.frame_timeout(), IDLE_TICK);
        app.menu_big = true;
        assert_eq!(app.frame_timeout(), ANIMATION_TICK);
        app.menu_opened = Some(ago(10)); // Animation finished.
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
        app.set_status("Enabling wg0…".into(), Kind::Info);
        assert_eq!(app.frame_timeout(), SPINNER_TICK);
        let spinner = app.spinner();
        assert!(!spinner.is_empty());
        let on = app_with(Config::default(), "spinner-on");
        assert_eq!(on.spinner().chars().count(), spinner.chars().count());
    }

    // ---------------------------------------------------------- inline search

    fn tunnel_named(name: &str) -> Tunnel {
        Tunnel {
            name: name.to_string(),
            active: false,
            busy: None,
            conf: String::new(),
            address: String::new(),
            endpoint: String::new(),
            public_key: String::new(),
            listen_port: String::new(),
            peers: Vec::new(),
        }
    }

    fn searching_app() -> App {
        let mut app = app_with(all_off(), "search");
        app.tunnels = ["alpha", "cabin", "jrd", "limestone", "spare"].map(tunnel_named).into_iter().collect();
        app.state.select(Some(2));
        app
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(KeyCode::Char(c), NONE);
        }
    }

    #[test]
    fn slash_opens_the_search_and_typing_jumps_to_the_best_match() {
        let mut app = searching_app();
        app.on_key(KeyCode::Char('/'), NONE);
        assert!(app.overlay == Overlay::Search);
        type_text(&mut app, "lim");
        assert_eq!(app.state.selected(), Some(3));
        // `q` is query text, not a quit command.
        type_text(&mut app, "q");
        assert!(!app.quit);
        app.on_key(KeyCode::Backspace, NONE);
        assert_eq!(app.state.selected(), Some(3));
    }

    #[test]
    fn ctrl_f_does_not_open_the_search_and_ctrl_p_does_not_either() {
        let mut app = searching_app();
        app.on_key(KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert!(app.overlay == Overlay::None);
        app.on_key(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert!(app.overlay == Overlay::None);
    }

    #[test]
    fn the_search_only_looks_at_tunnel_names_not_inside_the_config() {
        let mut app = searching_app();
        app.tunnels[0].conf = "Endpoint = zzz.example.com:51820".into();
        app.on_key(KeyCode::Char('/'), NONE);
        type_text(&mut app, "zzz");
        assert_eq!(app.state.selected(), Some(2), "without a match, selection stays where it was");
    }

    #[test]
    fn enter_keeps_the_found_tunnel_and_esc_goes_back() {
        let mut app = searching_app();
        app.on_key(KeyCode::Char('/'), NONE);
        type_text(&mut app, "spa");
        app.on_key(KeyCode::Enter, NONE);
        assert!(app.overlay == Overlay::None);
        assert_eq!(app.state.selected(), Some(4));

        let mut app = searching_app();
        app.on_key(KeyCode::Char('/'), NONE);
        type_text(&mut app, "spa");
        assert_eq!(app.state.selected(), Some(4));
        app.on_key(KeyCode::Esc, NONE);
        assert!(app.overlay == Overlay::None);
        assert_eq!(app.state.selected(), Some(2), "Esc returns to the previous tunnel");
    }

    #[test]
    fn arrows_cycle_through_the_matches() {
        let mut app = searching_app();
        app.on_key(KeyCode::Char('/'), NONE);
        type_text(&mut app, "a"); // alpha, cabin, spare
        let first = app.state.selected();
        app.on_key(KeyCode::Down, NONE);
        let second = app.state.selected();
        assert_ne!(first, second);
        app.on_key(KeyCode::Down, NONE);
        app.on_key(KeyCode::Down, NONE);
        assert_eq!(app.state.selected(), first, "selection wraps around");
        app.on_key(KeyCode::Up, NONE);
        assert_ne!(app.state.selected(), first);
    }

    #[test]
    fn the_search_prompt_lives_in_the_status_bar() {
        let mut app = searching_app();
        app.on_key(KeyCode::Char('/'), NONE);
        type_text(&mut app, "li");
        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::draw(&mut app, f)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let row = |y: u16| (0..100u16).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        let status = row(23);
        assert!(status.contains("SEARCH"), "{status:?}");
        assert!(status.contains("/li"), "{status:?}");
        assert!(status.contains("1/1"), "{status:?}");
        // There is no longer a floating window over the list.
        let screen: String = (0..24u16).map(row).collect::<Vec<_>>().join("\n");
        assert!(!screen.contains("Search tunnel"));
    }
}
