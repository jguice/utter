use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// User-facing configuration. Lives at `~/.config/utter/config.toml` by default.
/// Environment variables of the form `UTTER_*` override the file values;
/// CLI flags (where present) override env vars. Defaults are the
/// "recommended, minimally-surprising" values for a fresh install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// PTT key: named alias (`rightmeta`, `capslock`, `f13`, ...) or
    /// numeric evdev keycode as a string.
    pub key: String,
    /// Synthesize Shift+Insert after dictation to paste into the focused
    /// window. When false, only the primary selection is written and the
    /// user pastes manually.
    pub auto_paste: bool,
    /// Also write dictations to the regular clipboard alongside the
    /// primary selection. Default leaves the regular clipboard untouched.
    pub write_clipboard: bool,
    /// Drop filler words (uh, um, er, ah, erm, hmm) and collapse stuttered
    /// repetitions (`I I I think` → `I think`, `wh wh wh what` → `what`)
    /// before emitting text.
    pub filter_filler_words: bool,
    /// Fire a `notify-send` toast on recording start and errors.
    pub show_notifications: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            key: "rightmeta".to_string(),
            auto_paste: true,
            write_clipboard: false,
            filter_filler_words: true,
            show_notifications: false,
        }
    }
}

impl Config {
    /// Deserialize from a TOML string, falling back to defaults for missing
    /// fields. Returns an error if the TOML is syntactically invalid or
    /// contains unknown fields.
    pub fn from_toml(text: &str) -> Result<Self> {
        toml::from_str(text).context("parse config TOML")
    }

    /// Serialize to a TOML string with a header comment and per-field
    /// doc lines. Written once on first-run migration; users are expected
    /// to edit it after that.
    pub fn to_toml(&self) -> String {
        format!(
            "# utter configuration. Managed by `utter set-key` and edited by hand.\n\
             # Env vars (UTTER_KEY, UTTER_AUTO_PASTE, UTTER_WRITE_CLIPBOARD,\n\
             # UTTER_FILTER_FILLER_WORDS, UTTER_SHOW_NOTIFICATIONS) override any\n\
             # value set here.\n\
             \n\
             # PTT key: named alias (rightmeta, capslock, f13, ...) or numeric evdev\n\
             # keycode as a string.\n\
             key = {key:?}\n\
             \n\
             # Synthesize Shift+Insert to paste. false = user pastes manually.\n\
             auto_paste = {auto_paste}\n\
             \n\
             # Also write dictations to the regular clipboard (for clipboard-manager\n\
             # users). Default leaves the regular clipboard untouched.\n\
             write_clipboard = {write_clipboard}\n\
             \n\
             # Drop fillers (uh, um, er, ah, erm, hmm) and collapse stuttered\n\
             # repetitions (`I I I think` → `I think`).\n\
             filter_filler_words = {filter_filler_words}\n\
             \n\
             # Fire notify-send on recording start / errors.\n\
             show_notifications = {show_notifications}\n",
            key = self.key,
            auto_paste = self.auto_paste,
            write_clipboard = self.write_clipboard,
            filter_filler_words = self.filter_filler_words,
            show_notifications = self.show_notifications,
        )
    }

    /// Apply UTTER_* env vars on top of `self`. Unrecognized values for
    /// boolean fields log a warning and keep the existing value — better
    /// than silently treating `UTTER_AUTO_PASTE=yes` as `false`.
    pub fn with_env_overrides(mut self, env: &HashMap<String, String>) -> Self {
        if let Some(v) = env.get("UTTER_KEY") {
            self.key = v.clone();
        }
        if let Some(v) = env.get("UTTER_AUTO_PASTE") {
            self.auto_paste = parse_bool_env("UTTER_AUTO_PASTE", v).unwrap_or(self.auto_paste);
        }
        if let Some(v) = env.get("UTTER_WRITE_CLIPBOARD") {
            self.write_clipboard =
                parse_bool_env("UTTER_WRITE_CLIPBOARD", v).unwrap_or(self.write_clipboard);
        }
        if let Some(v) = env.get("UTTER_FILTER_FILLER_WORDS") {
            self.filter_filler_words =
                parse_bool_env("UTTER_FILTER_FILLER_WORDS", v).unwrap_or(self.filter_filler_words);
        }
        if let Some(v) = env.get("UTTER_SHOW_NOTIFICATIONS") {
            self.show_notifications = parse_bool_env("UTTER_SHOW_NOTIFICATIONS", v)
                .unwrap_or(self.show_notifications);
        }
        self
    }

