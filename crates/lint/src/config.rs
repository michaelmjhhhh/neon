//! Which rules run, merged from layers of configuration.
//!
//! From lowest to highest precedence:
//!
//! 1. The built-in defaults: spacing rules on, the rest off.
//! 2. A global `lint.toml` in Neon's settings folder.
//! 3. Each `.neonlint.toml` from the open folder down to the note's own
//!    folder. A file with `root = true` ends the search upward.
//! 4. Within each file, `[[overrides]]` whose `files` match the note, in
//!    order.
//!
//! A layer changes only what it names: `rules.trailing-spaces = false` in a
//! subfolder turns that rule off there and leaves the others as the parent
//! set them.
//!
//! ```toml
//! # Show issues in the editor and fix on save for notes under this folder,
//! # whatever Neon's settings say.
//! show-issues = true
//! fix-on-save = true
//! ignore = ["Templates", "*.excalidraw.md"]
//!
//! [rules]
//! consecutive-blank-lines = { max = 2 }
//! cjk-spacing = true
//! proper-ellipsis = { level = "info", fix-on-save = true }
//! heading-increment = "off"
//!
//! [[overrides]]
//! files = ["Journal/**"]
//! rules = { single-h1 = true }
//!
//! [[custom]]
//! name = "full-width-colon"
//! pattern = '(\p{Han}):'
//! replacement = '$1：'
//! message = "Use a full-width colon after Chinese"
//! ```

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use regex::{Regex, RegexBuilder};
use serde::Deserialize;

use crate::context::Context;
use crate::glob::Glob;
use crate::rules::{Options, RuleInfo, rules};
use crate::{Edit, Finding, RuleName};

/// The name of a folder's configuration file.
pub const CONFIG_FILE_NAME: &str = ".neonlint.toml";

/// How a rule's findings show, or that the rule does not run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Level {
    Off,
    #[default]
    Warning,
    Info,
}

impl Level {
    /// `"off"`, `"warn"`, `"info"`, or `true` and `false` for the rule's
    /// usual level and off.
    fn parse(value: &toml::Value, on: Level) -> Result<Level, String> {
        match value {
            toml::Value::Boolean(true) => Ok(on),
            toml::Value::Boolean(false) => Ok(Level::Off),
            toml::Value::String(name) => match name.as_str() {
                "off" => Ok(Level::Off),
                "warn" | "warning" => Ok(Level::Warning),
                "info" => Ok(Level::Info),
                _ => Err(format!("unknown level `{name}`; use off, warn or info")),
            },
            _ => Err("a level is off, warn, info, true or false".into()),
        }
    }
}

/// The level `true` turns a rule on at.
fn on_level(info: &RuleInfo) -> Level {
    match info.default_level {
        Level::Off => Level::Warning,
        level => level,
    }
}

/// A rule set to run, with its options read.
pub(crate) struct ActiveRule {
    pub info: &'static RuleInfo,
    pub level: Level,
    pub fix_on_save: bool,
    pub options: Options,
}

/// The rules that apply to one file.
pub struct Config {
    rules: Vec<ActiveRule>,
    custom: Vec<Arc<Custom>>,
    ignored: bool,
    show_issues: Option<bool>,
    fix_on_save: Option<bool>,
    errors: Vec<String>,
}

impl Default for Config {
    /// The built-in defaults alone.
    fn default() -> Self {
        Merged::default().build()
    }
}

impl Config {
    /// The defaults with one configuration file's text on top, read as if
    /// for a file outside any folder its patterns name.
    pub fn from_toml(text: &str) -> Self {
        let mut merged = Merged::default();
        match Layer::parse(text, PathBuf::new()) {
            Ok(layer) => merged.apply(&layer, None),
            Err(error) => merged.errors.push(error),
        }
        merged.build()
    }

    /// Whether the file is ignored and nothing runs on it.
    pub fn is_ignored(&self) -> bool {
        self.ignored
    }

    /// Problems in the configuration files, each naming its file. Rules
    /// with bad options run with their defaults; files that cannot be read
    /// are skipped.
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// Whether the files set the editor to show issues, overriding the
    /// app's setting.
    pub fn show_issues(&self) -> Option<bool> {
        self.show_issues
    }

    /// Whether the files set saving to fix issues, overriding the app's
    /// setting.
    pub fn fix_on_save(&self) -> Option<bool> {
        self.fix_on_save
    }

