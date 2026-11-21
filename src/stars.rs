//! GitHub stars for the wgctui repository, shown in the header's upper-right corner.
//!
//! This is wgctui's only network access. A background thread fetches the count once at startup
//! using the system `curl`; it never blocks the interface, and the result is cached for one hour.
//! It can be disabled using the `GitHub stars` option (OPTIONS → General) or `show_stars = off`
//! in `/etc/wgctui.conf`. If the API request fails (for example, due to rate limiting), the
//! public repository page is tried instead. If both requests fail, the header shows a dash.
//!
//! The program always runs as root, so the cache lives in /var/cache and does not depend on
//! the HOME directory inherited by sudo.

use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Repository whose stars are shown.
pub const REPO: &str = "fir3desert/WgcTUI";
/// How long a cached value is trusted before querying GitHub again.
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);
/// Cache file format: `"<stars> <unix timestamp>"`.
const CACHE_PATH: &str = "/var/cache/wgctui/stars.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarsState {
    /// Disabled in options: show nothing and make no requests.
    Off,
    /// The request is in progress and there is no cached value.
    Loading,
    /// The count could not be retrieved.
    Unavailable,
    Count(u64),
}

#[derive(Debug)]
pub struct Stars {
    state: StarsState,
    receiver: Option<Receiver<Option<u64>>>,
}

impl Stars {
    /// Start fetching the count if `enabled`. Use a recent cached value as-is; display a stale
    /// value while requesting an update.
    pub fn start(enabled: bool) -> Self {
        if !enabled {
            return Self::off();
        }
        // Tests never access the network or the real cache.
        if cfg!(test) {
            return Self {
                state: StarsState::Loading,
                receiver: None,
            };
        }
        let cached = read_cache(Path::new(CACHE_PATH), now());
        if let Some((count, true)) = cached {
            return Self {
                state: StarsState::Count(count),
                receiver: None,
            };
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
        Self {
            state: StarsState::Off,
            receiver: None,
        }
    }

    /// Collect the background request result, if it has arrived.
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
                // A stale cached count is better than replacing it with a dash.
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

    /// Enable or disable this feature from the options screen.
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

/// Header-corner label: `★ 42`, `★ 1.2k`, etc.; `None` when disabled.
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

/// Extract `stargazers_count` from the GitHub API JSON response.
pub fn parse_star_count(json: &str) -> Option<u64> {
    parse_count_after_key(json, "\"stargazers_count\"")
}

/// Extract `stargazerCount` from the JSON embedded in the public repository page.
fn parse_page_star_count(page: &str) -> Option<u64> {
    parse_count_after_key(page, "\"stargazerCount\"")
}

fn parse_count_after_key(text: &str, key: &str) -> Option<u64> {
    let after = &text[text.find(key)? + key.len()..];
    let after = after.trim_start().strip_prefix(':')?.trim_start();
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn fetch_count(repo: &str) -> Option<u64> {
    let api_url = format!("https://api.github.com/repos/{repo}");
    fetch_url(&api_url, true)
        .as_deref()
        .and_then(parse_star_count)
        .or_else(|| {
            let page_url = format!("https://github.com/{repo}");
            fetch_url(&page_url, false)
                .as_deref()
                .and_then(parse_page_star_count)
        })
}

fn fetch_url(url: &str, is_api: bool) -> Option<String> {
    let mut command = Command::new("curl");
    command.args(["--silent", "--fail", "--location", "--max-time", "6"]);
    if is_api {
        command.args(["--header", "Accept: application/vnd.github+json"]);
    }
    let output = command
        .args(["--header", "User-Agent: wgctui", url])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Cached value as `(stars, is_recent)`.
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
    fn parses_the_count_from_the_repository_page_as_a_fallback() {
        let page = r#"{"sidebarAboutData":{"stargazerCount":1,"watcherCount":0}}"#;
        assert_eq!(parse_page_star_count(page), Some(1));
        assert_eq!(parse_page_star_count(r#"{"stargazerCount":0}"#), Some(0));
        assert_eq!(parse_page_star_count(r#"{"stargazerCount":"many"}"#), None);
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
        assert_eq!(
            read_cache(&path, 1_000 + CACHE_TTL.as_secs() + 1),
            Some((77, false))
        );
        fs::write(&path, "garbage").unwrap();
        assert_eq!(read_cache(&path, 1_000), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_request_keeps_the_cached_count() {
        let (sender, receiver) = mpsc::channel();
        let mut stars = Stars {
            state: StarsState::Count(9),
            receiver: Some(receiver),
        };
        sender.send(None).unwrap();
        stars.poll();
        assert_eq!(stars.state(), StarsState::Count(9));

        let (sender, receiver) = mpsc::channel();
        let mut stars = Stars {
            state: StarsState::Loading,
            receiver: Some(receiver),
        };
        sender.send(None).unwrap();
        stars.poll();
        assert_eq!(stars.state(), StarsState::Unavailable);

        let (sender, receiver) = mpsc::channel();
        let mut stars = Stars {
            state: StarsState::Loading,
            receiver: Some(receiver),
        };
        sender.send(Some(5)).unwrap();
        stars.poll();
        assert_eq!(stars.state(), StarsState::Count(5));
    }

    #[test]
    fn a_dropped_worker_means_unavailable() {
        let (sender, receiver) = mpsc::channel::<Option<u64>>();
        let mut stars = Stars {
            state: StarsState::Loading,
            receiver: Some(receiver),
        };
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
        stars.set_enabled(true); // Already enabled: keep the current count.
        assert_eq!(stars.state(), StarsState::Count(3));
        stars.set_enabled(false);
        assert_eq!(stars.state(), StarsState::Off);
    }
}
