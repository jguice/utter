# Custom dictionary — design

Reference behavior: Wispr Flow's dictionary includes replacement rules that fix
consistent wrong output after transcription. utter should support that model
while preserving its local-only, fast, no-cloud architecture.

## Goal

Users can teach utter names, acronyms, product names, jargon, and repeated
misrecognitions. The feature must work the same way on Linux and macOS. The
management surface can differ by platform:

- Linux: CLI first.
- macOS: same CLI plus a native menu/window UI later.
- Runtime behavior: shared Rust code, same dictionary file format, same
  correction semantics.

Success for the first implementation: a user can add a correction for a repeated
speech-to-text mistake, dictate, and get corrected text pasted without any
network call or service restart.

## Non-negotiables

1. **Local only.** No cloud calls, no telemetry, no remote dictionary sync.
2. **Fast.** Dictionary processing should be negligible compared with Parakeet
   inference. Target: under 1 ms for post-processing with a few hundred entries;
   recognition-time boosting, if added through `transcribe-rs`, should stay under
   5% transcription latency overhead in normal dictionaries.
3. **Cross-platform semantics.** Linux and macOS may expose different UIs, but
   they load the same file and use the same correction engine.
4. **No restart for dictionary edits.** The daemon should pick up dictionary
   changes on the next dictation.
5. **Config separation.** The dictionary is user content, not daemon/service
   configuration. Store it separately from `config.toml`.

## Dictionary file

Path:

- Linux: `~/.config/utter/dictionary.toml`
- macOS: `~/Library/Application Support/utter/dictionary.toml`

Schema v1:

```toml
version = 1

[[entries]]
term = "API"
replace = ["a pie", "A.P.I."]

[[entries]]
term = "QMK"
replace = ["cue em kay"]

[[entries]]
term = "McKenzie"
replace = ["Mackenzie", "MacKenzie"]

[[entries]]
term = "Draft"
replace = ["Draught"]
```

Rules:

- `term` is the desired output and the future recognition-time vocabulary word.
- `replace` is required for CLI-created entries. Each string is a
  post-transcription trigger rewritten to `term`.
- Term-only entries may exist after manual file edits, but phase 1 pasted output
  changes only when a `replace` trigger matches.
- Empty or whitespace-only `term` / `replace` values are invalid.
- `term` is limited to 60 Unicode grapheme clusters for v1, matching Wispr's
  user-facing constraint and keeping future UI rows predictable. Replacement
  triggers use the same limit unless real-world examples show a need for longer
  phrases.
- Duplicate `term`s are merged by CLI/UI tools. Manual duplicate entries are
  accepted at load time but normalized on the next CLI/UI write.
- Matching is case-insensitive by default, but output always uses the exact
  casing and punctuation from `term`.
- Matching is Unicode-aware from day one. Users should be able to correct names
  with accents, non-ASCII punctuation, and emoji.
- Replacements are phrase-aware, not blind substring replacement. `draft` must
  not rewrite `redraft` or `drafted`; an emoji replacement should still work as
  an exact symbol match.
- Apply the longest matching trigger first. Ties use file order.
- Replacements are not recursive. One left-to-right pass prevents loops like
  `a -> b`, `b -> a`.

Examples:

- `utter dictionary add LUFS --replace luffs` rewrites `luffs` to `LUFS`.
- `utter dictionary add AcmeCloud --replace "acme cloud" --replace "acme clout"`
  rewrites those heard-as forms to `AcmeCloud`.
- If the desired output is spelled with punctuation, make that the term and add
  the heard-as form: `utter dictionary add "C++" --replace "see plus plus"`.

Possible future fields, not v1:

```toml
boost = true
boost_weight = 1.5
case_sensitive = false
notes = "Project-specific acronym"
```

Do not add these until the implementation needs them.

## CLI

Add a cross-platform `dictionary` subcommand:

```bash
utter dictionary add LUFS --replace luffs
utter dictionary add AcmeCloud --replace "acme cloud"
utter dictionary add API --replace "a pie"
utter dictionary remove API
utter dictionary list
utter dictionary path
```

Behavior:

- `add TERM` without at least one `--replace WRONG` exits with an error because
  term-only entries do not affect phase 1 corrections.
- `add TERM --replace WRONG` creates the entry if missing and appends `WRONG` if
  it is not already present.
- `remove TERM` removes the full entry.
- `list` prints a stable, human-readable table.
- `path` prints the dictionary file path for manual editing.
- Writes are atomic: write a temp file in the same directory, then rename.
- CLI writes normalize duplicate entries and duplicate replacements.

CSV import can be a follow-up:

```bash
utter dictionary import words.csv
```

Import format should mirror Wispr-style usage:

- One column: vocabulary terms.
- Two columns: `wrong,correct`, treated as `term = correct`, `replace += wrong`.

## Runtime flow

Current flow:

```text
audio -> Parakeet -> filler/stutter cleanup -> trailing space -> paste
```

Dictionary v1 flow:

```text
audio
  -> Parakeet
  -> filler/stutter cleanup if enabled
  -> dictionary post-processing
  -> trailing space
  -> paste
```

Loading:

- Add a shared `src/dictionary.rs` module.
- Daemon tracks dictionary path, last modified time, and last-good parsed
  dictionary.
