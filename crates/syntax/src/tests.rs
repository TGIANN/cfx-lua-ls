use crate::{SyntaxKind::*, SyntaxNode, parse};

fn ok(src: &str) -> SyntaxNode {
    let p = parse(src);
    assert!(p.errors().is_empty(), "unexpected errors for {src:?}: {:?}\n{}", p.errors(), p.debug_tree());
    let root = p.syntax();
    assert_eq!(root.text().to_string(), src, "tree is not lossless");
    root
}

fn err(src: &str) {
    let p = parse(src);
    assert!(!p.errors().is_empty(), "expected an error for {src:?}\n{}", p.debug_tree());
    assert_eq!(p.syntax().text().to_string(), src, "tree is not lossless");
}

fn has(root: &SyntaxNode, kind: crate::SyntaxKind) -> bool {
    root.descendants().any(|n| n.kind() == kind)
}

#[test]
fn lua54_statements() {
    ok("local a <const>, b <close> = 1, nil");
    ok("local function f(a, b, ...) return a, b end");
    ok("function M.a.b:c(x) self.x = x end");
    ok("for i = 1, 10, 2 do print(i) end");
    ok("for k, v in pairs(t) do goto continue ::continue:: end");
    ok("while x do break end repeat x = x - 1 until x == 0");
    ok("if a then elseif b then else end");
    ok("do local x = 1 end ; ;");
    ok("return");
    ok("return 1;");
    ok("x, y.z, w[1] = 1, 2, 3");
    ok("f{1, 2; x = 3, [4] = 5,} f'str' f[[long]] obj:m'x' (f)()");
    ok("local x = a // b ~ c & d | e << 1 >> 2 .. 'x' .. 'y' ^ 2 ^ 3");
    ok("local x = not a == b and -c or #d ~= ~e");
    ok("local f = function(...) local t = {...} return select('#', ...) end");
    ok("#!/usr/bin/lua\nprint(1)");
    ok("local s = 'a\\z\n  b' .. \"\\x41\\u{48}\\065\"");
}

#[test]
fn cfxlua_extensions() {
    let root = ok("a += 1 b.c -= 2 d[1] *= 3 e /= 4 f <<= 1 g >>= 1 h &= 1 i |= 1 j ^= 1");
    assert_eq!(root.descendants().filter(|n| n.kind() == COMPOUND_ASSIGN_STAT).count(), 9);

    let root = ok("local a, b in t");
    assert!(has(&root, LOCAL_STAT));
    ok("a, b.c in GetSomething()");

    let root = ok("local t = { .a, .b = 2, c = 3, 4 }");
    assert_eq!(root.descendants().filter(|n| n.kind() == FIELD_SET).count(), 2);

    ok("local x = a?.b?.c local y = a?['k']?[1] if p?.x then end");
    ok("RequestModel(`adder`) if GetEntityModel(v) == `buzzard` then end");
    ok("print('x') /* comment \n over lines */ print('y')");
}

#[test]
fn syntax_errors() {
    err("local = 1");
    err("x = ");
    err("if a then");
    err("f(");
    err("a, b += 1");
    err("a?b");
    err("1 + 2");
    err("x = 'unfinished");
    err("end");
    err("return 1 x = 2");
    err("local t = { a = 1 b = 2 }");
    err("x = 1e");
    err("/* never closed");
}

#[test]
fn error_recovery_keeps_following_statements() {
    let p = parse("local = 1\nlocal ok = 2\nfunction f() end");
    let root = p.syntax();
    assert!(has(&root, FUNCTION_STAT));
    assert_eq!(root.descendants().filter(|n| n.kind() == LOCAL_STAT).count(), 2);
}

#[test]
fn doc_comments_attach_to_declaration() {
    let src = "local a = 1\n\n---@class Foo\n---@field x number\nlocal Foo = {}\n";
    let root = ok(src);
    let foo = root.descendants().filter(|n| n.kind() == LOCAL_STAT).nth(1).unwrap();
    assert!(foo.text().to_string().starts_with("---@class Foo"), "{:?}", foo.text());

    // A blank line breaks the attachment.
    let root = ok("---@type number\n\nlocal x = 1");
    let x = root.descendants().find(|n| n.kind() == LOCAL_STAT).unwrap();
    assert!(x.text().to_string().starts_with("local"));

    // Inside a function body, and on table fields.
    let root =
        ok("function f()\n  ---@type string\n  local s = g()\nend\nlocal t = {\n  --- the name\n  name = 'x',\n}");
    let s = root.descendants().find(|n| n.kind() == LOCAL_STAT).unwrap();
    assert!(s.text().to_string().starts_with("---@type string"));
    let field = root.descendants().find(|n| n.kind() == FIELD_NAMED).unwrap();
    assert!(field.text().to_string().starts_with("--- the name"));
}

