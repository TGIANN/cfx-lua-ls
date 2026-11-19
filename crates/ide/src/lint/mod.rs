//! Diagnostics beyond syntax: LuaLS's rules and the FiveM, ox_lib, performance and
//! style rules of `docs/RULES.md`. One walk over the tree feeds every rule; the
//! results then pass through the off switches: `---@diagnostic disable*` comments
//! in the file and the configured disabled codes / severities.

mod docs;
mod fivem;
mod flow;
mod framework;
mod luadoc;
mod luals;
mod ox;
mod signature;
mod style;

pub use docs::{RULE_DOCS, describe};

use crate::library::Game;
use crate::manifest::FileSide;
use crate::names::{Access, Names, Target};
use crate::{Parse, TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::{SyntaxNode, SyntaxToken};
use rowan::{NodeOrToken, WalkEvent};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    pub fn parse(s: &str) -> Option<Severity> {
        Some(match s.to_ascii_lowercase().as_str() {
            "error" => Severity::Error,
            "warning" => Severity::Warning,
            "information" | "info" => Severity::Information,
            "hint" => Severity::Hint,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Unnecessary,
    Deprecated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub title: String,
    pub edits: Vec<(TextRange, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lint {
    pub range: TextRange,
    pub code: &'static str,
    pub message: String,
    pub severity: Severity,
    pub tag: Option<Tag>,
    pub fix: Option<Fix>,
}

/// Every rule with its default severity; `None`: off by default.
/// Every diagnostic code `---@diagnostic` may name: LuaLS's and this server's.
pub(crate) fn diagnostic_names() -> impl Iterator<Item = &'static str> {
    luadoc::LUALS_CODES.iter().copied().chain(RULES.iter().map(|(c, _)| *c))
}

pub const RULES: &[(&str, Option<Severity>)] = &[
    // LuaLS
    ("unused-local", Some(Severity::Hint)),
    ("unused-vararg", Some(Severity::Hint)),
    ("unused-label", Some(Severity::Hint)),
    ("redefined-local", Some(Severity::Hint)),
    ("trailing-space", Some(Severity::Hint)),
    ("empty-block", Some(Severity::Hint)),
    ("code-after-break", Some(Severity::Hint)),
    ("undefined-global", Some(Severity::Warning)),
    ("global-in-nil-env", Some(Severity::Warning)),
    ("undefined-env-child", Some(Severity::Information)),
    ("lowercase-global", Some(Severity::Information)),
    ("unbalanced-assignments", Some(Severity::Warning)),
    ("redundant-value", Some(Severity::Warning)),
    ("duplicate-index", Some(Severity::Warning)),
    ("newline-call", Some(Severity::Warning)),
    ("count-down-loop", Some(Severity::Warning)),
    ("deprecated", Some(Severity::Warning)),
    ("undefined-doc-name", Some(Severity::Warning)),
    ("unreachable-code", Some(Severity::Hint)),
    ("redundant-return", Some(Severity::Hint)),
    ("unused-function", Some(Severity::Hint)),
    ("ambiguity-1", Some(Severity::Warning)),
    ("duplicate-doc-field", Some(Severity::Warning)),
    ("duplicate-doc-param", Some(Severity::Warning)),
    ("undefined-doc-param", Some(Severity::Warning)),
    ("doc-field-no-class", Some(Severity::Warning)),
    ("unknown-cast-variable", Some(Severity::Warning)),
    ("unknown-diag-code", Some(Severity::Warning)),
    ("unknown-operator", Some(Severity::Warning)),
    ("duplicate-doc-alias", Some(Severity::Warning)),
    ("undefined-doc-class", Some(Severity::Warning)),
    ("circle-doc-class", Some(Severity::Warning)),
    ("newfield-call", Some(Severity::Warning)),
    ("different-requires", Some(Severity::Warning)),
    ("duplicate-set-field", Some(Severity::Warning)),
    ("close-non-object", Some(Severity::Warning)),
    ("missing-return", Some(Severity::Warning)),
    ("missing-return-value", Some(Severity::Warning)),
    ("redundant-return-value", Some(Severity::Warning)),
    ("redundant-parameter", Some(Severity::Warning)),
    ("missing-parameter", Some(Severity::Warning)),
    ("discard-returns", Some(Severity::Warning)),
    ("param-type-mismatch", Some(Severity::Warning)),
    ("assign-type-mismatch", Some(Severity::Warning)),
    ("cast-local-type", Some(Severity::Warning)),
    ("return-type-mismatch", Some(Severity::Warning)),
    ("missing-fields", Some(Severity::Warning)),
    ("need-check-nil", Some(Severity::Warning)),
    ("undefined-field", Some(Severity::Warning)),
    ("inject-field", Some(Severity::Warning)),
    ("cast-type-mismatch", Some(Severity::Warning)),
    ("invisible", Some(Severity::Warning)),
    ("await-in-sync", None),
    ("not-yieldable", None),
    ("incomplete-signature-doc", None),
    ("missing-global-doc", None),
    ("missing-local-export-doc", None),
    ("global-element", None),
    ("no-unknown", None),
    // FiveM
    ("fivem/wrong-side-native", Some(Severity::Error)),
    ("fivem/backtick-hash", Some(Severity::Warning)),
    ("fivem/gethashkey", Some(Severity::Information)),
    ("fivem/citizen-prefix", Some(Severity::Hint)),
    ("fivem/native-distance", Some(Severity::Warning)),
    ("fivem/getplayerped-self", Some(Severity::Warning)),
    ("fivem/loop-without-wait", Some(Severity::Warning)),
    ("fivem/invariant-native-in-loop", Some(Severity::Information)),
    ("fivem/unsigned-hash", Some(Severity::Warning)),
    ("fivem/source-after-wait", Some(Severity::Warning)),
    ("fivem/unchecked-net-arg", Some(Severity::Warning)),
    ("fivem/sandbox-blocked", Some(Severity::Error)),
    ("fivem/sandbox-path", Some(Severity::Error)),
    ("fivem/sandbox-write", Some(Severity::Warning)),
    ("fivem/readdir-pairs", Some(Severity::Error)),
    ("fivem/sandbox-setlocale", Some(Severity::Warning)),
    // Exports, NUI and frameworks
    ("fivem/unknown-export", Some(Severity::Warning)),
    ("fivem/export-wrong-side", Some(Severity::Error)),
    ("fivem/export-dot-call", Some(Severity::Warning)),
    ("fivem/nui-message-encoded", Some(Severity::Information)),
    ("qb/unchecked-player", Some(Severity::Information)),
    ("qb/core-object-event", Some(Severity::Warning)),
    ("esx/shared-object-event", Some(Severity::Information)),
    ("fivem/net-event-same-side", Some(Severity::Information)),
    // fxmanifest.lua against the disk
    ("fivem/manifest-missing-file", Some(Severity::Error)),
    ("fivem/manifest-path-case", Some(Severity::Warning)),
    ("fivem/manifest-no-match", Some(Severity::Warning)),
    ("fivem/manifest-missing-import", Some(Severity::Error)),
    // ox_lib (only where the manifest loads `@ox_lib/init.lua` for the file's side)
    ("ox/cache-ped", Some(Severity::Information)),
    ("ox/cache-playerid", Some(Severity::Information)),
    ("ox/cache-serverid", Some(Severity::Information)),
    ("ox/cache-vehicle", Some(Severity::Information)),
    ("ox/cache-weapon", Some(Severity::Hint)),
    ("ox/cache-resource", Some(Severity::Information)),
    ("ox/request-asset", Some(Severity::Information)),
    ("ox/callback", Some(Severity::Hint)),
    ("ox/notify", Some(Severity::Hint)),
    ("ox/text-ui", Some(Severity::Hint)),
    ("ox/progress", Some(Severity::Hint)),
    ("ox/closest", Some(Severity::Hint)),
    ("ox/keybind", Some(Severity::Hint)),
    ("ox/add-command", Some(Severity::Hint)),
    ("ox/raycast", Some(Severity::Hint)),
    // Lua performance
    ("perf/concat-in-loop", Some(Severity::Warning)),
    ("perf/table-insert-append", Some(Severity::Hint)),
    ("perf/sequential-index-keys", Some(Severity::Information)),
    ("perf/constant-table-in-loop", Some(Severity::Hint)),
    ("perf/closure-in-loop", Some(Severity::Hint)),
    ("perf/global-in-loop", None),
    ("perf/runtime-load", Some(Severity::Information)),
    // Readability and naming
    ("style/redundant-boolean-return", Some(Severity::Hint)),
    ("style/string-key-access", Some(Severity::Hint)),
    ("naming/local", Some(Severity::Information)),
    ("naming/const", Some(Severity::Information)),
];

pub fn default_severity(code: &str) -> Option<Severity> {
    RULES.iter().find(|(c, _)| *c == code).and_then(|(_, s)| *s)
}

/// The configured switches (`.luarc.json` `diagnostics.*`, `Lua.diagnostics.*`).
#[derive(Debug, Clone, Default)]
pub struct LintConfig {
    pub disable: HashSet<String>,
    /// Code -> severity, or `None` to turn it off (LuaLS `"Off"` / `"none"`).
    pub severity: HashMap<String, Option<Severity>>,
    /// `diagnostics.globals`: names that are never undefined.
    pub globals: HashSet<String>,
    /// `doc.privateName` / `doc.protectedName`: globs (`m_*`) naming the
    /// fields that are private / protected without a `---@field private`.
    pub private_names: Vec<String>,
    pub protected_names: Vec<String>,
    /// `doc.regengine` is `lua`: the names are Lua patterns (`string.find`).
    pub lua_name_patterns: bool,
    /// `cfx.lualsCompat`: LuaLS's own behaviour where this server is
    /// deliberately narrower (nil checked, a local typed by its first value,
    /// `unpack` deprecated, an empty function body not a declaration, every
    /// unknown `---@diagnostic` code).
    pub luals_compat: bool,
    /// `type.weakNilCheck`, when set; unset: on, unless `luals_compat`.
    pub weak_nil_check: Option<bool>,
    /// `type.weakUnionCheck`: a union fits when one of its members does.
    pub weak_union_check: bool,
    /// `type.castNumberToInteger`: a `number` fits `integer`.
    pub cast_number_to_integer: bool,
    /// `type.checkTableShape`: a table fits a class only with its fields.
    pub check_table_shape: bool,
    /// `diagnostics.unusedLocalExclude`: globs of locals never unused.
    pub unused_local_exclude: Vec<String>,
}

impl LintConfig {
    /// Whether nil is checked against a declared type.
    pub fn strict_nil(&self) -> bool {
        !self.weak_nil_check.unwrap_or(!self.luals_compat)
    }
}

impl LintConfig {
    /// A configured severity also turns on a rule that is off by default.
    fn severity_of(&self, code: &'static str, default: Option<Severity>) -> Option<Severity> {
        if self.disable.contains(code) {
            return None;
        }
        match self.severity.get(code) {
            Some(s) => *s,
            None => default,
        }
    }
}

/// What a file's diagnostics depend on beyond its own text.
pub struct Context<'a> {
    pub side: FileSide,
    pub game: Game,
    /// The resource's folder name (its name on the server).
    pub resource: Option<&'a str>,
    /// `@ox_lib/init.lua` is loaded for this file's side.
    pub ox_lib: bool,
    /// Whether a global is defined by another file the file can see (its resource
    /// on the same side, the manifest's `@res/file.lua` imports).
    pub defined_elsewhere: &'a dyn Fn(&str) -> bool,
    /// Whether another file sharing the file's Lua state reads a global.
    pub used_elsewhere: &'a dyn Fn(&str) -> bool,
    /// `undefined-global` stays silent: the resource has escrow files or imports a
    /// file that is not in the workspace (they may define any global).
    pub globals_unknown: bool,
    /// `---@alias` / `---@enum` types the other files of the resource, and the
    /// resources its manifest names, declare.
    pub aliases: &'a [(String, crate::luacats::DocType)],
    pub config: &'a LintConfig,
}

impl Default for Context<'_> {
    fn default() -> Self {
        static NO: fn(&str) -> bool = |_| false;
        static CONFIG: std::sync::OnceLock<LintConfig> = std::sync::OnceLock::new();
        Context {
            side: FileSide::Unknown,
            game: Game::Gta5,
            resource: None,
            ox_lib: false,
            defined_elsewhere: &NO,
            used_elsewhere: &NO,
            globals_unknown: false,
            aliases: &[],
            config: CONFIG.get_or_init(LintConfig::default),
        }
    }
}

/// The walk state the rules share.
pub(crate) struct Linter<'a> {
    pub(crate) text: &'a str,
    pub(crate) root: SyntaxNode,
    pub(crate) names: &'a Names,
    pub(crate) ctx: &'a Context<'a>,
    out: Vec<Lint>,
    /// Name reference by the start of its token.
    refs: HashMap<TextSize, (Target, Access)>,
    /// Enclosing loops and functions, innermost last.
    pub(crate) frames: Vec<Frame>,
    /// The file's types, built on first use and shared by the rules.
    types: std::cell::OnceCell<crate::types::FileTypes<'a>>,
}

#[derive(Debug)]
pub(crate) enum Frame {
    /// `finite`: a `for` loop.
    Loop { finite: bool },
    Function {
        /// An event handler: `source` is only valid before its first yield.
        handler: bool,
        /// A yielding call has finished in this function.
        yielded: bool,
    },
}

impl<'a> Linter<'a> {
    pub(crate) fn push(&mut self, code: &'static str, range: TextRange, message: impl Into<String>) {
        self.push_full(code, range, message.into(), None, None);
    }

    pub(crate) fn push_fix(&mut self, code: &'static str, range: TextRange, message: impl Into<String>, fix: Fix) {
        self.push_full(code, range, message.into(), None, Some(fix));
    }

    /// With a severity of its own (LuaLS gives some repeats a lower one), unless
    /// the code is configured.
    pub(crate) fn push_level(&mut self, code: &'static str, range: TextRange, message: String, level: Severity, tag: Option<Tag>) {
        if self.ctx.config.disable.contains(code) {
            return;
        }
        let severity = match self.ctx.config.severity.get(code) {
            Some(None) => return,
            Some(Some(s)) => *s,
            None => level,
        };
        self.out.push(Lint { range, code, message, severity, tag, fix: None });
    }

    pub(crate) fn push_full(&mut self, code: &'static str, range: TextRange, message: String, tag: Option<Tag>, fix: Option<Fix>) {
        if !RULES.iter().any(|(c, _)| *c == code) {
            return;
        }
        let Some(severity) = self.ctx.config.severity_of(code, default_severity(code)) else { return };
        self.out.push(Lint { range, code, message, severity, tag, fix });
    }

    /// Whether a rule is on (by default or by the configuration).
    pub(crate) fn enabled(&self, code: &'static str) -> bool {
        self.ctx.config.severity_of(code, default_severity(code)).is_some()
    }

    pub(crate) fn types(&self) -> &crate::types::FileTypes<'a> {
        self.types.get_or_init(|| {
            crate::types::FileTypes::from_root(self.root.clone(), self.names, self.text).with_config(self.ctx.config).with_aliases(self.ctx.aliases)
        })
    }

    /// The text of a node or token.
    pub(crate) fn src(&self, r: TextRange) -> &'a str {
        &self.text[r]
    }

    /// Whether a `NAME_EXPR` names a global (not a local).
    pub(crate) fn is_global_name(&self, expr: &SyntaxNode) -> bool {
        expr.kind() == NAME_EXPR
            && expr.first_token().is_some_and(|t| matches!(self.refs.get(&t.text_range().start()), Some((Target::Global, _))))
    }

    /// `a`, `a.b.c` when the chain starts at a global name.
    pub(crate) fn global_path(&self, expr: &SyntaxNode) -> Option<String> {
        match expr.kind() {
            NAME_EXPR if self.is_global_name(expr) => Some(self.src(expr.text_range()).to_string()),
            INDEX_EXPR => {
                let object = self.global_path(&expr.first_child()?)?;
                let field = field_name(expr)?;
                Some(format!("{object}.{}", field.text()))
            }
            _ => None,
        }
    }

    /// The global path a call calls (`Wait`, `Citizen.Wait`), if it is one.
    pub(crate) fn callee(&self, call: &SyntaxNode) -> Option<String> {
        if call.kind() != CALL_EXPR {
            return None;
        }
        self.global_path(&call.first_child()?)
    }

    /// The argument expressions of a call.
    pub(crate) fn args(call: &SyntaxNode) -> Vec<SyntaxNode> {
        call.children().find(|c| c.kind() == ARG_LIST).map(|a| a.children().collect()).unwrap_or_default()
    }

    /// Inside a loop body of the current function.
    pub(crate) fn in_loop(&self) -> bool {
        matches!(self.frames.last(), Some(Frame::Loop { .. }))
    }

    pub(crate) fn function(&mut self) -> Option<(&mut bool, &mut bool)> {
        self.frames.iter_mut().rev().find_map(|f| match f {
            Frame::Function { handler, yielded } => Some((handler, yielded)),
            Frame::Loop { .. } => None,
        })
    }
}

