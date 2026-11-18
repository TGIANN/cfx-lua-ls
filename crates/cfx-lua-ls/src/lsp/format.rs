//! Formatting settings as LuaLS takes them: `Lua.format.enable` and
//! `Lua.format.defaultConfig` from `workspace/didChangeConfiguration`, the nearest
//! `.editorconfig` (it replaces `defaultConfig`), then the editor's `tabSize` and
//! `insertSpaces` on top.

use cfx_lua_fmt::{EditorConfig, Style};
use lsp_types::{FormattingOptions, Uri};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct Settings {
    pub(crate) enable: bool,
    pub(crate) default_config: HashMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enable: true, default_config: HashMap::new() }
    }
}

/// `Lua.format` out of the settings object a client pushes (`{ "Lua": { "format": ... } }`).
pub(crate) fn settings(value: &Value) -> Settings {
    let format = &value["Lua"]["format"];
    let default_config = format["defaultConfig"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| {
                    let v = match v {
                        Value::String(s) => s.clone(),
                        Value::Bool(b) => b.to_string(),
                        Value::Number(n) => n.to_string(),
                        _ => return None,
                    };
                    Some((k.clone(), v))
                })
                .collect()
        })
        .unwrap_or_default();
    Settings { enable: format["enable"].as_bool().unwrap_or(true), default_config }
}

/// The style for one request.
pub(crate) fn style(uri: &Uri, settings: &Settings, workspace: &[PathBuf], options: &FormattingOptions) -> Style {
    let style = match file_path(uri).and_then(|p| editorconfig(&p, workspace)) {
        Some(config) => config.style(uri.as_str()),
        None => {
            let mut style = Style::default();
            style.apply(&settings.default_config);
            style
        }
    };
    style.with_editor_options(Some(options.tab_size as usize), Some(options.insert_spaces))
}

/// The nearest `.editorconfig` from the file's folder up to the workspace folder
/// that holds it (LuaLS `provider/formatting.lua`); a file outside every folder
/// climbs to the root, as in LuaLS; with no workspace, only the file's own folder.
/// Read on every request, so edits to it count at once.
fn editorconfig(file: &Path, workspace: &[PathBuf]) -> Option<EditorConfig> {
    let mut dir = file.parent()?;
    loop {
        if let Ok(bytes) = std::fs::read(dir.join(".editorconfig")) {
            return Some(EditorConfig::parse(&String::from_utf8_lossy(&bytes)));
        }
        if workspace.is_empty() || workspace.iter().any(|w| same_path(w, dir)) {
            return None;
        }
        dir = dir.parent()?;
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

/// The local path of a `file:` URI.
pub(crate) fn file_path(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let (host, path) = rest.split_at(slash);
    let path = percent_decode(path)?;
    let path = match host {
        "" | "localhost" => path,
        _ if cfg!(windows) => format!("//{host}{path}"),
        _ => return None,
    };
    // `/c:/x` is `c:/x` on Windows.
    let b = path.as_bytes();
    if cfg!(windows) && b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        return Some(PathBuf::from(&path[1..]));
    }
    Some(PathBuf::from(path))
}

/// The `file:` URI of a local path.
pub(crate) fn file_uri(path: &std::path::Path) -> Option<Uri> {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~' | b':') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out.parse().ok()
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(byte) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::str::FromStr;

    #[test]
    fn settings_from_the_pushed_object() {
        let s = settings(&json!({ "Lua": { "format": { "enable": false, "defaultConfig": { "indent_size": "2", "max_line_length": 80, "x": [] } } } }));
        assert!(!s.enable);
        assert_eq!(s.default_config.get("indent_size").map(String::as_str), Some("2"));
        assert_eq!(s.default_config.get("max_line_length").map(String::as_str), Some("80"));
        assert!(!s.default_config.contains_key("x"));
        assert!(settings(&json!(null)).enable);
    }

    #[test]
    fn file_uris_to_paths() {
        let p = file_path(&Uri::from_str("file:///c%3A/My%20Res/%5Bscripts%5D/a.lua").unwrap()).unwrap();
        if cfg!(windows) {
            assert_eq!(p, PathBuf::from("c:/My Res/[scripts]/a.lua"));
        } else {
            assert_eq!(p, PathBuf::from("/c:/My Res/[scripts]/a.lua"));
        }
        assert_eq!(file_path(&Uri::from_str("untitled:Untitled-1").unwrap()), None);
    }
}
