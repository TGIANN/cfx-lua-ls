//! Prints the syntax tree of a file: `cargo run -p cfx-lua-ide --example tree -- <file.lua>`.

fn main() {
    let path = std::env::args().nth(1).expect("a file");
    let text = std::fs::read_to_string(path).expect("read the file");
    print!("{:#?}", cfx_lua_ide::parse(&text).syntax());
}