#[test]
fn deep_nesting_is_an_error_not_a_crash() {
    let src = format!("x = {}1{}", "(".repeat(20_000), ")".repeat(20_000));
    let p = parse(&src);
    assert!(!p.errors().is_empty());
    assert_eq!(p.syntax().text().to_string(), src);
}

#[test]
fn same_line_comments_stay_with_what_came_before() {
    let root = ok("local a = 1 -- note\nlocal b = 2");
    let b = root.descendants().filter(|n| n.kind() == LOCAL_STAT).nth(1).unwrap();
    assert!(b.text().to_string().starts_with("local b"), "{:?}", b.text());

    let root = ok("local t = {\n  a = 1, -- first\n  b = 2, -- second\n  --- doc for c\n  c = 3,\n}");
    let fields: Vec<String> =
        root.descendants().filter(|n| n.kind() == FIELD_NAMED).map(|n| n.text().to_string()).collect();
    assert_eq!(fields, ["a = 1", "b = 2", "--- doc for c\n  c = 3"]);

    // A comment as the very first thing in the file still documents the first statement.
    let root = ok("---@class A\nlocal A = {}");
    assert!(root.descendants().find(|n| n.kind() == LOCAL_STAT).unwrap().text().to_string().starts_with("---@class"));
}

#[test]
fn bom_is_trivia() {
    let root = ok("\u{feff}---@type number\nlocal x = 1");
    assert!(root.descendants().find(|n| n.kind() == LOCAL_STAT).unwrap().text().to_string().starts_with("---@type"));
    ok("\u{feff}#!/bin/lua\nprint(1)");
}

#[test]
fn set_constructor_with_index_key() {
    let root = ok("local t = { [k], [1] = 2, .a }");
    assert_eq!(root.descendants().filter(|n| n.kind() == FIELD_INDEXED).count(), 2);
}

/// Render the first expression of `x = <expr>` as an s-expression.
fn shape(src: &str) -> String {
    fn render(n: &SyntaxNode) -> String {
        let kids: Vec<SyntaxNode> = n.children().collect();
        let op = || {
            n.children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| !t.kind().is_trivia())
                .map(|t| t.text().to_string())
                .unwrap_or_default()
        };
        match n.kind() {
            BINARY_EXPR | UNARY_EXPR => {
                format!("({} {})", op(), kids.iter().map(render).collect::<Vec<_>>().join(" "))
            }
            INDEX_EXPR | CALL_EXPR | METHOD_CALL_EXPR | ARG_LIST | PAREN_EXPR => {
                let mut parts = vec![format!("{:?}", n.kind()).to_lowercase()];
                for e in n.children_with_tokens() {
                    match e {
                        rowan::NodeOrToken::Node(c) => parts.push(render(&c)),
                        rowan::NodeOrToken::Token(t) if t.kind() == NAME => parts.push(t.text().to_string()),
                        _ => {}
                    }
                }
                format!("({})", parts.join(" "))
            }
            _ => n.text().to_string().trim().to_string(),
        }
    }
    let root = ok(&format!("x = {src}"));
    let list = root.descendants().find(|n| n.kind() == EXPR_LIST).unwrap();
    render(&list.first_child().unwrap())
}

#[test]
fn expression_shapes() {
    assert_eq!(shape("-x^2"), "(- (^ x 2))");
    assert_eq!(shape("2^-3"), "(^ 2 (- 3))");
    assert_eq!(shape("2^3^4"), "(^ 2 (^ 3 4))");
    assert_eq!(shape("a..b..c"), "(.. a (.. b c))");
    assert_eq!(shape("not a == b"), "(== (not a) b)");
    assert_eq!(shape("a or b and c"), "(or a (and b c))");
    assert_eq!(shape("a + b * c - d"), "(- (+ a (* b c)) d)");
    assert_eq!(shape("a < b == c"), "(== (< a b) c)");
    assert_eq!(shape("1 | 2 ~ 3 & 4 << 5"), "(| 1 (~ 2 (& 3 (<< 4 5))))");
    assert_eq!(shape("#t + 1"), "(+ (# t) 1)");
    assert_eq!(shape("a.b[c]:d(e)"), "(method_call_expr (index_expr (index_expr a b) c) d (arg_list e))");
    assert_eq!(shape("f(a)(b)"), "(call_expr (call_expr f (arg_list a)) (arg_list b))");
    assert_eq!(shape("a?.b?[1]"), "(index_expr (index_expr a b) 1)");
    assert_eq!(shape("(a).b"), "(index_expr (paren_expr a) b)");
}