    /// Whether any rule runs.
    pub fn is_empty(&self) -> bool {
        self.ignored || (self.rules.is_empty() && self.custom.is_empty())
    }

    /// Whether any rule would fix something on save.
    pub fn fixes_on_save(&self) -> bool {
        !self.ignored
            && (self
                .rules
                .iter()
                .any(|rule| rule.fix_on_save && rule.info.fixable)
                || self
                    .custom
                    .iter()
                    .any(|custom| custom.fix_on_save && custom.replacement.is_some()))
    }

    pub(crate) fn active_rules(&self) -> &[ActiveRule] {
        &self.rules
    }

    pub(crate) fn custom_rules(&self) -> &[Arc<Custom>] {
        &self.custom
    }

    /// Only the rule `name`, turned on with the options in `options`.
    #[cfg(test)]
    pub(crate) fn only(name: &str, options: &str) -> Self {
        let options: toml::Table = toml::from_str(options).unwrap();
        let mut merged = Merged::default();
        for info in rules() {
            merged.rules.entry(info.name).or_default().level = Some(Level::Off);
        }
        let info = rules().iter().find(|info| info.name == name).unwrap();
        let state = merged.rules.entry(info.name).or_default();
        state.level = Some(on_level(info));
        state.options = options;
        let config = merged.build();
        assert!(config.errors.is_empty(), "{:?}", config.errors);
        config
    }
}

/// A pattern from a `[[custom]]` table.
pub(crate) struct Custom {
    pub name: Arc<str>,
    regex: Regex,
    replacement: Option<String>,
    /// Whether `replacement` refers to groups, so matches need captures.
    expands: bool,
    message: Cow<'static, str>,
    level: Level,
    fix_on_save: bool,
}

