use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use unicode_segmentation::UnicodeSegmentation;

const CURRENT_VERSION: u32 = 1;
const MAX_GRAPHEMES: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Dictionary {
    pub version: u32,
    pub entries: Vec<DictionaryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DictionaryEntry {
    pub term: String,
    #[serde(default)]
    pub replace: Vec<String>,
}

pub struct DictionaryStore {
    path: PathBuf,
    last_modified: Option<SystemTime>,
    dictionary: Dictionary,
}

impl Default for Dictionary {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            entries: Vec::new(),
        }
    }
}

impl Dictionary {
    pub fn default_path() -> Result<PathBuf> {
        Ok(dirs::config_dir()
            .context("no XDG config dir")?
            .join("utter/dictionary.toml"))
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::from_toml(&text)
    }

    pub fn from_toml(text: &str) -> Result<Self> {
        let dictionary: Self = toml::from_str(text).context("parse dictionary TOML")?;
        dictionary.validate()?;
        Ok(dictionary)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("dictionary serialization should not fail")
    }

    pub fn save_atomic(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }

        let tmp = path.with_extension(format!("toml.tmp.{}", std::process::id()));
        std::fs::write(&tmp, self.to_toml()).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("rename {} to {}", tmp.display(), path.display()))?;
        Ok(())
    }

    pub fn add_term(&mut self, term: &str) -> Result<bool> {
        validate_value("term", term)?;
        if self.entries.iter().any(|entry| entry.term == term) {
            return Ok(false);
        }
        self.entries.push(DictionaryEntry {
            term: term.to_string(),
            replace: Vec::new(),
        });
        Ok(true)
    }

    pub fn add_replacement(&mut self, term: &str, replacement: &str) -> Result<bool> {
        validate_value("term", term)?;
        validate_value("replacement", replacement)?;

        let entry = if let Some(entry) = self.entries.iter_mut().find(|entry| entry.term == term) {
            entry
        } else {
            self.entries.push(DictionaryEntry {
                term: term.to_string(),
                replace: Vec::new(),
            });
            self.entries
                .last_mut()
                .expect("entry was just pushed and must exist")
        };

        if entry
            .replace
            .iter()
            .any(|existing| existing.to_lowercase() == replacement.to_lowercase())
        {
            return Ok(false);
        }
        entry.replace.push(replacement.to_string());
        Ok(true)
    }

    pub fn remove_term(&mut self, term: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.term != term);
        self.entries.len() != before
    }

    pub fn normalize(&mut self) {
        let mut normalized: Vec<DictionaryEntry> = Vec::with_capacity(self.entries.len());
        for entry in std::mem::take(&mut self.entries) {
            if let Some(existing) = normalized.iter_mut().find(|e| e.term == entry.term) {
                for replacement in entry.replace {
                    if !existing
                        .replace
                        .iter()
                        .any(|r| r.to_lowercase() == replacement.to_lowercase())
                    {
                        existing.replace.push(replacement);
                    }
                }
            } else {
                let mut deduped = DictionaryEntry {
                    term: entry.term,
                    replace: Vec::new(),
                };
                for replacement in entry.replace {
                    if !deduped
                        .replace
                        .iter()
                        .any(|r| r.to_lowercase() == replacement.to_lowercase())
                    {
                        deduped.replace.push(replacement);
                    }
                }
                normalized.push(deduped);
            }
        }
        self.entries = normalized;
    }

    pub fn apply_replacements(&self, text: &str) -> String {
        let rules = self.rules();
        if rules.is_empty() || text.is_empty() {
            return text.to_string();
        }

        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            if let Some((end, term)) = rules
                .iter()
                .find_map(|rule| rule.matches_at(text, i).map(|end| (end, rule.term)))
            {
                out.push_str(term);
                i = end;
                continue;
            }

            let ch = text[i..]
                .chars()
                .next()
                .expect("i always points at a char boundary");
            out.push(ch);
            i += ch.len_utf8();
        }
        out
    }

    fn validate(&self) -> Result<()> {
        if self.version != CURRENT_VERSION {
            return Err(anyhow!(
                "unsupported dictionary version {} (expected {CURRENT_VERSION})",
                self.version
            ));
        }
        for entry in &self.entries {
            validate_value("term", &entry.term)?;
            for replacement in &entry.replace {
                validate_value("replacement", replacement)?;
            }
        }
        Ok(())
    }

    fn rules(&self) -> Vec<Rule<'_>> {
        let mut rules = Vec::new();
        for entry in &self.entries {
            for replacement in &entry.replace {
                rules.push(Rule::new(replacement, &entry.term));
            }
        }
        rules.sort_by_key(|rule| std::cmp::Reverse(rule.graphemes));
        rules
    }
}

