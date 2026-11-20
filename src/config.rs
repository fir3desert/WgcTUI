//! Ajustes de wgctui.
//!
//! El programa siempre acaba corriendo como root (`wg::ensure_root` se relanza con sudo),
//! así que el archivo vive en /etc y no depende del HOME que deje sudo.
//!
//! Formato (una clave por línea, `#` para comentarios):
//!
//! ```text
//! header_animation = on
//! banner_animation = on
//! menu_animation = on
//! ```
//!
//! El parseo es tolerante: claves en mayúsculas o con espacios, líneas rotas o claves
//! desconocidas se ignoran y lo que falte queda en su valor por defecto (todo activado).
//! La clave antigua `animations` (un único interruptor) sigue entendiéndose: fija los tres
//! ajustes a la vez, y las claves nuevas que vengan después la sobrescriben.

use std::{fs, io, path::Path};

pub const CONFIG_PATH: &str = "/etc/wgctui.conf";

/// Cada una de las animaciones que se pueden activar o desactivar por separado.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    /// Texto animado de la esquina superior izquierda.
    Header,
    /// Animación del banner grande del menú de Esc (barrido de luz y onda).
    Banner,
    /// Animación de aparición del propio menú de Esc.
    Menu,
}

impl Setting {
    /// En el orden en que salen en la pantalla OPTIONS.
    pub const ALL: [Setting; 3] = [Setting::Header, Setting::Banner, Setting::Menu];

    /// Clave en el archivo de configuración.
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
    pub header_animation: bool,
    pub banner_animation: bool,
    pub menu_animation: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            header_animation: true,
            banner_animation: true,
            menu_animation: true,
        }
    }
}

/// `Some(valor)` si el texto es un sí/no reconocible; `None` si no se entiende.
fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

impl Config {
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

    /// Invierte un ajuste y devuelve su nuevo valor.
    pub fn toggle(&mut self, setting: Setting) -> bool {
        let value = !self.get(setting);
        self.set(setting, value);
        value
    }

    /// Lee el archivo; si no existe o está roto, valores por defecto.
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
            let Some(value) = parse_bool(value) else {
                continue; // valor que no se entiende: se queda lo que hubiera
            };
            let key = key.trim().to_lowercase();
            if key == "animations" {
                // Clave antigua: un solo interruptor para todo.
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
        // Mayúsculas, espacios y distintas formas de decir "no".
        assert!(!Config::parse("  header_animation=FALSE ").header_animation);
        assert!(!Config::parse("# wgctui\nbanner_animation = 0\n").banner_animation);
        assert!(!Config::parse("MENU_ANIMATION = No\n").menu_animation);
        // Valores raros, claves desconocidas y líneas rotas no estropean nada.
        let c = Config::parse("header_animation = rarísimo\nfoo = off\nsin igual\n= off\n\n");
        assert_eq!(c, ALL_ON);
        // Una línea rota no impide leer las siguientes.
        let c = Config::parse("???\nmenu_animation = off\n");
        assert!(!c.menu_animation);
    }

    #[test]
    fn last_value_wins_and_garbage_keeps_the_previous_one() {
        assert!(Config::parse("menu_animation = off\nmenu_animation = on\n").menu_animation);
        assert!(!Config::parse("menu_animation = off\nmenu_animation = ¿?\n").menu_animation);
    }

    #[test]
    fn legacy_single_switch_still_works() {
        let off = Config::parse("animations = off\n");
        assert!(Setting::ALL.into_iter().all(|s| !off.get(s)));
        assert_eq!(Config::parse("animations = on\n"), ALL_ON);
        // Las claves nuevas posteriores mandan sobre la antigua.
        let mixed = Config::parse("animations = off\nbanner_animation = on\n");
        assert_eq!(
            (mixed.header_animation, mixed.banner_animation, mixed.menu_animation),
            (false, true, false)
        );
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
        assert!(text.contains("header_animation = on\n"));
        assert!(text.contains("banner_animation = on\n"));
        assert!(text.contains("menu_animation = on\n"));

        let mut c = Config::default();
        c.set(Setting::Header, false);
        c.set(Setting::Menu, false);
        let text = c.render();
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
        c.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path), c);
        let _ = fs::remove_file(&path);
        // Sin archivo: valores por defecto.
        assert_eq!(Config::load_from(&path), ALL_ON);
    }
}
