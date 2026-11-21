//! Tunnel finder (`/`), like runetui's file finder.
//!
//! It uses the same algorithm as runetui: the typed text must appear in order within the name
//! (fuzzy search). Matches score higher when letters are consecutive, start a word (`_`, `-`,
//! `.`, or uppercase), or fall within the file name. Matched letters are returned for
//! highlighting.

use std::cmp::Reverse;

use crate::wg::Tunnel;

// ------------------------------------------------------------------ matching

const SCORE_MATCH: i32 = 16;
/// First character of the path or of a folder / file name.
const BONUS_START: i32 = 10;
/// Character after `_`, `-`, `.` or a space.
const BONUS_SEPARATOR: i32 = 8;
/// Upper-case letter after a lower-case one (`fooBar`).
const BONUS_CAMEL: i32 = 7;
/// Character after other punctuation.
const BONUS_OTHER: i32 = 6;
/// A matched character right after the previous matched one.
const BONUS_CONSECUTIVE: i32 = 9;
/// Every matched character that lies in the file name.
const BONUS_NAME_CHAR: i32 = 3;
/// The whole query matched inside the file name.
const BONUS_ALL_IN_NAME: i32 = 30;
/// Cost of every character skipped between two matches.
const GAP: i32 = 2;
const NEG: i32 = -1_000_000_000;

fn valid(score: i32) -> bool {
    score > NEG / 2
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i32,
    /// Character (not byte) indexes of the candidate that matched the query.
    pub positions: Vec<usize>,
}

/// Turn what the user typed into the characters that are matched: spaces are
/// ignored, `\` is a `/` and case does not matter.
pub fn normalize_query(query: &str) -> Vec<char> {
    query
        .chars()
        .filter(|character| !character.is_whitespace())
        .map(|character| if character == '\\' { '/' } else { fold(character) })
        .collect()
}

fn fold(character: char) -> char {
    character.to_lowercase().next().unwrap_or(character)
}

/// Reusable buffers for scoring many candidates without allocating each time.
#[derive(Default)]
pub struct Matcher {
    chars: Vec<char>,
    folded: Vec<char>,
    bonus: Vec<i32>,
    /// `ends[i * n + j]`: best score with query char `i` matched at `j`.
    ends: Vec<i32>,
    /// `upto[i * n + j]`: best score with query chars `0..=i` matched at or
    /// before `j`, minus the gap cost since the last match.
    upto: Vec<i32>,
}

impl Matcher {
    /// Score `candidate` for an already normalised `query` (see
    /// [`normalize_query`]).  `None` when the query is not a subsequence.
    pub fn score(&mut self, query: &[char], candidate: &str) -> Option<FuzzyMatch> {
        if query.is_empty() {
            return Some(FuzzyMatch {
                score: 0,
                positions: Vec::new(),
            });
        }
        self.chars.clear();
        self.chars.extend(candidate.chars());
        let n = self.chars.len();
        let m = query.len();
        if m > n {
            return None;
        }
        self.folded.clear();
        self.folded.extend(self.chars.iter().map(|c| fold(*c)));

        // Cheap rejection first: most candidates are not a subsequence.
        let mut next = 0;
        for character in &self.folded {
            if next < m && *character == query[next] {
                next += 1;
            }
        }
        if next < m {
            return None;
        }

        let name_start = self
            .chars
            .iter()
            .rposition(|c| *c == '/')
            .map_or(0, |slash| slash + 1);

        self.bonus.clear();
        for j in 0..n {
            let current = self.chars[j];
            let bonus = if j == 0 {
                BONUS_START
            } else {
                let previous = self.chars[j - 1];
                if previous == '/' {
                    BONUS_START
                } else if matches!(previous, '_' | '-' | '.' | ' ') {
                    BONUS_SEPARATOR
                } else if previous.is_lowercase() && current.is_uppercase() {
                    BONUS_CAMEL
                } else if !previous.is_alphanumeric() {
                    BONUS_OTHER
                } else {
                    0
                }
            };
            self.bonus.push(bonus);
        }

        self.ends.clear();
        self.ends.resize(m * n, NEG);
        self.upto.clear();
        self.upto.resize(m * n, NEG);

        for i in 0..m {
            for j in i..n {
                let at = i * n + j;
                if self.folded[j] == query[i] {
                    let name_bonus = if j >= name_start { BONUS_NAME_CHAR } else { 0 };
                    let best = if i == 0 {
                        self.bonus[j]
                    } else if j > 0 {
                        let before = (i - 1) * n + j - 1;
                        let mut best = NEG;
                        if valid(self.ends[before]) {
                            best = self.ends[before] + self.bonus[j].max(BONUS_CONSECUTIVE);
                        }
                        if valid(self.upto[before]) {
                            best = best.max(self.upto[before] + self.bonus[j]);
                        }
                        best
                    } else {
                        NEG
                    };
                    if valid(best) {
                        self.ends[at] = best + SCORE_MATCH + name_bonus;
                    }
                }
                let carried = if j > 0 && valid(self.upto[at - 1]) {
                    self.upto[at - 1] - GAP
                } else {
                    NEG
                };
                self.upto[at] = self.ends[at].max(carried);
            }
        }

        // The best alignment ends wherever the last query char scores most.
        let last = (m - 1) * n;
        let mut best_end = None;
        let mut best_score = NEG;
        for j in 0..n {
            let score = self.ends[last + j];
            if valid(score) && score > best_score {
                best_score = score;
                best_end = Some(j);
            }
        }
        let mut j = best_end?;

        // Walk back to find which characters were matched.
        let mut positions = vec![0usize; m];
        positions[m - 1] = j;
        for i in (1..m).rev() {
            let name_bonus = if j >= name_start { BONUS_NAME_CHAR } else { 0 };
            let target = self.ends[i * n + j];
            let before = (i - 1) * n + j - 1;
            let consecutive = valid(self.ends[before])
                && self.ends[before] + self.bonus[j].max(BONUS_CONSECUTIVE) + SCORE_MATCH + name_bonus
                    == target;
            if consecutive {
                j -= 1;
            } else {
                let carried = self.upto[before];
                let mut found = None;
                for previous in (i - 1..j).rev() {
                    let score = self.ends[(i - 1) * n + previous];
                    if valid(score) && score - GAP * (j - 1 - previous) as i32 == carried {
                        found = Some(previous);
                        break;
                    }
                }
                j = found?;
            }
            positions[i - 1] = j;
        }

        let mut score = best_score;
        if positions.iter().all(|position| *position >= name_start) {
            score += BONUS_ALL_IN_NAME;
        }
        Some(FuzzyMatch { score, positions })
    }
}

