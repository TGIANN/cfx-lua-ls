//! cfx-lua-ls command line: `lsp` runs the language server on stdio, `check`
//! prints diagnostics for folders, `mcp` serves AI agents (Model Context Protocol), `parse` checks the parser against real resources.

mod check;
mod config;
mod lsp;
mod mcp;
mod parse_cmd;

/// Deep expression chains recurse in the parser up to its limit: every thread that
/// parses gets this stack (the workspace scan and `check` run on rayon's pool).
const STACK: usize = 16 << 20;

fn big_stack(f: impl FnOnce() + Send + 'static) {
    let run = std::thread::Builder::new().stack_size(STACK).spawn(f);
    if !matches!(run.map(|h| h.join()), Ok(Ok(()))) {
        std::process::exit(1);
    }
}

fn main() {
    let _ = rayon::ThreadPoolBuilder::new().stack_size(STACK).build_global();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // Clients may append `--stdio`; stdio is the only transport.
        Some("lsp") => lsp::run_stdio(),
        Some("check") => {
            let rest = args[1..].to_vec();
            big_stack(move || check::check_cmd(&rest));
        }
        Some("mcp") => big_stack(mcp::run_stdio),
        Some("parse") if args.len() > 1 => parse_cmd::parse_cmd(&args[1..]),
        _ => {
            eprintln!("usage: cfx-lua-ls lsp | cfx-lua-ls mcp | cfx-lua-ls check <folder-or-file>... | cfx-lua-ls parse <file-or-folder>...");
            std::process::exit(2);
        }
    }
}