    /// Canonical on-disk path for the config file. `dirs::config_dir()`
    /// resolves to `$XDG_CONFIG_HOME` (usually `~/.config`) on Linux and
    /// `~/Library/Application Support` on macOS — matching the rest of
    /// utter's path conventions.
    pub fn default_path() -> Result<PathBuf> {
        Ok(dirs::config_dir()
            .context("no XDG config dir")?
            .join("utter/config.toml"))
    }

    /// Write the current config to `path`, creating the parent dir if
    /// needed. Used by `utter set-key` when it updates the PTT key.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        std::fs::write(path, self.to_toml())
            .with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    /// Return a copy with the PTT key replaced. Pure — used by set-key.
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    /// Load from `path` if it exists, else synthesize from env vars
    /// (first-run migration) and write it. Env vars are then layered on
    /// top so precedence holds: env > file > default.
    pub fn load_or_migrate(path: &Path, env: &HashMap<String, String>) -> Result<Self> {
        let base = if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("read {}", path.display()))?;
            Self::from_toml(&text)?
        } else {
            let synthesized = Self::default().with_env_overrides(env);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            std::fs::write(path, synthesized.to_toml())
                .with_context(|| format!("write {}", path.display()))?;
            log::info!("wrote initial config to {}", path.display());
            synthesized
        };
        Ok(base.with_env_overrides(env))
    }
}

fn parse_bool_env(name: &str, value: &str) -> Option<bool> {
    match value {
        "1" | "true" | "TRUE" | "True" => Some(true),
        "0" | "false" | "FALSE" | "False" | "" => Some(false),
        other => {
            log::warn!("ignoring {name}={other:?} (expected 0 or 1)");
            None
        }
    }
}

