//! `name-style-check`, off by default as in LuaLS: EmmyLuaCodeStyle's
//! `NameStyleChecker` and `NameStyleRuleMatcher`, with the rules of
//! `Lua.nameStyle.config` (`LuaDiagnosticStyle`).
//!
//! The walk follows the C++ one statement by statement, quirks included: only
//! the first name of a top-level `local a, b` is checked, and a `for` index is
//! never checked. `pattern` rules use the `regex` crate, which matches the
//! ECMAScript syntax of `std::regex` except look-around and back-references; a
//! pattern it cannot compile is `Off`, as an invalid one is in C++.

use super::Linter;
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::{SyntaxNode, SyntaxToken};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Case {
    Snake,
    UpperSnake,
    Camel,
    Pascal,
}

#[derive(Debug, Clone)]
pub enum Rule {
    Off,
    Case(CaseRule),
    Ignore(Vec<String>),
    Pattern { source: String, re: regex::Regex, groups: Vec<(usize, CaseRule)> },
}

/// A case rule (kept opaque outside the module).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaseRule(Case);

/// `LuaDiagnosticStyle`'s name rules.
#[derive(Debug, Clone)]
pub struct NameStyleConfig {
    local: Vec<Rule>,
    param: Vec<Rule>,
    function: Vec<Rule>,
    local_function: Vec<Rule>,
    table_field: Vec<Rule>,
    global: Vec<Rule>,
    module: Vec<Rule>,
    require_module: Vec<Rule>,
    class: Vec<Rule>,
    constant: Vec<Rule>,
    module_local: Vec<Rule>,
}

fn case(c: Case) -> Rule {
    Rule::Case(CaseRule(c))
}

impl Default for NameStyleConfig {
    fn default() -> Self {
        NameStyleConfig {
            local: vec![case(Case::Snake)],
            param: vec![case(Case::Snake)],
            function: vec![case(Case::Snake)],
            local_function: vec![case(Case::Snake)],
            table_field: vec![case(Case::Snake)],
            global: vec![case(Case::Snake), case(Case::UpperSnake)],
            module: vec![case(Case::Snake)],
            require_module: vec![case(Case::Snake), case(Case::Pascal)],
            class: vec![case(Case::Snake), case(Case::Pascal)],
            constant: vec![case(Case::Snake), case(Case::UpperSnake)],
            module_local: vec![case(Case::Snake)],
        }
    }
}

fn case_named(name: &str) -> Option<Case> {
    match name {
        "snake_case" => Some(Case::Snake),
        "upper_snake_case" => Some(Case::UpperSnake),
        "pascal_case" => Some(Case::Pascal),
        "camel_case" => Some(Case::Camel),
        _ => None,
    }
}

/// `MakeNameStyle`.
fn make_rule(v: &serde_json::Value) -> Rule {
    if let Some(s) = v.as_str() {
        return case_named(s).map_or(Rule::Off, case);
    }
    let Some(o) = v.as_object() else { return Rule::Off };
    let Some(kind) = o.get("type").and_then(|t| t.as_str()) else { return Rule::Off };
    if let Some(c) = case_named(kind) {
        return case(c);
    }
    match kind {
        "ignore" => {
            let names = match o.get("param") {
                Some(serde_json::Value::String(s)) => vec![s.clone()],
                Some(serde_json::Value::Array(a)) => a.iter().filter_map(|i| i.as_str().map(str::to_string)).collect(),
                _ => Vec::new(),
            };
            Rule::Ignore(names)
        }
        "pattern" => {
            let Some(source) = o.get("param").and_then(|p| p.as_str()) else { return Rule::Off };
            let Ok(re) = regex::Regex::new(&format!("^(?:{source})$")) else { return Rule::Off };
            let groups = o
                .iter()
                .filter_map(|(k, v)| {
                    let id = k.strip_prefix('$')?.parse::<usize>().ok()?;
                    Some((id, CaseRule(case_named(v.as_str()?)?)))
                })
                .collect();
            Rule::Pattern { source: source.to_string(), re, groups }
        }
        _ => Rule::Off,
    }
}

