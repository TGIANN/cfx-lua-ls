//! Editor features over the CfxLua syntax tree. Each feature is a plain function
//! of a `Parse` (and the text) that returns `TextRange`-based data; the language
//! server converts it to LSP types.

pub mod api;
pub(crate) mod assist;
mod consts;
mod diagnostics;
mod doc_complete;
mod fields;
mod folding;
mod highlight;
pub mod library;
pub mod lint;
pub mod types;
mod inlay;
pub mod luacats;
mod luapat;
pub mod manifest;
pub mod workspace;
mod names;
mod selection;
mod semantic;
mod symbols;
mod util;

pub use cfx_lua_syntax::{Parse, TextRange, TextSize, parse};
pub use assist::{
    CompleteOptions, Completion, CompletionKind, Completions, EventAt, FileCtx, Hover, ShowWord, Signature, SignatureHelp, api_definition, complete, complete_with, event_at,
    hover,
    hover_with,
    signature_help,
};
pub use diagnostics::{Diagnostic, MAX_DIAGNOSTICS, syntax_diagnostics};
pub use fields::{field_references, field_rename, function_references, rename};
pub use folding::{Fold, FoldKind, folding_ranges};
pub use highlight::{HighlightKind, highlights};
pub use names::{Access, Attrib, Decl, DeclKind, NameRef, Names, Symbol, Target, Value, Vararg, is_name, names};
pub use selection::{MAX_SELECTION_STEPS, selection_ranges};
pub use semantic::{HighlightToken, TOKEN_MODIFIERS, TOKEN_TYPES, TokenType, modifier, semantic_tokens};
pub use symbols::{DocumentSymbol, MAX_TEXT, SymbolKind, document_symbols};
pub use types::{FileTypes, TypeSettings, type_at, type_at_with};
pub use inlay::{InlayHint, InlayKind, MAX_INLAY_HINTS, inlay_hints};

/// FiveM escrow (Asset Escrow) files are encrypted and start with `FXAP`: they are
/// never parsed and get no features.
pub fn is_escrow(text: &str) -> bool {
    text.starts_with("FXAP")
}
