fn main() {
    let src = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let p = cfx_lua_syntax::parse(&src);
    println!("{:#?}", p.syntax());
}