impl NameStyleConfig {
    /// `LuaDiagnosticStyle::ParseTree` over `Lua.nameStyle.config`: a key that is
    /// set replaces the default; `module_local_name_style` falls back on
    /// `local_name_style` when only the latter is set.
    pub fn parse(v: &serde_json::Value) -> NameStyleConfig {
        let mut c = NameStyleConfig::default();
        let Some(o) = v.as_object() else { return c };
        let set = |key: &str, rules: &mut Vec<Rule>| {
            if let Some(v) = o.get(key).filter(|v| !v.is_null()) {
                *rules = match v.as_array() {
                    Some(a) => a.iter().map(make_rule).collect(),
                    None => vec![make_rule(v)],
                };
            }
        };
        set("local_name_style", &mut c.local);
        set("function_param_name_style", &mut c.param);
        set("function_name_style", &mut c.function);
        set("local_function_name_style", &mut c.local_function);
        set("table_field_name_style", &mut c.table_field);
        set("global_variable_name_style", &mut c.global);
        set("module_name_style", &mut c.module);
        set("require_module_name_style", &mut c.require_module);
        set("class_name_style", &mut c.class);
        set("const_variable_name_style", &mut c.constant);
        set("module_local_name_style", &mut c.module_local);
        if o.get("module_local_name_style").is_none_or(|v| v.is_null()) && o.get("local_name_style").is_some_and(|v| !v.is_null()) {
            c.module_local = c.local.clone();
        }
        c
    }
}

fn snake(s: &str, upper: bool) -> bool {
    // None, Letter, Underscore
    let mut state = 0;
    let class = |b: u8| if upper { b.is_ascii_uppercase() } else { b.is_ascii_lowercase() };
    for b in s.bytes() {
        if b >= 0x80 {
            return false;
        }
        state = match state {
            0 if class(b) => 1,
            0 => return false,
            2 if class(b) || b.is_ascii_digit() => 1,
            2 => return false,
            _ if class(b) || b.is_ascii_digit() => 1,
            _ if b == b'_' => 2,
            _ => return false,
        };
    }
    true
}

fn camel(s: &str, pascal: bool) -> bool {
    // None, Lower, Upper
    let mut state = 0;
    for b in s.bytes() {
        if b >= 0x80 {
            return false;
        }
        state = match state {
            0 if !pascal && b.is_ascii_lowercase() => 1,
            0 if pascal && b.is_ascii_uppercase() => 2,
            0 => return false,
            1 if b.is_ascii_lowercase() || b.is_ascii_digit() => 1,
            1 if b.is_ascii_uppercase() => 2,
            2 if b.is_ascii_uppercase() || b.is_ascii_digit() => 2,
            2 if b.is_ascii_lowercase() => 1,
            _ => return false,
        };
    }
    true
}

fn case_matches(c: CaseRule, s: &str) -> bool {
    match c.0 {
        Case::Snake => snake(s, false),
        Case::UpperSnake => snake(s, true),
        Case::Camel => camel(s, false),
        Case::Pascal => camel(s, true),
    }
}

/// `NameStyleRuleMatcher::Match`: any rule matching is enough; no rules or an
/// empty name always match.
fn matches(rules: &[Rule], name: &str) -> bool {
    if rules.is_empty() || name.is_empty() {
        return true;
    }
    rules.iter().any(|r| match r {
        Rule::Off => false,
        Rule::Case(c) => case_matches(*c, name),
        Rule::Ignore(names) => names.iter().any(|n| n == name),
        Rule::Pattern { re, groups, .. } => match re.captures(name) {
            None => false,
            Some(caps) => groups
                .iter()
                .filter(|(id, _)| *id < caps.len())
                .all(|(id, c)| case_matches(*c, caps.get(*id).map_or("", |m| m.as_str()))),
        },
    })
}