- On each `stop`, before post-processing, reload if the file mtime changed.
- If the dictionary file is missing, use an empty dictionary.
- If parsing fails, log a warning and keep using the last-good dictionary. A bad
  manual edit should not break dictation.

Ordering:

1. Raw transcript from Parakeet.
2. Optional filler/stutter cleanup.
3. Dictionary replacements.
4. Trim and append utter's existing trailing space.
5. Emit text.

This order lets users write replacements against the cleaned text they actually
see, while preserving the existing filler cleanup behavior.

## Shared implementation shape

New pure module:

```text
src/dictionary.rs
  Dictionary
  DictionaryEntry
  DictionaryStore
  default_path()
  load()
  save_atomic()
  add_term()
  add_replacement()
  remove_term()
  apply_replacements()
```

The module must have no platform-specific behavior except path resolution via
`dirs::config_dir()`, matching `Config::default_path()`.

The macOS UI should call this same module rather than implementing its own file
editing logic. That keeps Linux CLI, macOS CLI, and macOS UI aligned.

## Recognition-time vocabulary boosting

Post-processing fixes consistent mistakes but does not improve recognition. To
match the vocabulary side of Wispr's feature, investigate a local-only Parakeet
hotword path in `transcribe-rs`.

Current `transcribe-rs` status:

- utter uses `transcribe-rs = { version = "0.3", features = ["onnx"] }`.
- Current resolved crate: `transcribe-rs 0.3.11`.
- `ParakeetParams` currently exposes language and timestamp granularity, but no
  hotword/vocabulary parameter.
- Parakeet decoding is a greedy loop over vocabulary logits; adding a local
  score bonus before argmax is plausible.

Prototype API, preferably upstream:

```rust
pub struct ParakeetParams {
    pub language: Option<String>,
    pub timestamp_granularity: Option<TimestampGranularity>,
    pub vocabulary: Vec<String>,
    pub vocabulary_boost: f32,
}
```

Prototype algorithm:

1. Compile dictionary `term`s into Parakeet token sequences using the model
   vocabulary.
2. Build a trie of hotword token sequences.
3. During greedy decode, track active hotword prefixes.
4. Before argmax, add a small bonus to logits for tokens that continue an active
   hotword prefix or start a hotword.
5. Keep default behavior byte-for-byte unchanged when `vocabulary` is empty.

Acceptance criteria for adopting boosting:

- No cloud dependency.
- No model retraining.
- No meaningful startup cost for typical dictionaries.
- No more than 5% latency overhead on short dictations with a few hundred terms.
- Quality improves for targeted terms without creating obvious false positives
  in ordinary dictation.

If upstream will not take it quickly, use a fork:

```toml
[patch.crates-io]
transcribe-rs = { git = "https://github.com/<owner>/transcribe-rs", rev = "..." }
```

Pin by commit SHA, not a moving branch, before release packaging.

## macOS UI

Do not block v1 on the UI. The first implementation should ship the shared file
format and CLI on both platforms.

Later macOS UI:

- Menu item: "Dictionary..."
- Window with search, add, edit, delete.
- Each entry shows `term` and replacement phrases.
- UI writes through `src/dictionary.rs` so behavior remains identical to CLI.
- No separate macOS-only dictionary format.

## Tests

Unit tests:

- Missing dictionary file loads as empty.
- TOML round-trip.
- Duplicate terms/replacements normalize on write.
- Replacement matching is case-insensitive.
- Replacement output honors the exact case and punctuation in `term`.
- Replacement matching respects word/phrase boundaries.
- Longest match wins.
- Replacements are non-recursive.
- Bad TOML keeps last-good dictionary in `DictionaryStore`.

CLI tests where practical:

- `dictionary add TERM` fails with a clear error.
- `dictionary add TERM --replace WRONG`
- `dictionary remove TERM`
- `dictionary list`

Manual smoke:

1. Add `LUFS --replace luffs`.
2. Dictate text that Parakeet produces as "luffs".
3. Confirm pasted output says `LUFS`.
4. Add `AcmeCloud --replace "acme cloud"`.
5. Dictate text that Parakeet produces as "acme cloud".
6. Confirm pasted output says `AcmeCloud`.
7. Edit dictionary while daemon is running.
8. Confirm next dictation picks up the edit without restart.

## Rollout

Phase 1:

- Add shared dictionary file/module.
- Add CLI management.
- Apply post-transcription replacements.
- Update `docs/CONFIGURATION.md` and README docs.

Phase 2:

- Prototype Parakeet vocabulary boosting in `transcribe-rs`.
- Decide upstream PR vs fork.
- Wire dictionary `term`s into `ParakeetParams`.

Phase 3:

- Add macOS dictionary UI over the shared module.
- Add CSV import if still wanted.

## Resolved product decisions

- Phase 1 correction behavior is explicit: `replace` triggers rewrite to the
  exact `term` the user provided. The CLI requires at least one `--replace`
  trigger because term-only entries do not change phase 1 pasted output.
- Terms and replacement triggers are limited to 60 Unicode grapheme clusters for
  v1.
- Matching is Unicode-aware from day one.
- Dictionary entries are global only for v1. App/context-specific dictionaries
  are out of scope.
