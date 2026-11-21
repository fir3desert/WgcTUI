//! wgctui configuration.
//!
//! The program always runs as root (`wg::ensure_root` relaunches it with sudo), so the file
//! lives in /etc and does not depend on the HOME directory inherited by sudo.
//!
//! Format (one key per line, `#` for comments):
//!
//! ```text
//! update_ms = 1000
//! show_stars = on
//! header_animation = on
//! banner_animation = on
//! menu_animation = on
//! ```
//!
//! `update_ms` is the update interval (as in btop): how often WireGuard data is read.
//! The default is 1000 ms; the range is 100 to 20000.
//!
//! `show_stars` shows or hides the GitHub star count in the header. When set to `off`, wgctui
//! makes no network requests.
//!
//! Parsing is tolerant: uppercase keys, extra whitespace, malformed lines, and unknown keys
//! are handled gracefully. Missing values keep their defaults (all enabled, 1000 ms). An
//! out-of-range `update_ms` value is clamped to the nearest limit. The legacy `animations` key
//! (a single switch) is still supported: it sets all three options, and any newer keys that
//! follow override it.

use std::{fs, io, path::Path};

pub const CONFIG_PATH: &str = "/etc/wgctui.conf";

/// Default, minimum, and maximum update intervals, in milliseconds.
pub const DEFAULT_UPDATE_MS: u32 = 1000;
pub const MIN_UPDATE_MS: u32 = 100;
pub const MAX_UPDATE_MS: u32 = 20_000;
/// Up to `UPDATE_FINE_UNTIL` (inclusive), step by 100 ms; above it, step by 500 ms.
const UPDATE_FINE_UNTIL: u32 = 1000;
const UPDATE_FINE_STEP: u32 = 100;
const UPDATE_COARSE_STEP: u32 = 500;

/// Next interval when increasing it (slowing down); never exceeds `MAX_UPDATE_MS`.
/// Also handles values that are not on the step grid (e.g. 1234 → 1500).
pub fn update_ms_up(ms: u32) -> u32 {
    let step = if ms < UPDATE_FINE_UNTIL { UPDATE_FINE_STEP } else { UPDATE_COARSE_STEP };
    ((ms / step + 1) * step).clamp(MIN_UPDATE_MS, MAX_UPDATE_MS)
}

/// Next interval when decreasing it (speeding up); never goes below `MIN_UPDATE_MS`.
pub fn update_ms_down(ms: u32) -> u32 {
    let step = if ms <= UPDATE_FINE_UNTIL { UPDATE_FINE_STEP } else { UPDATE_COARSE_STEP };
    (ms.saturating_sub(1) / step * step).clamp(MIN_UPDATE_MS, MAX_UPDATE_MS)
}

/// An animation that can be enabled or disabled independently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    /// Animated text in the upper-left corner.
    Header,
    /// Large Esc-menu banner animation (light sweep and wave).
    Banner,
    /// Esc-menu entrance animation.
    Menu,
}

impl Setting {
    /// In the order shown on the OPTIONS screen.
    pub const ALL: [Setting; 3] = [Setting::Header, Setting::Banner, Setting::Menu];

    /// Key in the configuration file.
    pub const fn key(self) -> &'static str {
        match self {
            Setting::Header => "header_animation",
            Setting::Banner => "banner_animation",
            Setting::Menu => "menu_animation",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Data update interval in milliseconds (100..=20000).
    pub update_ms: u32,
    /// GitHub star count in the upper-right corner of the header.
    pub show_stars: bool,
    pub header_animation: bool,
    pub banner_animation: bool,
    pub menu_animation: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            update_ms: DEFAULT_UPDATE_MS,
            show_stars: true,
            header_animation: true,
            banner_animation: true,
            menu_animation: true,
        }
    }
}

/// `Some(value)` if the text is a recognized yes/no value; `None` if it is unrecognized.
fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

impl Config {
    /// Update interval as a `Duration`.
    pub fn update_every(&self) -> std::time::Duration {
        std::time::Duration::from_millis(u64::from(self.update_ms))
    }