impl DictionaryStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            last_modified: None,
            dictionary: Dictionary::default(),
        }
    }

    pub fn reload_if_changed(&mut self) {
        let modified = match std::fs::metadata(&self.path).and_then(|m| m.modified()) {
            Ok(modified) => Some(modified),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                log::warn!("dictionary metadata {}: {e:#}", self.path.display());
                return;
            }
        };

        if modified == self.last_modified {
            return;
        }

        match Dictionary::load_from(&self.path) {
            Ok(dictionary) => {
                self.dictionary = dictionary;
                self.last_modified = modified;
                log::info!(
                    "dictionary loaded from {} ({} entr{})",
                    self.path.display(),
                    self.dictionary.entries.len(),
                    if self.dictionary.entries.len() == 1 {
                        "y"
                    } else {
                        "ies"
                    },
                );
            }
            Err(e) => {
                log::warn!(
                    "dictionary parse failed for {}; keeping last-good dictionary: {e:#}",
                    self.path.display()
                );
            }
        }
    }

    pub fn dictionary(&self) -> &Dictionary {
        &self.dictionary
    }
}

struct Rule<'a> {
    trigger: &'a str,
    trigger_lower: String,
    term: &'a str,
    chars: usize,
    graphemes: usize,
}

impl<'a> Rule<'a> {
    fn new(trigger: &'a str, term: &'a str) -> Self {
        Self {
            trigger,
            trigger_lower: trigger.to_lowercase(),
            term,
            chars: trigger.chars().count(),
            graphemes: trigger.graphemes(true).count(),
        }
    }

    fn matches_at(&self, text: &str, start: usize) -> Option<usize> {
        if !text.is_char_boundary(start) {
            return None;
        }
        let end = end_after_chars(text, start, self.chars)?;
        let candidate = &text[start..end];
        if candidate.to_lowercase() != self.trigger_lower {
            return None;
        }
        if !has_word_boundary_before(text, start, self.trigger) {
            return None;
        }
        if !has_word_boundary_after(text, end, self.trigger) {
            return None;
        }
        Some(end)
    }
}

fn validate_value(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(anyhow!("{name} cannot be empty"));
    }
    let count = value.graphemes(true).count();
    if count > MAX_GRAPHEMES {
        return Err(anyhow!(
            "{name} is {count} characters; max is {MAX_GRAPHEMES}"
        ));
    }
    Ok(())
}

fn end_after_chars(text: &str, start: usize, count: usize) -> Option<usize> {
    let mut end = start;
    for _ in 0..count {
        let ch = text.get(end..)?.chars().next()?;
        end += ch.len_utf8();
    }
    Some(end)
}

fn has_word_boundary_before(text: &str, start: usize, trigger: &str) -> bool {
    let Some(first) = trigger.chars().next() else {
        return false;
    };
    if !is_word_char(first) {
        return true;
    }
    previous_char(text, start).is_none_or(|ch| !is_word_char(ch))
}

fn has_word_boundary_after(text: &str, end: usize, trigger: &str) -> bool {
    let Some(last) = trigger.chars().next_back() else {
        return false;
    };
    if !is_word_char(last) {
        return true;
    }
    text.get(end..)
        .and_then(|rest| rest.chars().next())
        .is_none_or(|ch| !is_word_char(ch))
}

