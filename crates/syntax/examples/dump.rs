//! Prints the syntax tree of a file: `cargo run -p cfx-lua-syntax --example dump -- file.lua`.
fn main() {
    let path = std::env::args().nth(1).expect("a file");
    let text = std::fs::read_to_string(path).expect("readable file");
    let parse = cfx_lua_syntax::parse(&text);
    print!("{}", parse.debug_tree());
    for e in parse.errors() {
        println!("error {:?} {}: {}", e.range, e.code, e.message);
    }
}
