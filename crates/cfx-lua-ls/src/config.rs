//! Diagnostic settings: `.luarc.json` / `.luarc.jsonc` in a workspace folder, then
//! the client's `Lua.diagnostics.*` on top, as LuaLS reads them. Both the flat
//! (`"diagnostics.globals": [...]`) and the nested (`"diagnostics": {...}`) forms
//! are accepted.

use cfx_lua_ide::lint::{LintConfig, Severity};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(crate) struct Diagnostics {
    /// `diagnostics.enable`: `false` keeps only syntax errors.
    pub(crate) enable: bool,
    pub(crate) lint: LintConfig,
    /// `format.defaultConfig` (`codestyle-check` outside an editor: the style
    /// when no `.editorconfig` applies).
    pub(crate) format_default: std::collections::HashMap<String, String>,
    /// The folders the settings were read for: the `.editorconfig` search stops
    /// at them.
    pub(crate) folders: Vec<std::path::PathBuf>,
    /// `diagnostics.groupSeverity` / `groupFileStatus` (`true`: `Any` or
    /// `Opened`), and the codes whose own value ends in `!`, which the groups
    /// leave alone.
    group_severity: std::collections::HashMap<String, Severity>,
    group_status: std::collections::HashMap<String, bool>,
    forced_severity: std::collections::HashSet<String>,
    forced_status: std::collections::HashSet<String>,
}

impl Default for Diagnostics {
    fn default() -> Self {
        Diagnostics {
            enable: true,
            lint: LintConfig::default(),
            format_default: Default::default(),
            folders: Vec::new(),
            group_severity: Default::default(),
            group_status: Default::default(),
            forced_severity: Default::default(),
            forced_status: Default::default(),
        }
    }
}

/// LuaLS's diagnostic groups (`script/proto/diagnostic.lua`, 3.19.2), for
/// `diagnostics.groupSeverity` and `diagnostics.groupFileStatus`.
const GROUPS: &[(&str, &[&str])] = &[
    (
        "unused",
        &[
            "unused-local",
            "unused-function",
            "unused-label",
            "unused-vararg",
            "trailing-space",
            "redundant-return",
            "empty-block",
            "code-after-break",
            "unreachable-code",
        ],
    ),
    (
        "unbalanced",
        &[
            "redundant-value",
            "unbalanced-assignments",
            "redundant-parameter",
            "missing-parameter",
            "missing-return-value",
            "redundant-return-value",
            "missing-return",
            "missing-fields",
        ],
    ),
    (
        "type-check",
        &[
            "need-check-nil",
            "undefined-field",
            "cast-local-type",
            "assign-type-mismatch",
            "param-type-mismatch",
            "cast-type-mismatch",
            "return-type-mismatch",
            "inject-field",
        ],
    ),
    (
        "luadoc",
        &[
            "duplicate-doc-alias",
            "undefined-doc-class",
            "undefined-doc-name",
            "circle-doc-class",
            "undefined-doc-param",
            "duplicate-doc-param",
            "doc-field-no-class",
            "duplicate-doc-field",
            "unknown-diag-code",
            "unknown-cast-variable",
            "unknown-operator",
            "incomplete-signature-doc",
            "missing-global-doc",
            "missing-local-export-doc",
        ],
    ),
    ("codestyle", &["codestyle-check", "spell-check", "name-style-check"]),
    ("ambiguity", &["newline-call", "newfield-call", "ambiguity-1", "count-down-loop", "different-requires"]),
    ("await", &["await-in-sync", "not-yieldable"]),
    ("strong", &["no-unknown"]),
    ("redefined", &["redefined-local"]),
    ("global", &["undefined-global", "global-in-nil-env", "lowercase-global", "undefined-env-child"]),
    ("conventions", &["global-element"]),
    ("duplicate", &["duplicate-index", "duplicate-set-field"]),
    ("strict", &["close-non-object", "deprecated", "discard-returns", "invisible"]),
];

/// `section.key` in either form.
fn get<'v>(v: &'v Value, section: &str, key: &str) -> Option<&'v Value> {
    v.get(section).and_then(|s| s.get(key)).or_else(|| v.get(format!("{section}.{key}")))
}

/// LuaLS severities: `Error`, `Warning`, `Information`, `Hint`, each optionally
/// with `!` (forced); `None` / `Off` turns the rule off. Unknown values keep the
/// default.
fn severity(s: &str) -> Option<Option<Severity>> {
    let s = s.trim_end_matches('!');
    match s.to_ascii_lowercase().as_str() {
        "none" | "off" => Some(None),
        "default" => None,
        _ => Severity::parse(s).map(Some),
    }
}