/// Codes of the errors `src` produces, in source order.
fn codes(src: &str) -> Vec<&'static str> {
    parse(src).errors().iter().map(|e| e.code).collect()
}

#[test]
fn compile_time_checks() {
    // goto into a local's scope: `::continue::` before `until` is not the block's
    // last statement, so `y` is in scope there (a classic Lua 5.4 error).
    assert_eq!(codes("repeat\n if x then goto continue end\n local y = 1\n ::continue::\nuntil y"), [
        "JUMP_LOCAL_SCOPE"
    ]);
    ok("while x do\n if y then goto continue end\n local z = 1\n ::continue::\nend");
    ok("local x = 1 ::top:: local y = 2 if y then goto top end"); // backward jump
    ok("do goto out end ::out::");
    assert_eq!(codes("goto a do ::a:: end"), ["NO_VISIBLE_LABEL"]);
    assert_eq!(codes("::a:: do ::a:: end"), ["REDEFINED_LABEL"]);
    assert_eq!(codes("function f() ::a:: end ::a::"), Vec::<&str>::new());
    assert_eq!(codes("::a:: function f() goto a end"), ["NO_VISIBLE_LABEL"]); // functions are a boundary

    // const / close
    ok("local x <const> = 1 local x = 2 x = 3"); // shadowed
    ok("local x <const> = x"); // the right side sees the global
    assert_eq!(codes("local x <const> = 1 function f() x = 2 end"), ["SET_CONST"]); // upvalue
    assert_eq!(codes("local x <const> = 1 x += 1"), ["SET_CONST"]);
    assert_eq!(codes("local x <const> = 1 x, y in t"), ["SET_CONST"]);
    ok("local x <const> = 1 t.x = 2 function t.x() end");
    assert_eq!(codes("local a <close>, b <close> = nil, nil"), ["MULTI_CLOSE"]);
    ok("local function f() f = nil end");
    ok("local self <const> = 1 function t:m() self = 2 end"); // the method's own `self`

    // break and varargs
    ok("for i = 1, 2 do if i then break end end");
    assert_eq!(codes("while true do local f = function() break end end"), ["BREAK_OUTSIDE"]);
    ok("local t = {...}"); // the main chunk is vararg
    assert_eq!(codes("local function f(a) return ... end"), ["UNEXPECT_DOTS"]);
    ok("function M:m(...) return self, ... end");
}

#[test]
fn local_limit_counts_active_locals() {
    let names = |n: usize| (1..=n).map(|i| format!("l{i}")).collect::<Vec<_>>().join(", ");
    // 200 active is fine; locals of a closed block are gone.
    ok(&format!("local {}\ndo local a end", names(199)));
    ok(&format!("local {}\n", names(200)));
    assert_eq!(codes(&format!("local {}\n", names(201))), ["LOCAL_LIMIT"]);
    // `self` is a local of a method.
    assert_eq!(codes(&format!("function t:m({}) end", names(200))), ["LOCAL_LIMIT"]);
}

#[test]
fn deep_expression_chains_do_not_overflow() {
    // Valid Lua (left-nested, no parser depth limit). `Parse` must be the last
    // owner of the tree: it frees deep trees without recursion.
    for src in [format!("x = a{}", ".b".repeat(20_000)), format!("x = a{}", " + a".repeat(20_000))] {
        let p = parse(&src);
        assert!(p.errors().is_empty());
        assert_eq!(p.syntax().text().len(), rowan::TextSize::of(src.as_str()));
    }
}

#[test]
fn syntax_shapes_from_other_lua_versions() {
    assert_eq!(codes("local <const> x = 1"), ["UNSUPPORT_SYMBOL"]);
    assert_eq!(codes("function f(...args) end"), ["UNSUPPORT_NAMED_VARARG"]);
    assert_eq!(codes("print(('x'):rep(2)) print('x':rep(2))"), ["NEED_PAREN"]);
    assert_eq!(codes("local x <const>= 1"), ["MISS_SPACE_BETWEEN"]);
    err("function t[1]() end");
    assert_eq!(codes("s ..= 'x'"), ["UNSUPPORT_SYMBOL"]);
}