/// Score `candidate` against a raw query string.
#[cfg(test)]
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    Matcher::default().score(&normalize_query(query), candidate)
}

// ------------------------------------------------------------------ finder

/// A tunnel that matches the query.
pub struct Hit {
    /// Position of the tunnel in `App::tunnels`.
    pub index: usize,
    /// Character (not byte) indexes of matching letters in the name.
    pub positions: Vec<usize>,
}

/// Search state: the query and the selected result.
#[derive(Default)]
pub struct Finder {
    pub query: String,
    pub selected: usize,
}

impl Finder {
    pub fn open(&mut self) {
        self.query.clear();
        self.selected = 0;
    }

    pub fn type_char(&mut self, character: char) {
        self.query.push(character);
        self.selected = 0;
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    pub fn clear_query(&mut self) {
        self.query.clear();
        self.selected = 0;
    }

    /// Delete the last word (`Ctrl+W`).
    pub fn delete_word(&mut self) {
        let trimmed = self.query.trim_end().len();
        self.query.truncate(trimmed);
        while let Some(last) = self.query.chars().last() {
            if last.is_alphanumeric() {
                self.query.pop();
            } else {
                break;
            }
        }
        self.selected = 0;
    }

    /// Matching tunnels, best first. An empty query returns all tunnels in their original order
    /// without highlighted letters. This is recalculated each time; there are few tunnels, and
    /// this keeps results in sync with the list, which refreshes every second.
    pub fn hits(&self, tunnels: &[Tunnel]) -> Vec<Hit> {
        let query = normalize_query(&self.query);
        if query.is_empty() {
            return (0..tunnels.len())
                .map(|index| Hit {
                    index,
                    positions: Vec::new(),
                })
                .collect();
        }
        let mut matcher = Matcher::default();
        let mut scored: Vec<(i32, Hit)> = Vec::new();
        for (index, tunnel) in tunnels.iter().enumerate() {
            if let Some(found) = matcher.score(&query, &tunnel.name) {
                scored.push((
                    found.score,
                    Hit {
                        index,
                        positions: found.positions,
                    },
                ));
            }
        }
        scored.sort_by_key(|(score, hit)| (Reverse(*score), tunnels[hit.index].name.len(), hit.index));
        scored.into_iter().map(|(_, hit)| hit).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(query: &str, candidate: &str) -> Option<i32> {
        fuzzy_match(query, candidate).map(|found| found.score)
    }

    fn positions(query: &str, candidate: &str) -> Vec<usize> {
        fuzzy_match(query, candidate).expect("it matches").positions
    }

    #[test]
    fn only_subsequences_match() {
        assert!(fuzzy_match("mrs", "src/main.rs").is_some());
        assert!(fuzzy_match("rsm", "src/main.rs").is_none(), "order matters");
        assert!(fuzzy_match("xyz", "src/main.rs").is_none());
        assert!(fuzzy_match("mainn", "main").is_none(), "a longer query cannot match");
        assert!(fuzzy_match("aa", "a").is_none());
    }

    #[test]
    fn an_empty_query_matches_everything_without_highlights() {
        let found = fuzzy_match("", "anything").unwrap();
        assert_eq!(found.score, 0);
        assert!(found.positions.is_empty());
        assert!(fuzzy_match("   ", "anything").is_some(), "spaces are ignored");
    }

    #[test]
    fn case_and_backslashes_do_not_matter() {
        assert!(fuzzy_match("README", "readme.md").is_some());
        assert!(fuzzy_match("readme", "README.md").is_some());
        assert_eq!(positions("src\\main", "src/main.rs"), vec![0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(score("r m", "readme.md"), score("rm", "readme.md"));
    }

    #[test]
    fn consecutive_characters_beat_scattered_ones() {
        let tight = score("foo", "foo.rs").unwrap();
        let loose = score("foo", "f_o_o.rs").unwrap();
        assert!(tight > loose, "{tight} vs {loose}");
    }

    #[test]
    fn word_starts_beat_the_middle_of_a_word() {
        let boundary = score("fb", "foo_bar.rs").unwrap();
        let middle = score("fb", "fabric.rs").unwrap();
        assert!(boundary > middle, "{boundary} vs {middle}");
        let camel = score("fb", "fooBar.rs").unwrap();
        let flat = score("fb", "foobar.rs").unwrap();
        assert!(camel > flat, "{camel} vs {flat}");
    }

    #[test]
    fn the_file_name_beats_the_folders() {
        let name = score("main", "src/main.rs").unwrap();
        let folder = score("main", "docs/main_notes/readme.md").unwrap();
        assert!(name > folder, "{name} vs {folder}");
    }

    #[test]
    fn highlights_point_at_the_matched_characters() {
        assert_eq!(positions("mrs", "src/main.rs"), vec![4, 9, 10]);
        let found = fuzzy_match("rdm", "docs/README.md").unwrap();
        for position in &found.positions {
            let character = "docs/README.md".chars().nth(*position).unwrap();
            assert!("rdm".contains(character.to_ascii_lowercase()));
        }
        assert_eq!(found.positions.len(), 3);
        assert!(found.positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn positions_are_character_indexes_not_bytes() {
        // `é` and `ñ` take two bytes each.
        assert_eq!(positions("ni", "café/niño.md"), vec![5, 6]);
    }

    #[test]
    fn a_match_that_could_pick_a_bad_alignment_picks_the_good_one() {
        // `ab` appears early and scattered, and later as a word start.
        let found = fuzzy_match("ab", "a_x_x_x/ab.md").unwrap();
        assert_eq!(found.positions, vec![8, 9]);
    }


    fn tunnel(name: &str) -> Tunnel {
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

    fn names(finder: &Finder, tunnels: &[Tunnel]) -> Vec<String> {
        finder
            .hits(tunnels)
            .iter()
            .map(|hit| tunnels[hit.index].name.clone())
            .collect()
    }

    #[test]
    fn an_empty_query_lists_every_tunnel_in_order() {
        let tunnels = vec![tunnel("casa"), tunnel("empresa"), tunnel("oficina")];
        let finder = Finder::default();
        assert_eq!(names(&finder, &tunnels), vec!["casa", "empresa", "oficina"]);
        assert!(finder.hits(&tunnels).iter().all(|hit| hit.positions.is_empty()));
    }

    #[test]
    fn typing_filters_and_ranks_the_tunnels() {
        let tunnels = vec![tunnel("oficina-madrid"), tunnel("casa"), tunnel("oficina")];
        let mut finder = Finder::default();
        for character in "ofi".chars() {
            finder.type_char(character);
        }
        // Both match "ofi"; the shorter one ranks first when scores are equal.
        assert_eq!(names(&finder, &tunnels), vec!["oficina", "oficina-madrid"]);
        finder.clear_query();
        for character in "xyz".chars() {
            finder.type_char(character);
        }
        assert!(finder.hits(&tunnels).is_empty());
    }

    #[test]
    fn delete_word_and_backspace_edit_the_query() {
        let mut finder = Finder::default();
        for character in "casa vpn".chars() {
            finder.type_char(character);
        }
        finder.delete_word();
        assert_eq!(finder.query, "casa ");
        finder.backspace();
        assert_eq!(finder.query, "casa");
        finder.open();
        assert!(finder.query.is_empty());
    }
}