/// Applies one settings object (`.luarc.json` or the client's `Lua` section).
pub(crate) fn apply(d: &mut Diagnostics, v: &Value) {
    if let Some(b) = get(v, "diagnostics", "enable").and_then(Value::as_bool) {
        d.enable = b;
    }
    let strings = |key: &str| -> Vec<String> {
        get(v, "diagnostics", key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    d.lint.disable.extend(strings("disable"));
    d.lint.globals.extend(strings("globals"));
    let doc = |key: &str| -> Option<Vec<String>> {
        get(v, "doc", key).and_then(Value::as_array).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
    };
    if let Some(names) = doc("privateName") {
        d.lint.private_names = names;
    }
    if let Some(names) = doc("protectedName") {
        d.lint.protected_names = names;
    }
    if let Some(names) = doc("packageName") {
        d.lint.package_names = names;
    }
    if let Some(words) = get(v, "spell", "dict").and_then(Value::as_array) {
        d.lint.spell_dict = words.iter().filter_map(|w| w.as_str().map(str::to_string)).collect();
    }
    if let Some(map) = get(v, "format", "defaultConfig").and_then(Value::as_object) {
        d.format_default = map
            .iter()
            .filter_map(|(k, v)| match v {
                Value::String(s) => Some((k.clone(), s.clone())),
                Value::Bool(b) => Some((k.clone(), b.to_string())),
                Value::Number(n) => Some((k.clone(), n.to_string())),
                _ => None,
            })
            .collect();
    }
    if let Some(c) = get(v, "nameStyle", "config") {
        d.lint.name_style = cfx_lua_ide::lint::name_style::NameStyleConfig::parse(c);
    }
    if let Some(b) = get(v, "cfx", "lualsCompat").and_then(Value::as_bool) {
        d.lint.luals_compat = b;
    }
    if let Some(b) = get(v, "type", "weakNilCheck").and_then(Value::as_bool) {
        d.lint.weak_nil_check = Some(b);
    }
    if let Some(b) = get(v, "type", "weakUnionCheck").and_then(Value::as_bool) {
        d.lint.weak_union_check = b;
    }
    if let Some(m) = get(v, "runtime", "special").and_then(Value::as_object) {
        d.lint.special = m.iter().filter_map(|(k, b)| Some((k.clone(), b.as_str()?.to_string()))).collect();
    }
    if let Some(b) = get(v, "type", "castNumberToInteger").and_then(Value::as_bool) {
        d.lint.cast_number_to_integer = b;
    }
    if let Some(b) = get(v, "type", "checkTableShape").and_then(Value::as_bool) {
        d.lint.check_table_shape = b;
    }
    d.lint.unused_local_exclude.extend(strings("unusedLocalExclude"));
    if let Some(engine) = get(v, "doc", "regengine").and_then(Value::as_str) {
        d.lint.lua_name_patterns = engine.eq_ignore_ascii_case("lua");
    }
    if let Some(map) = get(v, "diagnostics", "severity").and_then(Value::as_object) {
        for (code, s) in map {
            let Some(s) = s.as_str() else { continue };
            if let Some(level) = severity(s) {
                if s.ends_with('!') {
                    d.forced_severity.insert(code.clone());
                }
                if level.is_none() {
                    // `"None"` (not a LuaLS severity) is an explicit off a group does not undo.
                    d.forced_status.insert(code.clone());
                }
                d.lint.severity.insert(code.clone(), level);
            }
        }
    }
    // `"Any"` / `"Opened"` turns a rule on (at its default level, Warning for
    // the rules that are off by default), `"None"` off.
    if let Some(map) = get(v, "diagnostics", "neededFileStatus").and_then(Value::as_object) {
        for (code, s) in map {
            if s.as_str().is_some_and(|s| s.ends_with('!')) {
                d.forced_status.insert(code.clone());
            }
            match s.as_str().map(|s| s.trim_end_matches('!').to_ascii_lowercase()).as_deref() {
                Some("none") => {
                    d.lint.severity.insert(code.clone(), None);
                }
                Some("any" | "opened") => {
                    let level = cfx_lua_ide::lint::enabled_severity(code);
                    d.lint.severity.entry(code.clone()).or_insert(Some(level));
                }
                _ => {}
            }
        }
    }
    // Groups, as LuaLS's `getSeverity` / `getStatus`: a group's value wins over a
    // code's own unless that ends in `!`; `"Fallback"` leaves the codes' own.
    if let Some(map) = get(v, "diagnostics", "groupFileStatus").and_then(Value::as_object) {
        for (group, s) in map {
            match s.as_str().map(str::to_ascii_lowercase).as_deref() {
                Some("any" | "opened") => {
                    d.group_status.insert(group.clone(), true);
                }
                Some("none") => {
                    d.group_status.insert(group.clone(), false);
                }
                Some("fallback") => {
                    d.group_status.remove(group);
                }
                _ => {}
            }
        }
    }
    if let Some(map) = get(v, "diagnostics", "groupSeverity").and_then(Value::as_object) {
        for (group, s) in map {
            match s.as_str() {
                Some(s) if s.eq_ignore_ascii_case("fallback") => {
                    d.group_severity.remove(group);
                }
                Some(s) => {
                    if let Some(level) = Severity::parse(s) {
                        d.group_severity.insert(group.clone(), level);
                    }
                }
                None => {}
            }
        }
    }
    for (group, codes) in GROUPS {
        if let Some(&on) = d.group_status.get(*group) {
            for code in codes.iter().filter(|c| !d.forced_status.contains(**c)) {
                if !on {
                    d.lint.severity.insert(code.to_string(), None);
                } else if !d.lint.severity.get(*code).is_some_and(Option::is_some) {
                    d.lint.severity.insert(code.to_string(), Some(cfx_lua_ide::lint::enabled_severity(code)));
                }
            }
        }
        // The level only: a group severity does not turn a rule on (LuaLS keeps
        // severity and file status apart).
        if let Some(&level) = d.group_severity.get(*group) {
            for code in codes.iter().filter(|c| !d.forced_severity.contains(**c)) {
                let on = match d.lint.severity.get(*code) {
                    Some(s) => s.is_some(),
                    None => cfx_lua_ide::lint::default_severity(code).is_some(),
                };
                if on {
                    d.lint.severity.insert(code.to_string(), Some(level));
                }
            }
        }
    }
}

/// JSON with `//` and `/* */` comments and trailing commas (LuaLS reads
/// `.luarc.json` as JSONC).
pub(crate) fn parse_jsonc(text: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut in_string) = (0, false);
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if c == b'\\' && i + 1 < bytes.len() {
                // The backslash and the whole character after it.
                let n = text[i + 1..].chars().next().map_or(0, char::len_utf8);
                out.push_str(&text[i..i + 1 + n]);
                i += 1 + n;
                continue;
            }
            in_string = c != b'"';
        } else if c == b'"' {
            in_string = true;
        } else if text[i..].starts_with("//") {
            i = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
            continue;
        } else if text[i..].starts_with("/*") {
            i = text[i + 2..].find("*/").map_or(bytes.len(), |n| i + n + 4);
            continue;
        } else if c == b',' {
            // A comma before `}` / `]` (only whitespace or comments between) is dropped.
            let rest = text[i + 1..].trim_start();
            if rest.starts_with('}') || rest.starts_with(']') {
                i += 1;
                continue;
            }
        }
        let len = text[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&text[i..i + len]);
        i += len;
    }
    serde_json::from_str(&out).ok()
}