fn previous_char(text: &str, start: usize) -> Option<char> {
    text.get(..start)?.chars().next_back()
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(entries: Vec<DictionaryEntry>) -> Dictionary {
        Dictionary {
            version: CURRENT_VERSION,
            entries,
        }
    }

    #[test]
    fn missing_file_loads_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let dictionary = Dictionary::load_from(&tmp.path().join("dictionary.toml")).unwrap();
        assert_eq!(dictionary, Dictionary::default());
    }

    #[test]
    fn toml_roundtrips() {
        let original = dict(vec![DictionaryEntry {
            term: "AcmeCloud".to_string(),
            replace: vec!["acme cloud".to_string(), "acme clout".to_string()],
        }]);
        let parsed = Dictionary::from_toml(&original.to_toml()).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn add_and_normalize_dedupes_entries_and_replacements() {
        let mut dictionary = dict(vec![
            DictionaryEntry {
                term: "LUFS".to_string(),
                replace: vec!["luffs".to_string()],
            },
            DictionaryEntry {
                term: "LUFS".to_string(),
                replace: vec!["Luffs".to_string(), "lufz".to_string()],
            },
        ]);
        dictionary.normalize();
        assert_eq!(dictionary.entries.len(), 1);
        assert_eq!(dictionary.entries[0].replace, vec!["luffs", "lufz"]);

        assert!(!dictionary.add_replacement("LUFS", "LUFZ").unwrap());
        assert!(dictionary.add_replacement("LUFS", "loofs").unwrap());
    }

    #[test]
    fn replacement_matching_is_case_insensitive_but_output_is_exact_term() {
        let dictionary = dict(vec![DictionaryEntry {
            term: "AcmeCloud".to_string(),
            replace: vec!["acme cloud".to_string()],
        }]);
        assert_eq!(
            dictionary.apply_replacements("I use ACME CLOUD."),
            "I use AcmeCloud."
        );
    }

    #[test]
    fn replacement_matching_respects_word_boundaries() {
        let dictionary = dict(vec![DictionaryEntry {
            term: "Draft".to_string(),
            replace: vec!["draught".to_string()],
        }]);
        assert_eq!(
            dictionary.apply_replacements("draught redraught draughted"),
            "Draft redraught draughted"
        );
    }

    #[test]
    fn longest_match_wins() {
        let dictionary = dict(vec![
            DictionaryEntry {
                term: "AI".to_string(),
                replace: vec!["a i".to_string()],
            },
            DictionaryEntry {
                term: "OpenAI".to_string(),
                replace: vec!["open a i".to_string()],
            },
        ]);
        assert_eq!(
            dictionary.apply_replacements("open a i shipped it"),
            "OpenAI shipped it"
        );
    }

    #[test]
    fn replacements_are_not_recursive() {
        let dictionary = dict(vec![
            DictionaryEntry {
                term: "b".to_string(),
                replace: vec!["a".to_string()],
            },
            DictionaryEntry {
                term: "c".to_string(),
                replace: vec!["b".to_string()],
            },
        ]);
        assert_eq!(dictionary.apply_replacements("a b"), "b c");
    }

    #[test]
    fn unicode_and_emoji_replacements_work() {
        let dictionary = dict(vec![
            DictionaryEntry {
                term: "Beyonce".to_string(),
                replace: vec!["Beyoncé".to_string()],
            },
            DictionaryEntry {
                term: "✅".to_string(),
                replace: vec![":check:".to_string()],
            },
        ]);
        assert_eq!(
            dictionary.apply_replacements("Beyoncé :check:"),
            "Beyonce ✅"
        );
    }

    #[test]
    fn validates_sixty_grapheme_limit() {
        let sixty = "🙂".repeat(60);
        let sixty_one = "🙂".repeat(61);
        assert!(validate_value("term", &sixty).is_ok());
        assert!(validate_value("term", &sixty_one).is_err());
    }

    #[test]
    fn store_keeps_last_good_dictionary_when_reload_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dictionary.toml");
        let original = dict(vec![DictionaryEntry {
            term: "LUFS".to_string(),
            replace: vec!["luffs".to_string()],
        }]);
        original.save_atomic(&path).unwrap();

        let mut store = DictionaryStore::new(path.clone());
        store.reload_if_changed();
        assert_eq!(store.dictionary().apply_replacements("luffs"), "LUFS");

        std::fs::write(&path, "not = valid = toml").unwrap();
        store.last_modified = None;
        store.reload_if_changed();
        assert_eq!(store.dictionary().apply_replacements("luffs"), "LUFS");
    }
}