/// `MakeDiagnosticInfo`'s rule text.
fn describe(rules: &[Rule]) -> String {
    let mut out = String::new();
    for (i, r) in rules.iter().enumerate() {
        match r {
            Rule::Case(c) => out.push_str(match c.0 {
                Case::Snake => "snake-case",
                Case::Camel => "camel-case",
                Case::Pascal => "pascal-case",
                Case::UpperSnake => "upper-snake-case",
            }),
            Rule::Pattern { source, .. } => out.push_str(&format!("match '{source}'")),
            Rule::Off | Rule::Ignore(_) => continue,
        }
        if i + 1 < rules.len() {
            out.push_str(" | ");
        }
    }
    out
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Local,
    Module,
    LocalFunction,
    Global,
    Param,
    ImportModule,
    Class,
    Function,
    TableField,
    Const,
    ModuleLocal,
}

const TABLE_FIELD_SPECIAL: &[&str] = &[
    "__add", "__sub", "__mul", "__div", "__mod", "__pow", "__unm", "__idiv", "__band", "__bor", "__bxor", "__bnot", "__shl", "__shr",
    "__concat", "__len", "__eq", "__lt", "__index", "__newindex", "__call", "__gc", "__close", "__mode", "__name",
];

struct Checker {
    scopes: Vec<HashSet<String>>,
    module: Option<String>,
    found: Vec<(Kind, SyntaxToken)>,
}

impl Checker {
    fn record(&mut self, name: &SyntaxToken) {
        if let Some(s) = self.scopes.last_mut() {
            s.insert(name.text().to_string());
        }
    }

    fn is_global(&self, name: &SyntaxToken) -> bool {
        !self.scopes.iter().any(|s| s.contains(name.text()))
    }

    fn push(&mut self, kind: Kind, name: SyntaxToken) {
        self.found.push((kind, name));
    }

    fn params(&mut self, body: Option<SyntaxNode>) {
        let Some(body) = body else { return };
        if let Some(list) = body.children().find(|c| c.kind() == PARAM_LIST) {
            for p in list.children().filter(|c| c.kind() == PARAM) {
                if let Some(name) = name_token(&p) {
                    self.record(&name);
                    self.push(Kind::Param, name);
                }
            }
        }
        if let Some(block) = body.children().find(|c| c.kind() == BLOCK) {
            self.body(&block);
        }
    }

    /// `CheckInNode`: expressions and other non-statement nodes.
    fn node(&mut self, n: &SyntaxNode) {
        for c in n.children() {
            match c.kind() {
                FUNCTION_EXPR => {
                    self.scopes.push(HashSet::new());
                    self.params(c.children().find(|b| b.kind() == FUNC_BODY));
                    self.scopes.pop();
                }
                BLOCK => {
                    self.scopes.push(HashSet::new());
                    self.body(&c);
                    self.scopes.pop();
                }
                _ => self.node(&c),
            }
        }
    }