/// Snapshot of the current process environment filtered to `UTTER_*` keys,
/// for passing to `load_or_migrate` / `with_env_overrides`. Isolated so
/// tests can supply their own map without `std::env::set_var` races.
pub fn utter_env_snapshot() -> HashMap<String, String> {
    std::env::vars()
        .filter(|(k, _)| k.starts_with("UTTER_"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn default_has_sensible_values() {
        let c = Config::default();
        assert_eq!(c.key, "rightmeta");
        assert!(c.auto_paste, "auto_paste on by default");
        assert!(!c.write_clipboard, "write_clipboard off by default — don't pollute");
        assert!(c.filter_filler_words, "filter_filler_words on by default");
        assert!(!c.show_notifications, "show_notifications off by default");
    }

    #[test]
    fn toml_roundtrips_through_from_and_to() {
        let original = Config {
            key: "capslock".to_string(),
            auto_paste: false,
            write_clipboard: true,
            filter_filler_words: false,
            show_notifications: true,
        };
        let text = original.to_toml();
        let parsed = Config::from_toml(&text).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn from_toml_fills_in_missing_fields() {
        let text = "key = \"f13\"\n";
        let c = Config::from_toml(text).unwrap();
        assert_eq!(c.key, "f13");
        assert!(c.auto_paste, "other fields default");
        assert!(!c.write_clipboard);
    }

    #[test]
    fn from_toml_rejects_unknown_fields() {
        let text = "key = \"rightmeta\"\nunknown_knob = 42\n";
        let err = Config::from_toml(text).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("unknown") || msg.contains("unknown_knob"), "err was: {msg}");
    }

    #[test]
    fn from_toml_rejects_syntax_errors() {
        assert!(Config::from_toml("key =").is_err());
        assert!(Config::from_toml("not = valid = toml").is_err());
    }

    #[test]
    fn env_overrides_every_field() {
        let base = Config::default();
        let e = env(&[
            ("UTTER_KEY", "f13"),
            ("UTTER_AUTO_PASTE", "0"),
            ("UTTER_WRITE_CLIPBOARD", "1"),
            ("UTTER_FILTER_FILLER_WORDS", "0"),
            ("UTTER_SHOW_NOTIFICATIONS", "1"),
        ]);
        let c = base.with_env_overrides(&e);
        assert_eq!(c.key, "f13");
        assert!(!c.auto_paste);
        assert!(c.write_clipboard);
        assert!(!c.filter_filler_words);
        assert!(c.show_notifications);
    }

    #[test]
    fn env_without_utter_vars_is_noop() {
        let base = Config {
            key: "capslock".to_string(),
            auto_paste: false,
            write_clipboard: true,
            filter_filler_words: false,
            show_notifications: true,
        };
        let c = base.clone().with_env_overrides(&env(&[("PATH", "/usr/bin")]));
        assert_eq!(c, base);
    }

    #[test]
    fn env_accepts_true_false_spellings() {
        let c = Config::default().with_env_overrides(&env(&[
            ("UTTER_AUTO_PASTE", "false"),
            ("UTTER_WRITE_CLIPBOARD", "true"),
        ]));
        assert!(!c.auto_paste);
        assert!(c.write_clipboard);
    }

    #[test]
    fn env_bogus_bool_preserves_existing_value() {
        // Unrecognized strings log a warning and leave the field alone.
        let c = Config {
            auto_paste: true,
            ..Config::default()
        };
        let with_bogus = c
            .clone()
            .with_env_overrides(&env(&[("UTTER_AUTO_PASTE", "yes")]));
        assert!(with_bogus.auto_paste, "bogus value didn't flip the field");
    }

    #[test]
    fn load_or_migrate_creates_file_from_env() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("utter/config.toml");
        assert!(!path.exists());

        let e = env(&[
            ("UTTER_AUTO_PASTE", "0"),
            ("UTTER_KEY", "f13"),
        ]);
        let c = Config::load_or_migrate(&path, &e).unwrap();

        assert!(path.exists(), "config file written");
        assert_eq!(c.key, "f13");
        assert!(!c.auto_paste);

        // File contents: persisted values. Re-reading gives the same config.
        let e_empty = HashMap::new();
        let c2 = Config::load_or_migrate(&path, &e_empty).unwrap();
        assert_eq!(c2.key, "f13");
        assert!(!c2.auto_paste);
    }

    #[test]
    fn load_or_migrate_reads_existing_file_and_env_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "key = \"capslock\"\nauto_paste = true\n").unwrap();

        // Env var overrides the file.
        let e = env(&[("UTTER_AUTO_PASTE", "0")]);
        let c = Config::load_or_migrate(&path, &e).unwrap();
        assert_eq!(c.key, "capslock", "from file");
        assert!(!c.auto_paste, "from env, overriding file");
    }

    #[test]
    fn with_key_replaces_only_key_field() {
        let c = Config {
            auto_paste: false,
            write_clipboard: true,
            filter_filler_words: false,
            show_notifications: true,
            key: "rightmeta".to_string(),
        };
        let updated = c.clone().with_key("f13");
        assert_eq!(updated.key, "f13");
        // Other fields preserved.
        assert_eq!(updated.auto_paste, c.auto_paste);
        assert_eq!(updated.write_clipboard, c.write_clipboard);
        assert_eq!(updated.filter_filler_words, c.filter_filler_words);
        assert_eq!(updated.show_notifications, c.show_notifications);
    }

    #[test]
    fn save_to_writes_toml_readable_by_from_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/dir/config.toml");

        let original = Config::default().with_key("capslock");
        original.save_to(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let parsed = Config::from_toml(&text).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn load_or_migrate_no_env_and_no_file_gives_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("utter/config.toml");
        let c = Config::load_or_migrate(&path, &HashMap::new()).unwrap();
        assert_eq!(c, Config::default());
        assert!(path.exists(), "file written with defaults");
    }
}