impl Custom {
    pub fn check(&self, cx: &Context<'_>, out: &mut Vec<Finding>) {
        let source = cx.source;
        let names = [&*self.name];
        let mut report = |range: std::ops::Range<usize>, text: Option<String>| {
            if cx.is_protected(&range)
                || (!cx.suppressions.is_empty() && cx.suppressions.covers(range.start, &names))
            {
                return;
            }
            // A replacement that changes nothing is not a finding: fixing
            // it would never settle.
            if text.as_deref() == Some(&source[range.clone()]) {
                return;
            }
            out.push(Finding {
                rule: RuleName::Custom(self.name.clone()),
                range: range.clone(),
                message: self.message.clone(),
                level: self.level,
                fix: text
                    .map(|text| vec![Edit { range, text }])
                    .unwrap_or_default(),
                fix_on_save: self.fix_on_save,
                transient: false,
            });
        };
        if self.expands {
            let replacement = self.replacement.as_deref().unwrap_or_default();
            for captures in self.regex.captures_iter(source) {
                let found = captures.get(0).unwrap();
                if found.is_empty() {
                    continue;
                }
                let mut text = String::new();
                captures.expand(replacement, &mut text);
                report(found.range(), Some(text));
            }
        } else {
            for found in self.regex.find_iter(source) {
                if !found.is_empty() {
                    report(found.range(), self.replacement.clone());
                }
            }
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct FileConfig {
    root: bool,
    show_issues: Option<bool>,
    fix_on_save: Option<bool>,
    ignore: Vec<String>,
    rules: toml::Table,
    overrides: Vec<OverrideConfig>,
    custom: Vec<CustomConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct OverrideConfig {
    files: Vec<String>,
    #[serde(default)]
    rules: toml::Table,
    #[serde(default)]
    ignore: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CustomConfig {
    name: String,
    pattern: String,
    replacement: Option<String>,
    message: Option<String>,
    level: Option<toml::Value>,
    fix_on_save: Option<bool>,
    #[serde(default)]
    case_insensitive: bool,
}

/// One configuration file, read and compiled.
pub(crate) struct Layer {
    dir: PathBuf,
    root: bool,
    show_issues: Option<bool>,
    fix_on_save: Option<bool>,
    ignore: Vec<Glob>,
    rules: toml::Table,
    overrides: Vec<Override>,
    /// Patterns, including those turned off, which turn off a pattern of
    /// the same name from a layer below.
    custom: Vec<(Arc<str>, Option<Arc<Custom>>)>,
    /// Problems that leave the rest of the file usable.
    errors: Vec<String>,
}

struct Override {
    files: Vec<Glob>,
    rules: toml::Table,
    ignore: bool,
}

impl Layer {
    fn parse(text: &str, dir: PathBuf) -> Result<Self, String> {
        let file: FileConfig = toml::from_str(text).map_err(|error| error.to_string())?;
        let mut errors = Vec::new();
        let mut globs = |patterns: &[String]| -> Vec<Glob> {
            patterns
                .iter()
                .filter_map(|pattern| Glob::new(pattern).map_err(|error| errors.push(error)).ok())
                .collect()
        };
        let ignore = globs(&file.ignore);
        let overrides = file
            .overrides
            .into_iter()
            .map(|config| Override {
                files: globs(&config.files),
                rules: config.rules,
                ignore: config.ignore,
            })
            .collect();
        let custom = file
            .custom
            .into_iter()
            .filter_map(|config| {
                let name: Arc<str> = config.name.trim().into();
                match compile(config) {
                    Ok(custom) => Some((name, custom.map(Arc::new))),
                    Err(error) => {
                        errors.push(format!("custom rule `{name}`: {error}"));
                        None
                    }
                }
            })
            .collect();
        Ok(Self {
            dir,
            root: file.root,
            show_issues: file.show_issues,
            fix_on_save: file.fix_on_save,
            ignore,
            rules: file.rules,
            overrides,
            custom,
            errors,
        })
    }
}

/// A `[[custom]]` table compiled, or `None` for one turned off.
fn compile(config: CustomConfig) -> Result<Option<Custom>, String> {
    if config.name.trim().is_empty() {
        return Err("needs a name".into());
    }
    let level = match &config.level {
        Some(level) => Level::parse(level, Level::Warning)?,
        None => Level::Warning,
    };
    if level == Level::Off {
        return Ok(None);
    }
    let regex = RegexBuilder::new(&config.pattern)
        .case_insensitive(config.case_insensitive)
        .multi_line(true)
        // Keeps a pathological pattern from taking memory without bound.
        .size_limit(1 << 20)
        .build()
        .map_err(|error| error.to_string())?;
    let expands = config
        .replacement
        .as_deref()
        .is_some_and(|replacement| replacement.contains('$'));
    let message = match (config.message, &config.replacement) {
        (Some(message), _) => message.into(),
        (None, Some(replacement)) => format!("Replace with `{replacement}`").into(),
        (None, None) => format!("Matches `{}`", config.pattern).into(),
    };
    Ok(Some(Custom {
        name: config.name.trim().into(),
        regex,
        expands,
        message,
        level,
        // Custom patterns usually change words, so wait for an explicit fix.
        fix_on_save: config.fix_on_save.unwrap_or(false),
        replacement: config.replacement,
    }))
}

#[derive(Default)]
struct RuleState {
    level: Option<Level>,
    fix_on_save: Option<bool>,
    options: toml::Table,
}

/// Layers merged so far.
#[derive(Default)]
struct Merged {
    rules: HashMap<&'static str, RuleState>,
    custom: Vec<(Arc<str>, Option<Arc<Custom>>)>,
    ignored: bool,
    show_issues: Option<bool>,
    fix_on_save: Option<bool>,
    errors: Vec<String>,
}

impl Merged {
    /// Merge `layer` for the file at `file`, a path relative to the layer's
    /// folder with `/` between segments; `None` for a file with no path.
    fn apply(&mut self, layer: &Layer, file: Option<&str>) {
        let origin = layer.dir.join(CONFIG_FILE_NAME);
        for error in &layer.errors {
            self.errors.push(format!("{}: {error}", origin.display()));
        }
        if let Some(file) = file
            && layer.ignore.iter().any(|glob| glob.matches(file))
        {
            self.ignored = true;
        }
        self.show_issues = layer.show_issues.or(self.show_issues);
        self.fix_on_save = layer.fix_on_save.or(self.fix_on_save);
        self.apply_rules(&layer.rules, &origin);
        for (name, custom) in &layer.custom {
            self.custom.retain(|(existing, _)| existing != name);
            self.custom.push((name.clone(), custom.clone()));
        }
        let Some(file) = file else {
            return;
        };
        for config in &layer.overrides {
            if config.files.iter().any(|glob| glob.matches(file)) {
                self.ignored |= config.ignore;
                self.apply_rules(&config.rules, &origin);
            }
        }
    }

    fn apply_rules(&mut self, table: &toml::Table, origin: &Path) {
        for (key, value) in table {
            let Some(info) = rules().iter().find(|info| {
                info.name.eq_ignore_ascii_case(key)
                    || info
                        .alias
                        .is_some_and(|alias| alias.eq_ignore_ascii_case(key))
            }) else {
                self.errors
                    .push(format!("{}: unknown rule `{key}`", origin.display()));
                continue;
            };
            let state = self.rules.entry(info.name).or_default();
            let result = match value {
                toml::Value::Table(table) => table.iter().try_for_each(|(key, value)| {
                    match key.as_str() {
                        "level" => state.level = Some(Level::parse(value, on_level(info))?),
                        "fix-on-save" => {
                            state.fix_on_save = Some(
                                value
                                    .as_bool()
                                    .ok_or("`fix-on-save` must be true or false")?,
                            )
                        }
                        _ => {
                            state.options.insert(key.clone(), value.clone());
                        }
                    }
                    Ok::<_, String>(())
                }),
                value => Level::parse(value, on_level(info)).map(|level| state.level = Some(level)),
            };
            if let Err(error) = result {
                self.errors
                    .push(format!("{}: {}: {error}", origin.display(), info.name));
            }
            // A table with options alone turns the rule on, so
            // `consecutive-blank-lines = { max = 2 }` means what it says.
            if matches!(value, toml::Value::Table(_)) && state.level.is_none() {
                state.level = Some(on_level(info));
            }
        }
    }

    fn build(mut self) -> Config {
        let empty = toml::Table::new();
        let mut active = Vec::new();
        for info in rules() {
            let state = self.rules.get(info.name);
            let level = state
                .and_then(|state| state.level)
                .unwrap_or(info.default_level);
            if level == Level::Off {
                continue;
            }
            let table = state.map_or(&empty, |state| &state.options);
            let options = (info.options)(table).unwrap_or_else(|error| {
                self.errors.push(format!("{}: {error}", info.name));
                (info.options)(&empty).expect("a rule's defaults are valid")
            });
            active.push(ActiveRule {
                info,
                level,
                fix_on_save: info.fixable
                    && state
                        .and_then(|state| state.fix_on_save)
                        .unwrap_or(info.fix_on_save),
                options,
            });
        }
        Config {
            rules: active,
            custom: self
                .custom
                .into_iter()
                .filter_map(|(_, custom)| custom)
                .collect(),
            ignored: self.ignored,
            show_issues: self.show_issues,
            fix_on_save: self.fix_on_save,
            errors: self.errors,
        }
    }
}

/// Finds and caches the configuration files that apply to a note.
///
/// Files are read again only when their modification time or size
/// changes, so resolving a note's configuration costs a few `stat` calls.
/// Resolving reads the disk: call it off the main thread.
pub struct Resolver {
    global: Option<PathBuf>,
    files: HashMap<PathBuf, Cached>,
    /// The last configuration built for each note, and the layers it was
    /// built from.
    built: HashMap<PathBuf, (Vec<usize>, Arc<Config>)>,
}

struct Cached {
    stamp: (SystemTime, u64),
    layer: Result<Arc<Layer>, String>,
}

/// More notes than this and the cache of built configurations starts over.
const BUILT_LIMIT: usize = 256;

impl Resolver {
    /// `global` is the path of the global configuration file, read if it
    /// exists.
    pub fn new(global: Option<PathBuf>) -> Self {
        Self {
            global,
            files: HashMap::new(),
            built: HashMap::new(),
        }
    }

    /// The configuration for the note at `file`, in the open `folder`. A note
    /// without a path takes the folder's configuration.
    ///
    /// Without a folder, or for a note outside it, the search goes up to
    /// the nearest folder holding `.git`, or else to the top of the disk.
    pub fn resolve(&mut self, file: Option<&Path>, folder: Option<&Path>) -> Arc<Config> {
        let start = match file {
            Some(file) => file.parent(),
            None => folder,
        };
        let mut layers: Vec<Arc<Layer>> = Vec::new();
        let mut errors = Vec::new();
        let in_folder = |dir: &Path| folder.is_some_and(|folder| dir.starts_with(folder));
        let bounded = start.is_some_and(in_folder);
        let mut dir = start;
        while let Some(current) = dir {
            if let Some(layer) = self.load(&current.join(CONFIG_FILE_NAME), current, &mut errors) {
                let root = layer.root;
                layers.push(layer);
                if root {
                    break;
                }
            }
            let at_top = if bounded {
                folder == Some(current)
            } else {
                current.join(".git").exists()
            };
            if at_top {
                break;
            }
            dir = current.parent();
        }
        let global = self.global.clone().and_then(|path| {
            // The global file's patterns are relative to the open folder.
            let base = folder.map(Path::to_path_buf).unwrap_or_default();
            self.load(&path, &base, &mut errors)
        });
        let ordered: Vec<Arc<Layer>> = global.into_iter().chain(layers.into_iter().rev()).collect();

        let key = file.map(Path::to_path_buf).unwrap_or_default();
        let identity: Vec<usize> = ordered
            .iter()
            .map(|layer| Arc::as_ptr(layer) as usize)
            .collect();
        if errors.is_empty()
            && let Some((built_from, config)) = self.built.get(&key)
            && *built_from == identity
        {
            return config.clone();
        }

        let mut merged = Merged {
            errors,
            ..Merged::default()
        };
        for layer in &ordered {
            let relative = file.and_then(|file| relative_path(file, &layer.dir));
            merged.apply(layer, relative.as_deref());
        }
        let config = Arc::new(merged.build());
        if self.built.len() >= BUILT_LIMIT {
            self.built.clear();
        }
        self.built.insert(key, (identity, config.clone()));
        config
    }

    /// The layer in the file at `path`, read again if it changed.
    fn load(&mut self, path: &Path, dir: &Path, errors: &mut Vec<String>) -> Option<Arc<Layer>> {
        let Ok(metadata) = std::fs::metadata(path) else {
            self.files.remove(path);
            return None;
        };
        if !metadata.is_file() {
            return None;
        }
        let stamp = (
            metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            metadata.len(),
        );
        let fresh = self
            .files
            .get(path)
            .is_some_and(|cached| cached.stamp == stamp);
        if !fresh {
            let layer = std::fs::read_to_string(path)
                .map_err(|error| error.to_string())
                .and_then(|text| Layer::parse(&text, dir.to_path_buf()))
                .map(Arc::new);
            self.files
                .insert(path.to_path_buf(), Cached { stamp, layer });
        }
        match &self.files[path].layer {
            Ok(layer) => Some(layer.clone()),
            Err(error) => {
                errors.push(format!("{}: {error}", path.display()));
                None
            }
        }
    }
}

/// `file` relative to `dir`, with `/` between segments, or `None` for a file
/// outside it.
fn relative_path(file: &Path, dir: &Path) -> Option<String> {
    let relative = file.strip_prefix(dir).ok()?;
    let parts: Vec<_> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect();
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(config: &Config) -> Vec<&'static str> {
        config.rules.iter().map(|rule| rule.info.name).collect()
    }

    #[test]
    fn defaults_run_the_spacing_rules() {
        let config = Config::default();
        assert!(names(&config).contains(&"trailing-spaces"));
        assert!(!names(&config).contains(&"proper-ellipsis"));
        assert!(config.errors().is_empty());
    }

    #[test]
    fn a_file_turns_rules_on_and_off() {
        let config = Config::from_toml(
            r#"
            [rules]
            trailing-spaces = false
            MD012 = { max = 2 }
            proper-ellipsis = { level = "info", fix-on-save = true }
            cjk-spacing = true
            "#,
        );
        assert!(config.errors().is_empty(), "{:?}", config.errors());
        let names = names(&config);
        assert!(!names.contains(&"trailing-spaces"));
        let rule = |name| {
            config
                .rules
                .iter()
                .find(|rule| rule.info.name == name)
                .unwrap()
        };
        assert_eq!(rule("consecutive-blank-lines").options, Options::Max(2));
        assert_eq!(rule("proper-ellipsis").level, Level::Info);
        assert!(rule("proper-ellipsis").fix_on_save);
        assert_eq!(rule("cjk-spacing").level, Level::Warning);
    }

    #[test]
    fn mistakes_are_reported_and_skipped() {
        let config = Config::from_toml(
            r#"
            [rules]
            no-such-rule = true
            consecutive-blank-lines = { max = 0 }
            heading-increment = "loud"

            [[custom]]
            name = "bad"
            pattern = "("
            "#,
        );
        assert_eq!(config.errors().len(), 4, "{:?}", config.errors());
        // A rule with bad options still runs, with its defaults.
        let rule = config
            .rules
            .iter()
            .find(|rule| rule.info.name == "consecutive-blank-lines")
            .unwrap();
        assert_eq!(rule.options, Options::Max(1));

        let broken = Config::from_toml("rules = [");
        assert_eq!(broken.errors().len(), 1);
        assert!(names(&broken).contains(&"trailing-spaces"));
    }

    #[test]
    fn custom_patterns_replace_text() {
        let config = Config::from_toml(
            r#"
            [rules]
            trailing-spaces = false
            [[custom]]
            name = "colon"
            pattern = '(\p{Han}):'
            replacement = '$1：'
            [[custom]]
            name = "todo"
            pattern = 'TODO'
            message = "Unfinished"
            "#,
        );
        let source = "中文: `中文:` TODO\n";
        let found = crate::lint(source, &config);
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].message, "Unfinished");
        assert!(found[1].fix.is_empty());
        let fixed = crate::fix(source, &config, crate::FixMode::All).unwrap();
        assert_eq!(fixed.text, "中文： `中文:` TODO\n");
        // Custom fixes wait for an explicit fix, unless told otherwise.
        assert!(crate::fix(source, &config, crate::FixMode::OnSave).is_none());
    }

    fn write(dir: &Path, path: &str, text: &str) -> PathBuf {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn folders_layer_from_the_top_down() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            CONFIG_FILE_NAME,
            "ignore = [\"Templates\"]\n[rules]\ncjk-spacing = true\nproper-ellipsis = true\n",
        );
        write(
            root,
            &format!("Journal/{CONFIG_FILE_NAME}"),
            "[rules]\nproper-ellipsis = false\n[[overrides]]\nfiles = [\"2026/**\"]\nrules = { single-h1 = true }\n",
        );
        let mut resolver = Resolver::new(None);
        let resolve = |resolver: &mut Resolver, path: &str| {
            resolver.resolve(Some(&root.join(path)), Some(root))
        };

        let top = resolve(&mut resolver, "a.md");
        assert!(names(&top).contains(&"proper-ellipsis"));
        let journal = resolve(&mut resolver, "Journal/b.md");
        assert!(names(&journal).contains(&"cjk-spacing"));
        assert!(!names(&journal).contains(&"proper-ellipsis"));
        assert!(!names(&journal).contains(&"single-h1"));
        let dated = resolve(&mut resolver, "Journal/2026/c.md");
        assert!(names(&dated).contains(&"single-h1"));
        assert!(resolve(&mut resolver, "Templates/t.md").is_ignored());
        assert!(resolve(&mut resolver, "x/Templates/t.md").is_ignored());

        // Built once, then reused until a file changes.
        let again = resolve(&mut resolver, "Journal/b.md");
        assert!(Arc::ptr_eq(&journal, &again));
        write(
            root,
            &format!("Journal/{CONFIG_FILE_NAME}"),
            "root = true\n",
        );
        let changed = resolve(&mut resolver, "Journal/b.md");
        assert!(!Arc::ptr_eq(&journal, &changed));
        // `root = true` stops at the journal: the top's rules no longer apply.
        assert!(!names(&changed).contains(&"cjk-spacing"));
    }

    #[test]
    fn the_global_file_sits_below_the_folders() {
        let dir = tempfile::tempdir().unwrap();
        let settings = write(
            dir.path(),
            "settings/lint.toml",
            "[rules]\ncjk-spacing = true\nproper-ellipsis = true\n",
        );
        let notes = dir.path().join("notes");
        write(
            &notes,
            CONFIG_FILE_NAME,
            "show-issues = true\n[rules]\nproper-ellipsis = false\n",
        );
        let mut resolver = Resolver::new(Some(settings));
        let config = resolver.resolve(Some(&notes.join("a.md")), Some(&notes));
        assert!(names(&config).contains(&"cjk-spacing"));
        assert!(!names(&config).contains(&"proper-ellipsis"));
        assert_eq!(config.show_issues(), Some(true));
        assert_eq!(config.fix_on_save(), None);
        // An untitled note takes the folder's configuration.
        let untitled = resolver.resolve(None, Some(&notes));
        assert!(!names(&untitled).contains(&"proper-ellipsis"));
    }

    #[test]
    fn a_broken_file_is_reported_and_skipped() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), CONFIG_FILE_NAME, "[rules\n");
        let mut resolver = Resolver::new(None);
        let config = resolver.resolve(Some(&dir.path().join("a.md")), Some(dir.path()));
        assert_eq!(config.errors().len(), 1);
        assert!(names(&config).contains(&"trailing-spaces"));
    }
}