/// The `.name` of an `INDEX_EXPR` (not `[k]`).
/// The file's doc groups that are comments: a `---` line inside a long string
/// or a block comment is text, not docs.
pub(crate) fn code_doc_groups(root: &SyntaxNode, text: &str) -> Vec<crate::luacats::DocGroup> {
    let mut groups = crate::luacats::doc_groups(text);
    groups.retain(|g| g.tags.iter().all(|t| root.token_at_offset(t.range.start()).any(|tok| tok.kind() == COMMENT)));
    groups
}

pub(crate) fn field_name(expr: &SyntaxNode) -> Option<SyntaxToken> {
    let mut tokens = expr.children_with_tokens().filter_map(|e| e.into_token());
    tokens.find(|t| t.kind() == DOT)?;
    tokens.find(|t| t.kind() == NAME)
}

/// Calls that yield the running coroutine (`source` is restored after them).
pub(crate) fn is_yield(path: &str) -> bool {
    matches!(path, "Wait" | "Citizen.Wait" | "Citizen.Await" | "coroutine.yield" | "lib.waitFor" | "lib.callback.await")
        || path.ends_with(".await")
        || path.ends_with(".Await")
}

/// A lint made outside the tree walk (the workspace's rules), at the configured
/// severity; `None` when the code is off.
pub(crate) fn make(code: &'static str, range: TextRange, message: String, tag: Option<Tag>, config: &LintConfig) -> Option<Lint> {
    RULES.iter().find(|(c, _)| *c == code)?;
    let severity = config.severity_of(code, default_severity(code))?;
    Some(Lint { range, code, message, severity, tag, fix: None })
}