    /// `CheckSpecialVariableRule`: `x = class(...)` / `x = require(...)`.
    fn special(&mut self, name: &SyntaxToken, expr: &SyntaxNode) -> bool {
        let callee = match expr.kind() {
            CALL_EXPR => match expr.first_child() {
                Some(c) if c.kind() == NAME_EXPR => name_token(&c),
                Some(c) if c.kind() == INDEX_EXPR => super::field_name(&c),
                _ => None,
            },
            METHOD_CALL_EXPR => expr.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == NAME),
            _ => None,
        };
        match callee.as_ref().map(|t| t.text()) {
            Some("class" | "Class") => self.push(Kind::Class, name.clone()),
            Some("require" | "import" | "Import") => self.push(Kind::ImportModule, name.clone()),
            _ => return false,
        }
        true
    }

    /// `CheckInBody`: the statements of a block.
    fn body(&mut self, block: &SyntaxNode) {
        for stmt in block.children() {
            match stmt.kind() {
                LOCAL_STAT => self.local(&stmt),
                ASSIGN_STAT => self.assign(&stmt),
                NUMERIC_FOR_STAT | GENERIC_FOR_STAT => {
                    let names: Vec<SyntaxToken> =
                        stmt.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == NAME).collect();
                    for c in stmt.children().filter(|c| c.kind() != BLOCK) {
                        if c.kind() == EXPR_LIST {
                            for e in c.children() {
                                self.node(&e);
                            }
                        } else {
                            self.node(&c);
                        }
                    }
                    self.scopes.push(HashSet::new());
                    if stmt.kind() == NUMERIC_FOR_STAT {
                        if let Some(i) = names.first() {
                            self.record(i);
                        }
                    } else {
                        for name in names {
                            self.record(&name);
                            self.push(Kind::Local, name);
                        }
                    }
                    if let Some(b) = stmt.children().find(|c| c.kind() == BLOCK) {
                        self.body(&b);
                    }
                    self.scopes.pop();
                }
                FUNCTION_STAT => {
                    let names: Vec<SyntaxToken> = stmt
                        .children()
                        .find(|c| c.kind() == FUNC_NAME)
                        .map(|f| f.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == NAME).collect())
                        .unwrap_or_default();
                    let Some(name) = names.last().cloned() else { continue };
                    self.push(Kind::Function, name.clone());
                    self.scopes.push(HashSet::new());
                    if names.len() == 1 {
                        self.record(&name);
                    }
                    self.params(stmt.children().find(|c| c.kind() == FUNC_BODY));
                    self.scopes.pop();
                }
                LOCAL_FUNCTION_STAT => {
                    let Some(name) = name_token(&stmt) else { continue };
                    self.push(Kind::LocalFunction, name.clone());
                    self.scopes.push(HashSet::new());
                    self.record(&name);
                    self.params(stmt.children().find(|c| c.kind() == FUNC_BODY));
                    self.scopes.pop();
                }
                _ => self.node(&stmt),
            }
        }
    }

    fn local(&mut self, stmt: &SyntaxNode) {
        let exprs: Vec<SyntaxNode> = stmt.children().find(|c| c.kind() == EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default();
        if let Some(list) = stmt.children().find(|c| c.kind() == EXPR_LIST) {
            self.node(&list);
        }
        let locals: Vec<SyntaxNode> = stmt.children().filter(|c| c.kind() == LOCAL_NAME).collect();
        let names: Vec<SyntaxToken> = locals.iter().filter_map(name_token).collect();
        for name in &names {
            self.record(name);
        }
        let top = self.scopes.len() == 1;
        for (i, local) in locals.iter().enumerate() {
            let Some(name) = name_token(local) else { continue };
            if exprs.get(i).is_some_and(|e| self.special(&name, e)) {
                continue;
            }
            if top && self.module.as_deref() == Some(name.text()) {
                self.push(Kind::Module, name);
                break;
            }
            let constant = local.children().find(|c| c.kind() == ATTRIB).is_some_and(|a| {
                a.children_with_tokens().filter_map(|e| e.into_token()).any(|t| t.kind() == NAME && t.text() == "const")
            });
            if constant {
                self.push(Kind::Const, name);
            } else if top {
                self.push(Kind::ModuleLocal, name);
                break;
            } else {
                self.push(Kind::Local, name);
            }
        }
    }

    fn assign(&mut self, stmt: &SyntaxNode) {
        let exprs: Vec<SyntaxNode> = stmt.children().find(|c| c.kind() == EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default();
        if let Some(list) = stmt.children().find(|c| c.kind() == EXPR_LIST) {
            self.node(&list);
        }
        let vars: Vec<SyntaxNode> = stmt.children().find(|c| c.kind() == VAR_LIST).map(|l| l.children().collect()).unwrap_or_default();
        for (i, var) in vars.iter().enumerate() {
            let (name, plain) = match var.kind() {
                NAME_EXPR => (name_token(var), true),
                INDEX_EXPR => (super::field_name(var), false),
                _ => (None, false),
            };
            let Some(name) = name else { continue };
            if exprs.get(i).is_some_and(|e| self.special(&name, e)) {
                continue;
            }
            if plain && self.is_global(&name) {
                self.push(Kind::Global, name);
            } else if !plain {
                self.push(Kind::TableField, name);
            }
        }
    }
}

fn name_token(n: &SyntaxNode) -> Option<SyntaxToken> {
    n.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == NAME)
}