/// The first `.luarc.json` / `.luarc.jsonc` in the folders.
pub(crate) fn luarc(folders: &[PathBuf]) -> Option<(PathBuf, Value)> {
    folders.iter().find_map(|f| {
        [".luarc.json", ".luarc.jsonc"].iter().find_map(|name| {
            let path = f.join(name);
            let text = std::fs::read_to_string(&path).ok()?;
            Some((path, parse_jsonc(&text)?))
        })
    })
}

/// `.luarc.json` settings, then the client's `Lua` section.
pub(crate) fn load(folders: &[PathBuf], client: Option<&Value>) -> Diagnostics {
    let mut d = Diagnostics { folders: folders.to_vec(), ..Diagnostics::default() };
    if let Some((_, v)) = luarc(folders) {
        apply(&mut d, &v);
    }
    if let Some(v) = client {
        apply(&mut d, v);
    }
    d
}

/// Settings for a path outside an editor session: the nearest `.luarc.json` in it
/// or above it.
pub(crate) fn load_for(path: &Path) -> Diagnostics {
    let start = if path.is_dir() { Some(path) } else { path.parent() };
    let folder = start.and_then(|s| {
        s.ancestors().find(|d| d.join(".luarc.json").is_file() || d.join(".luarc.jsonc").is_file())
    });
    load(&folder.map(|f| vec![f.to_path_buf()]).unwrap_or_default(), None)
}