/// Applies every fix whose edits overlap no fix taken before it; returns the new
/// text and the number of fixes applied.
pub fn apply_fixes(text: &str, lints: &[Lint]) -> (String, usize) {
    let overlaps = |a: TextRange, b: TextRange| a == b || (a.start() < b.end() && b.start() < a.end());
    let mut taken: Vec<(TextRange, &str)> = Vec::new();
    let mut count = 0;
    for fix in lints.iter().filter_map(|l| l.fix.as_ref()) {
        if fix.edits.iter().any(|(r, _)| taken.iter().any(|(t, _)| overlaps(*r, *t))) {
            continue;
        }
        taken.extend(fix.edits.iter().map(|(r, t)| (*r, t.as_str())));
        count += 1;
    }
    taken.sort_by_key(|(r, _)| std::cmp::Reverse((r.start(), r.end())));
    let mut out = text.to_string();
    for (r, t) in taken {
        out.replace_range(std::ops::Range::<usize>::from(r), t);
    }
    (out, count)
}

/// All diagnostics of a file except syntax errors, sorted by position.
pub fn lint(parse: &Parse, text: &str, names: &Names, ctx: &Context) -> Vec<Lint> {
    let refs = names.refs.iter().map(|r| (r.range.start(), (r.target, r.access))).collect();
    let root = parse.syntax();
    let mut l = Linter { text, root: root.clone(), names, ctx, out: Vec::new(), refs, frames: Vec::new(), types: std::cell::OnceCell::new() };
    luals::names(&mut l);
    flow::unused_functions(&mut l);
    flow::duplicate_set_fields(&mut l);
    luadoc::run(&mut l);
    signature::run(&mut l);
    if (l.enabled("await-in-sync") || l.enabled("not-yieldable")) && (text.contains("async") || text.contains("coroutine.yield")) {
        let found = l.types().async_calls();
        for (range, code, message) in found {
            l.push(code, range, message);
        }
    }
    luals::text(&mut l, &root);
    for event in root.preorder_with_tokens() {
        match event {
            WalkEvent::Enter(NodeOrToken::Node(n)) => {
                match n.kind() {
                    // A `while` / `repeat` condition runs on every pass; a `for` header once.
                    WHILE_STAT | REPEAT_STAT => l.frames.push(Frame::Loop { finite: false }),
                    BLOCK if is_for_body(&n) => l.frames.push(Frame::Loop { finite: true }),
                    FUNC_BODY => {
                        let handler = fivem::is_event_handler(&l, &n);
                        l.frames.push(Frame::Function { handler, yielded: false });
                    }
                    _ => {}
                }
                luals::node(&mut l, &n);
                flow::node(&mut l, &n);
                fivem::node(&mut l, &n);
                ox::node(&mut l, &n);
                style::node(&mut l, &n);
                framework::node(&mut l, &n);
            }
            WalkEvent::Leave(NodeOrToken::Node(n)) => match n.kind() {
                WHILE_STAT | REPEAT_STAT | FUNC_BODY => {
                    l.frames.pop();
                }
                BLOCK if is_for_body(&n) => {
                    l.frames.pop();
                }
                CALL_EXPR => {
                    if l.callee(&n).is_some_and(|p| is_yield(&p))
                        && let Some((_, yielded)) = l.function()
                    {
                        *yielded = true;
                    }
                }
                _ => {}
            },
            WalkEvent::Enter(NodeOrToken::Token(t)) => fivem::token(&mut l, &t),
            WalkEvent::Leave(NodeOrToken::Token(_)) => {}
        }
    }
    let mut out = suppress(parse, text, l.out);
    out.sort_by_key(|d| (d.range.start(), d.range.end()));
    out
}