pub(super) fn run(l: &mut Linter) {
    if !l.enabled("name-style-check") {
        return;
    }
    let Some(block) = l.root.children().find(|c| c.kind() == BLOCK) else { return };
    let module = block
        .children()
        .find(|c| c.kind() == RETURN_STAT)
        .and_then(|r| r.children().find(|c| c.kind() == EXPR_LIST))
        .and_then(|e| e.first_child())
        .filter(|e| e.kind() == NAME_EXPR)
        .and_then(|e| name_token(&e))
        .map(|t| t.text().to_string());
    let mut c = Checker { scopes: vec![HashSet::new()], module, found: Vec::new() };
    c.body(&block);
    let config = &l.ctx.config.name_style;
    for (kind, name) in c.found {
        let text = name.text();
        let (rule_name, rules) = match kind {
            Kind::Local if matches!(text, "_" | "_ENV") => continue,
            Kind::Local => ("LocalVariableName", &config.local),
            Kind::Module => ("ModuleName", &config.module),
            Kind::LocalFunction if TABLE_FIELD_SPECIAL.contains(&text) => continue,
            Kind::LocalFunction => ("LocalFunctionName", &config.local_function),
            Kind::Global if matches!(text, "_G" | "_ENV") => continue,
            Kind::Global => ("GlobalVariableDefineName", &config.global),
            Kind::Param => ("ParamName", &config.param),
            Kind::ImportModule => ("ImportModuleName", &config.require_module),
            Kind::Class => ("ClassVariableName", &config.class),
            Kind::Function if TABLE_FIELD_SPECIAL.contains(&text) => continue,
            Kind::Function => ("FunctionDefineName", &config.function),
            Kind::TableField if TABLE_FIELD_SPECIAL.contains(&text) => continue,
            Kind::TableField => ("TableFieldDefineName", &config.table_field),
            Kind::Const => ("ConstVariableName", &config.constant),
            Kind::ModuleLocal => ("ModuleLocalVariableName", &config.module_local),
        };
        if !matches(rules, text) {
            let message = format!("Name '{text}' does not match rule '{rule_name}', which require '{}'", describe(rules));
            l.push("name-style-check", name.text_range(), message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cases() {
        assert!(snake("abc_d1", false) && snake("a_1b", false) && !snake("a__b", false) && !snake("Ab", false) && snake("a_", false));
        assert!(snake("ABC_D", true) && !snake("Abc", true));
        assert!(camel("abcDef", false) && camel("aBC", false) && !camel("Abc", false) && !camel("a_b", false));
        assert!(camel("AbcDef", true) && !camel("abc", true));
    }

    #[test]
    fn config() {
        let c = NameStyleConfig::parse(&serde_json::json!({
            "local_name_style": ["camel_case", { "type": "ignore", "param": ["X_Y"] }],
            "global_variable_name_style": { "type": "pattern", "param": "g_(\\w+)", "$1": "camel_case" },
        }));
        assert!(matches(&c.local, "fooBar") && matches(&c.local, "X_Y") && !matches(&c.local, "foo_bar"));
        // module_local falls back on local_name_style
        assert!(matches(&c.module_local, "fooBar"));
        assert!(matches(&c.global, "g_fooBar") && !matches(&c.global, "g_foo_bar") && !matches(&c.global, "fooBar"));
        assert_eq!(describe(&c.local), "camel-case | ");
        assert_eq!(describe(&c.global), "match 'g_(\\w+)'");
    }
}