pub(crate) fn is_luarc(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.eq_ignore_ascii_case(".luarc.json") || n.eq_ignore_ascii_case(".luarc.jsonc"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups() {
        let mut d = Diagnostics::default();
        let v = serde_json::json!({ "diagnostics": {
            "groupFileStatus": { "codestyle": "Any", "await": "Opened", "strict": "None", "global": "Fallback" },
            "groupSeverity": { "codestyle": "Hint", "unused": "Error", "strong": "Error" },
            "neededFileStatus": { "deprecated": "Any!" },
            "severity": { "unused-local": "Warning!", "trailing-space": "Information" },
        } });
        apply(&mut d, &v);
        let s = |c: &str| d.lint.severity.get(c).copied().unwrap_or_else(|| cfx_lua_ide::lint::default_severity(c));
        // On through the group, at the group's level.
        assert_eq!(s("spell-check"), Some(Severity::Hint));
        assert_eq!(s("codestyle-check"), Some(Severity::Hint));
        assert_eq!(s("await-in-sync"), Some(Severity::Warning));
        // Off through the group, except a code forced with `!`.
        assert_eq!(s("invisible"), None);
        assert_eq!(s("deprecated"), cfx_lua_ide::lint::default_severity("deprecated"));
        // A group level beats a code's own unless it ends in `!`, and turns nothing on.
        assert_eq!(s("unused-local"), Some(Severity::Warning));
        assert_eq!(s("trailing-space"), Some(Severity::Error));
        assert_eq!(s("no-unknown"), None);
        assert_eq!(s("undefined-global"), cfx_lua_ide::lint::default_severity("undefined-global"));
    }

    #[test]
    fn flat_nested_and_jsonc() {
        let v = parse_jsonc(
            "{\n  // comment\n  \"diagnostics.globals\": [\"Config\", \"QBCore\",],\n  /* block */ \"diagnostics\": { \"disable\": [\"lowercase-global\"], \"severity\": { \"undefined-global\": \"Error!\", \"naming/local\": \"None\" } },\n  \"url\": \"http://x//y\"\n}",
        )
        .unwrap();
        let mut d = Diagnostics::default();
        apply(&mut d, &v);
        assert!(d.lint.globals.contains("Config") && d.lint.globals.contains("QBCore"));
        assert!(d.lint.disable.contains("lowercase-global"));
        assert_eq!(d.lint.severity["undefined-global"], Some(Severity::Error));
        assert_eq!(d.lint.severity["naming/local"], None);
        assert_eq!(v["url"], "http://x//y");
        // An escape before a multi-byte character, and an unterminated string: no panic.
        assert!(parse_jsonc("{ \"a\": \"\\\u{e9}\" }").is_none());
        assert!(parse_jsonc("{ \"a\": \"\\").is_none());
        // The client's section on top.
        let client = serde_json::json!({ "diagnostics": { "enable": false } });
        apply(&mut d, &client);
        assert!(!d.enable);
    }

    #[test]
    fn type_switches() {
        let v = serde_json::json!({ "type": { "weakUnionCheck": true, "checkTableShape": true }, "type.weakNilCheck": false, "cfx.lualsCompat": true, "diagnostics.unusedLocalExclude": ["ll_*"] });
        let mut d = Diagnostics::default();
        assert!(!d.lint.strict_nil());
        apply(&mut d, &v);
        assert!(d.lint.weak_union_check && d.lint.check_table_shape && !d.lint.cast_number_to_integer);
        assert!(d.lint.luals_compat && d.lint.strict_nil());
        assert_eq!(d.lint.unused_local_exclude, ["ll_*"]);
        // `weakNilCheck` set wins over the compat default.
        apply(&mut d, &serde_json::json!({ "type.weakNilCheck": true }));
        assert!(!d.lint.strict_nil());
        apply(&mut d, &serde_json::json!({ "runtime": { "special": { "xx.assert": "assert" } } }));
        assert_eq!(d.lint.special, [("xx.assert".to_string(), "assert".to_string())]);
    }

    #[test]
    fn needed_file_status() {
        let v = serde_json::json!({ "diagnostics.neededFileStatus": { "await-in-sync": "Any!", "no-unknown": "Opened", "unused-local": "None", "deprecated": "Any" } });
        let mut d = Diagnostics::default();
        d.lint.severity.insert("deprecated".into(), Some(Severity::Error));
        apply(&mut d, &v);
        // Off by default: turned on at Warning; a configured severity is kept.
        assert_eq!(d.lint.severity["await-in-sync"], Some(Severity::Warning));
        assert_eq!(d.lint.severity["no-unknown"], Some(Severity::Warning));
        assert_eq!(d.lint.severity["unused-local"], None);
        assert_eq!(d.lint.severity["deprecated"], Some(Severity::Error));
        let v = serde_json::json!({ "doc": { "privateName": ["m_*"] }, "doc.protectedName": ["_*"] });
        apply(&mut d, &v);
        assert_eq!((d.lint.private_names.as_slice(), d.lint.protected_names.as_slice()), (&["m_*".to_string()][..], &["_*".to_string()][..]));
        assert!(!d.lint.lua_name_patterns);
        apply(&mut d, &serde_json::json!({ "doc.regengine": "lua" }));
        assert!(d.lint.lua_name_patterns);
    }
}