fn is_for_body(block: &SyntaxNode) -> bool {
    block.parent().is_some_and(|p| matches!(p.kind(), NUMERIC_FOR_STAT | GENERIC_FOR_STAT))
}

/// `---@diagnostic disable-next-line: a, b`, `disable-line`, `disable` / `enable`
/// (to the end of the file or the matching `enable`); no codes means all.
pub(crate) fn suppress(parse: &Parse, text: &str, lints: Vec<Lint>) -> Vec<Lint> {
    let line_starts: Vec<usize> =
        std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let line_of = |o: TextSize| line_starts.partition_point(|&s| s <= u32::from(o) as usize) - 1;
    // (line, kind, codes); kind: 0 next line, 1 this line, 2 disable from here, 3 enable from here.
    let mut marks: Vec<(usize, u8, Option<Vec<String>>)> = Vec::new();
    for t in parse.syntax().descendants_with_tokens().filter_map(|e| e.into_token()) {
        if t.kind() != COMMENT {
            continue;
        }
        let Some(rest) = t.text().strip_prefix("---@diagnostic") else { continue };
        let rest = rest.trim();
        let (action, codes) = match rest.split_once(':') {
            Some((a, c)) => (a.trim(), Some(c.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect())),
            None => (rest, None),
        };
        let kind = match action {
            "disable-next-line" => 0,
            "disable-line" => 1,
            "disable" => 2,
            "enable" => 3,
            _ => continue,
        };
        marks.push((line_of(t.text_range().start()), kind, codes));
    }
    if marks.is_empty() {
        return lints;
    }
    let matches = |codes: &Option<Vec<String>>, code: &str| codes.as_ref().is_none_or(|c| c.iter().any(|x| x == code));
    lints
        .into_iter()
        .filter(|d| {
            let line = line_of(d.range.start());
            // As LuaLS counts them: two `disable` need two `enable`.
            let mut off = 0i32;
            for (l, kind, codes) in &marks {
                match kind {
                    0 if *l + 1 == line && matches(codes, d.code) => return false,
                    1 if *l == line && matches(codes, d.code) => return false,
                    2 if *l < line && matches(codes, d.code) => off += 1,
                    3 if *l < line && matches(codes, d.code) => off -= 1,
                    _ => {}
                }
            }
            off <= 0
        })
        .collect()
}

#[cfg(test)]
mod tests;