    /// Increase (`up`) or decrease the update interval by one step and return the new value.
    pub fn step_update(&mut self, up: bool) -> u32 {
        self.update_ms = if up { update_ms_up(self.update_ms) } else { update_ms_down(self.update_ms) };
        self.update_ms
    }

    pub fn get(&self, setting: Setting) -> bool {
        match setting {
            Setting::Header => self.header_animation,
            Setting::Banner => self.banner_animation,
            Setting::Menu => self.menu_animation,
        }
    }

    pub fn set(&mut self, setting: Setting, value: bool) {
        match setting {
            Setting::Header => self.header_animation = value,
            Setting::Banner => self.banner_animation = value,
            Setting::Menu => self.menu_animation = value,
        }
    }

    /// Toggle a setting and return its new value.
    pub fn toggle(&mut self, setting: Setting) -> bool {
        let value = !self.get(setting);
        self.set(setting, value);
        value
    }

    /// Read the file; use defaults if it is missing or malformed.
    pub fn load() -> Self {
        Self::load_from(CONFIG_PATH)
    }

    pub fn load_from(path: impl AsRef<Path>) -> Self {
        fs::read_to_string(path)
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Self {
        let mut config = Config::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().to_lowercase();
            if key == "update_ms" {
                // Accept a number with or without "ms"; keep the previous value if unrecognized.
                let digits = value.trim().trim_end_matches("ms").trim();
                if let Ok(ms) = digits.parse::<i64>() {
                    config.update_ms = ms.clamp(i64::from(MIN_UPDATE_MS), i64::from(MAX_UPDATE_MS)) as u32;
                }
                continue;
            }
            let Some(value) = parse_bool(value) else {
                continue; // Unrecognized value: keep the previous one.
            };
            if key == "show_stars" {
                config.show_stars = value;
            } else if key == "animations" {
                // Legacy key: a single switch for all animations.
                for setting in Setting::ALL {
                    config.set(setting, value);
                }
            } else if let Some(setting) = Setting::ALL.into_iter().find(|s| s.key() == key) {
                config.set(setting, value);
            }
        }
        config
    }

    pub fn save(&self) -> io::Result<()> {
        self.save_to(CONFIG_PATH)
    }

    pub fn save_to(&self, path: impl AsRef<Path>) -> io::Result<()> {
        fs::write(path, self.render())
    }

