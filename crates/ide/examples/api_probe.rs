//! What the workspace index knows, from the command line:
//!
//! ```sh
//! api_probe <folder> exports <resource> [client|server]
//! api_probe <folder> members <file.lua> <root name> [field ...]
//! api_probe <folder> expr <file.lua> <expression>
//! ```
//!
//! `members` takes a global, or a local of the file by name (`QBCore`, `lib`).

use cfx_lua_ide::api::Ref;
use cfx_lua_ide::manifest::FileSide;
use cfx_lua_ide::workspace::{ApiItem, Workspace};
use std::path::PathBuf;
use std::time::Instant;

fn show(i: &ApiItem) -> String {
    let mut s = i.signature();
    if let Some(t) = &i.ty {
        s = format!("{s}: {t}");
    }
    match i.deprecated() {
        Some(d) => format!("{s}  [deprecated: {d}]"),
        None => s,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: api_probe <folder> exports <resource> [client|server] | members <file> <root> [field ...]");
        std::process::exit(2);
    }
    let t = Instant::now();
    let ws = Workspace::scan(&[PathBuf::from(&args[0])]);
    eprintln!("{} resources scanned in {:?}", ws.resource_count(), t.elapsed());
    match args[1].as_str() {
        "exports" => {
            let side = match args.get(3).map(String::as_str) {
                Some("client") => FileSide::Client,
                Some("server") => FileSide::Server,
                _ => FileSide::Unknown,
            };
            let t = Instant::now();
            let Some(e) = ws.exports_on(&args[2], side) else {
                println!("no resource answers for `{}`", args[2]);
                return;
            };
            println!("`{}` answered by {} ({:?}), complete: {}", args[2], e.resource, t.elapsed(), e.complete);
            for i in &e.items {
                println!("  {} ({:?})", show(i), i.side);
            }
            for i in &e.other_side {
                println!("  other side: {} ({:?})", show(i), i.side);
            }
        }
        "members" if args.len() >= 4 => {
            let path = PathBuf::from(&args[2]);
            let text = String::from_utf8_lossy(&std::fs::read(&path).expect("read the file")).into_owned();
            let p = cfx_lua_ide::parse(&text);
            let n = cfx_lua_ide::names(&p, &text);
            let root = match n.decls.iter().position(|d| d.name == args[3]) {
                Some(i) => Ref::Local(i as u32),
                None => Ref::Global(args[3].clone()),
            };
            let fields = args[4..].to_vec();
            let t = Instant::now();
            let m = ws.api_members(&path, None, &root, &fields);
            println!("{} member(s) of {} {:?} ({:?})", m.len(), args[3], fields, t.elapsed());
            for i in &m {
                println!("  {}", show(i));
            }
        }
        "expr" if args.len() >= 4 => {
            let path = PathBuf::from(&args[2]);
            let text = String::from_utf8_lossy(&std::fs::read(&path).expect("read the file")).into_owned();
            let t = Instant::now();
            let m = ws.members_of_expression(&path, &text, &args[3]).unwrap_or_default();
            println!("{} member(s) of {} ({:?})", m.len(), args[3], t.elapsed());
            for i in &m {
                println!("  {}", show(i));
            }
        }
        _ => eprintln!("unknown command"),
    }
}