    fn render(&self) -> String {
        let mut text = String::from("# wgctui\n");
        text.push_str(&format!("update_ms = {}\n", self.update_ms));
        text.push_str(&format!("show_stars = {}\n", if self.show_stars { "on" } else { "off" }));
        for setting in Setting::ALL {
            let value = if self.get(setting) { "on" } else { "off" };
            text.push_str(&format!("{} = {value}\n", setting.key()));
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_ON: Config = Config {
        update_ms: 1000,
        show_stars: true,
        header_animation: true,
        banner_animation: true,
        menu_animation: true,
    };

    #[test]
    fn everything_is_on_by_default() {
        assert_eq!(Config::default(), ALL_ON);
        assert_eq!(Config::parse(""), ALL_ON);
        assert_eq!(Config::parse("# nada\nheader_animation = on\n"), ALL_ON);
    }

    #[test]
    fn each_setting_turns_off_on_its_own() {
        let c = Config::parse("header_animation = off\n");
        assert_eq!((c.header_animation, c.banner_animation, c.menu_animation), (false, true, true));
        let c = Config::parse("banner_animation = off\n");
        assert_eq!((c.header_animation, c.banner_animation, c.menu_animation), (true, false, true));
        let c = Config::parse("menu_animation = off\n");
        assert_eq!((c.header_animation, c.banner_animation, c.menu_animation), (true, true, false));
    }

    #[test]
    fn parsing_is_tolerant() {
        // Uppercase keys, whitespace, and different ways to express "no".
        assert!(!Config::parse("  header_animation=FALSE ").header_animation);
        assert!(!Config::parse("# wgctui\nbanner_animation = 0\n").banner_animation);
        assert!(!Config::parse("MENU_ANIMATION = No\n").menu_animation);
        // Unusual values, unknown keys, and malformed lines do not break parsing.
        let c = Config::parse("header_animation = nonsense\nfoo = off\nno equals\n= off\n\n");
        assert_eq!(c, ALL_ON);
        // A malformed line does not prevent subsequent lines from being read.
        let c = Config::parse("???\nmenu_animation = off\n");
        assert!(!c.menu_animation);
    }

    #[test]
    fn last_value_wins_and_garbage_keeps_the_previous_one() {
        assert!(Config::parse("menu_animation = off\nmenu_animation = on\n").menu_animation);
        assert!(!Config::parse("menu_animation = off\nmenu_animation = ???\n").menu_animation);
    }

    #[test]
    fn legacy_single_switch_still_works() {
        let off = Config::parse("animations = off\n");
        assert!(Setting::ALL.into_iter().all(|s| !off.get(s)));
        assert_eq!(Config::parse("animations = on\n"), ALL_ON);
        // Newer keys take precedence over the legacy key.
        let mixed = Config::parse("animations = off\nbanner_animation = on\n");
        assert_eq!(
            (mixed.header_animation, mixed.banner_animation, mixed.menu_animation),
            (false, true, false)
        );
    }

    #[test]
    fn update_rate_defaults_to_one_second() {
        assert_eq!(Config::default().update_ms, 1000);
        assert_eq!(Config::parse("").update_ms, 1000);
        assert_eq!(Config::default().update_every(), std::time::Duration::from_secs(1));
    }

    #[test]
    fn update_rate_is_read_and_clamped_to_the_range() {
        assert_eq!(Config::parse("update_ms = 250\n").update_ms, 250);
        assert_eq!(Config::parse("  UPDATE_MS=5000 ms ").update_ms, 5000);
        assert_eq!(Config::parse("update_ms = 100\n").update_ms, 100);
        assert_eq!(Config::parse("update_ms = 20000\n").update_ms, 20_000);
        // Out of range: clamp to the nearest limit.
        assert_eq!(Config::parse("update_ms = 5\n").update_ms, 100);
        assert_eq!(Config::parse("update_ms = 0\n").update_ms, 100);
        assert_eq!(Config::parse("update_ms = -50\n").update_ms, 100);
        assert_eq!(Config::parse("update_ms = 999999999\n").update_ms, 20_000);
        // Invalid input is ignored; keep the previous value.
        assert_eq!(Config::parse("update_ms = fast\n").update_ms, 1000);
        assert_eq!(Config::parse("update_ms = 300\nupdate_ms = ???\n").update_ms, 300);
        assert_eq!(Config::parse("update_ms = 1.5\n").update_ms, 1000);
    }

    #[test]
    fn update_rate_does_not_disturb_the_animation_switches() {
        let c = Config::parse("update_ms = 2000\nbanner_animation = off\n");
        assert_eq!(c.update_ms, 2000);
        assert_eq!((c.header_animation, c.banner_animation, c.menu_animation), (true, false, true));
        // The legacy key does not affect the interval.
        assert_eq!(Config::parse("update_ms = 300\nanimations = off\n").update_ms, 300);
    }

    #[test]
    fn stepping_goes_by_100_up_to_a_second_and_by_500_after() {
        assert_eq!(update_ms_up(100), 200);
        assert_eq!(update_ms_up(900), 1000);
        assert_eq!(update_ms_up(1000), 1500);
        assert_eq!(update_ms_up(1500), 2000);
        assert_eq!(update_ms_up(19_500), 20_000);
        assert_eq!(update_ms_down(20_000), 19_500);
        assert_eq!(update_ms_down(1500), 1000);
        assert_eq!(update_ms_down(1000), 900);
        assert_eq!(update_ms_down(200), 100);
    }

    #[test]
    fn stepping_stops_at_the_limits() {
        assert_eq!(update_ms_up(20_000), 20_000);
        assert_eq!(update_ms_down(100), 100);
        let mut c = Config { update_ms: 100, ..Config::default() };
        assert_eq!(c.step_update(false), 100);
        c.update_ms = 20_000;
        assert_eq!(c.step_update(true), 20_000);
    }

    #[test]
    fn stepping_handles_values_off_the_grid() {
        assert_eq!(update_ms_up(1234), 1500);
        assert_eq!(update_ms_down(1234), 1000);
        assert_eq!(update_ms_up(150), 200);
        assert_eq!(update_ms_down(150), 100);
        assert_eq!(update_ms_up(950), 1000);
    }

    #[test]
    fn every_step_from_the_bottom_reaches_the_top_and_back() {
        let mut ms = MIN_UPDATE_MS;
        let mut steps = 0;
        while ms < MAX_UPDATE_MS {
            let next = update_ms_up(ms);
            assert!(next > ms);
            assert_eq!(update_ms_down(next), ms, "subir y bajar deben ser inversos ({ms})");
            ms = next;
            steps += 1;
        }
        assert_eq!(steps, 9 + 38); // 100→1000 en 9 pasos y 1000→20000 en 38
    }

    #[test]
    fn stars_are_shown_by_default_and_can_be_hidden() {
        assert!(Config::default().show_stars);
        assert!(Config::parse("").show_stars);
        assert!(Config::parse("show_stars = on\n").show_stars);
        assert!(!Config::parse("show_stars = off\n").show_stars);
        assert!(!Config::parse("  SHOW_STARS=False ").show_stars);
        assert!(!Config::parse("show_stars = 0\n").show_stars);
        // An unrecognized value keeps the previous value.
        assert!(Config::parse("show_stars = maybe\n").show_stars);
        assert!(!Config::parse("show_stars = off\nshow_stars = ???\n").show_stars);
    }

    #[test]
    fn stars_do_not_mix_with_the_animation_switches_or_the_interval() {
        let c = Config::parse("show_stars = off\n");
        assert_eq!((c.header_animation, c.banner_animation, c.menu_animation), (true, true, true));
        assert_eq!(c.update_ms, 1000);
        // The legacy animation key does not affect stars.
        assert!(Config::parse("animations = off\n").show_stars);
        assert!(!Config::parse("show_stars = off\nanimations = on\n").show_stars);
        let c = Config::parse("header_animation = off\nupdate_ms = 400\n");
        assert!(c.show_stars);
    }

    #[test]
    fn toggle_flips_only_one_setting() {
        let mut c = Config::default();
        assert!(!c.toggle(Setting::Banner));
        assert_eq!(
            (c.header_animation, c.banner_animation, c.menu_animation),
            (true, false, true)
        );
        assert!(c.toggle(Setting::Banner));
        assert_eq!(c, ALL_ON);
    }

    #[test]
    fn rendered_file_has_the_three_keys_and_round_trips() {
        let text = ALL_ON.render();
        assert!(text.contains("update_ms = 1000\n"));
        assert!(text.contains("show_stars = on\n"));
        assert!(text.contains("header_animation = on\n"));
        assert!(text.contains("banner_animation = on\n"));
        assert!(text.contains("menu_animation = on\n"));

        let mut c = Config::default();
        c.set(Setting::Header, false);
        c.set(Setting::Menu, false);
        c.update_ms = 350;
        c.show_stars = false;
        let text = c.render();
        assert!(text.contains("update_ms = 350\n"));
        assert!(text.contains("show_stars = off\n"));
        assert!(text.contains("header_animation = off\n"));
        assert!(text.contains("banner_animation = on\n"));
        assert!(text.contains("menu_animation = off\n"));
        assert_eq!(Config::parse(&text), c);
    }

    #[test]
    fn save_and_load_use_the_given_path() {
        let path = std::env::temp_dir().join(format!("wgctui-test-{}.conf", std::process::id()));
        let mut c = Config::default();
        c.set(Setting::Banner, false);
        c.update_ms = 4500;
        c.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path), c);
        let _ = fs::remove_file(&path);
        // Missing file: use defaults.
        assert_eq!(Config::load_from(&path), ALL_ON);
    }
}
