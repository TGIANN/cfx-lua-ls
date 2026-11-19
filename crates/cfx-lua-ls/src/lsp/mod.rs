//! The language server: stdio transport, document sync, syntax diagnostics,
//! document symbols, folding ranges, selection ranges, semantic tokens,
//! formatting, definition, references, document highlight, rename, hover,
//! completion, signature help, workspace symbols, lint
//! diagnostics with quick fixes over the workspace's resources.
//! Only protocol messages go to stdout; logs go to stderr.

mod convert;
mod document;
mod format;
mod semantic;

use convert::Encoding;
use document::Document;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, Completion, DocumentHighlightRequest, HoverRequest, SignatureHelpRequest, DocumentSymbolRequest, FoldingRangeRequest, Formatting, GotoDefinition,
    PrepareRenameRequest, RangeFormatting, References, Rename, Request as _, SelectionRangeRequest,
    InlayHintRequest, SemanticTokensFullRequest, WorkspaceSymbolRequest,
};
use lsp_types::{
    DiagnosticSeverity, DocumentFormattingParams, DocumentRangeFormattingParams, DocumentSymbolParams, DocumentSymbolResponse, FoldingRange, FoldingRangeKind,
    FoldingRangeParams, FoldingRangeProviderCapability, InitializeParams, InitializeResult,
    NumberOrString, OneOf, PositionEncodingKind, PublishDiagnosticsParams, SelectionRange, SelectionRangeParams,
    SelectionRangeProviderCapability, SemanticTokens, SemanticTokensFullOptions, SemanticTokensOptions,
    SemanticTokensParams, SemanticTokensResult, SemanticTokensServerCapabilities, ServerCapabilities, ServerInfo, SymbolKind,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions, TextEdit, Uri,
};
use cfx_lua_ide::lint::{Lint, Severity, Tag};
use cfx_lua_ide::workspace::Workspace;
use std::collections::HashMap;
use std::path::PathBuf;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// LuaLS's `source` for syntax errors, so both servers' diagnostics look alike.
const SYNTAX_SOURCE: &str = "Lua Syntax Check.";
const LINT_SOURCE: &str = "cfx-lua-ls";

pub(crate) fn run_stdio() {
    // Deep expression chains recurse in the parser up to its limit; give the
    // server the same stack as `parse`.
    let server = std::thread::Builder::new().name("lsp".into()).stack_size(16 << 20).spawn(|| {
        let (connection, io_threads) = Connection::stdio();
        let result = serve(&connection);
        drop(connection);
        io_threads.join().map_err(Error::from).and(result)
    });
    match server.map(|h| h.join()) {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => {
            eprintln!("cfx-lua-ls: {e}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("cfx-lua-ls: the server thread failed");
            std::process::exit(1);
        }
    }
}

pub(crate) fn serve(connection: &Connection) -> Result<(), Error> {
    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let encoding = negotiate_encoding(&params);
    let result = InitializeResult {
        capabilities: capabilities(encoding),
        server_info: Some(ServerInfo { name: "cfx-lua-ls".into(), version: Some(env!("CARGO_PKG_VERSION").into()) }),
    };
    connection.initialize_finish(id, serde_json::to_value(result)?)?;

    let workspace = workspace_folders(&params);
    let started = std::time::Instant::now();
    let ws = Workspace::scan(&workspace);
    eprintln!("cfx-lua-ls: {} resources read in {:.2?}", ws.resource_count(), started.elapsed());
    let diagnostics = crate::config::load(&workspace, None);
    let mut server = Server {
        connection,
        encoding,
        docs: HashMap::new(),
        workspace,
        format: format::Settings::default(),
        ws,
        diagnostics,
        client_settings: None,
        inlay_refresh: params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.inlay_hint.as_ref())
            .and_then(|h| h.refresh_support)
            .unwrap_or(false),
        next_id: 0,
    };
    for message in &connection.receiver {
        match message {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                server.request(req)?;
            }
            Message::Notification(n) => server.notification(n)?,
            Message::Response(_) => {}
        }
    }
    Ok(())
}

/// UTF-8 when the client offers it (no conversion), else UTF-16 (the LSP default).
fn negotiate_encoding(params: &InitializeParams) -> Encoding {
    let offered = params.capabilities.general.as_ref().and_then(|g| g.position_encodings.as_ref());
    if offered.is_some_and(|list| list.contains(&PositionEncodingKind::UTF8)) { Encoding::Utf8 } else { Encoding::Utf16 }
}

#[allow(deprecated)] // `root_uri`: the fallback for clients without workspace folders.
fn workspace_folders(params: &InitializeParams) -> Vec<PathBuf> {
    match &params.workspace_folders {
        Some(folders) => folders.iter().filter_map(|f| format::file_path(&f.uri)).collect(),
        None => params.root_uri.iter().filter_map(format::file_path).collect(),
    }
}

fn capabilities(encoding: Encoding) -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(match encoding {
            Encoding::Utf8 => PositionEncodingKind::UTF8,
            Encoding::Utf16 => PositionEncodingKind::UTF16,
        }),
        text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::INCREMENTAL),
            ..Default::default()
        })),
        document_symbol_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
            legend: semantic::legend(),
            full: Some(SemanticTokensFullOptions::Bool(true)),
            ..Default::default()
        })),
        definition_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(lsp_types::RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_range_formatting_provider: Some(OneOf::Left(true)),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        completion_provider: Some(lsp_types::CompletionOptions {
            // `(`, `,`, `{` and a space offer only the literal values and table
            // keys the position takes (LuaLS); `@` and `|` start doc tags and
            // `---|` values.
            trigger_characters: Some([".", ":", "'", "\"", "(", ",", "{", " ", "@", "|"].map(String::from).to_vec()),
            ..Default::default()
        }),
        signature_help_provider: Some(lsp_types::SignatureHelpOptions {
            trigger_characters: Some(vec!["(".into(), ",".into()]),
            ..Default::default()
        }),
        inlay_hint_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(lsp_types::CodeActionProviderCapability::Options(lsp_types::CodeActionOptions {
            code_action_kinds: Some(vec![lsp_types::CodeActionKind::QUICKFIX]),
            ..Default::default()
        })),
        ..Default::default()
    }
}

struct Server<'c> {
    connection: &'c Connection,
    encoding: Encoding,
    /// Keyed by the URI as the client sent it.
    docs: HashMap<Uri, Document>,
    /// Workspace folders: the search for `.editorconfig` stops at them.
    workspace: Vec<PathBuf>,
    format: format::Settings,
    /// Resources, sides and the globals of every file, for lint.
    ws: Workspace,
    diagnostics: crate::config::Diagnostics,
    /// The client's `Lua` section, applied over `.luarc.json`.
    client_settings: Option<serde_json::Value>,
    /// The client asks for inlay hints again on `workspace/inlayHint/refresh`.
    inlay_refresh: bool,
    /// Ids of the requests the server sends.
    next_id: i32,
}

impl Server<'_> {
    fn request(&mut self, req: Request) -> Result<(), Error> {
        let response = match req.method.as_str() {
            DocumentSymbolRequest::METHOD => match serde_json::from_value::<DocumentSymbolParams>(req.params) {
                Ok(p) => Response::new_ok(req.id, self.document_symbols(&p.text_document.uri)),
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            FoldingRangeRequest::METHOD => match serde_json::from_value::<FoldingRangeParams>(req.params) {
                Ok(p) => Response::new_ok(req.id, self.folding_ranges(&p.text_document.uri)),
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            SelectionRangeRequest::METHOD => match serde_json::from_value::<SelectionRangeParams>(req.params) {
                Ok(p) => Response::new_ok(req.id, self.selection_ranges(&p.text_document.uri, &p.positions)),
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            SemanticTokensFullRequest::METHOD => match serde_json::from_value::<SemanticTokensParams>(req.params) {
                Ok(p) => Response::new_ok(req.id, self.semantic_tokens(&p.text_document.uri)),
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            Formatting::METHOD => match serde_json::from_value::<DocumentFormattingParams>(req.params) {
                Ok(p) => Response::new_ok(req.id, self.formatting(&p)),
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            RangeFormatting::METHOD => match serde_json::from_value::<DocumentRangeFormattingParams>(req.params) {
                Ok(p) => Response::new_ok(req.id, self.range_formatting(&p)),
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            GotoDefinition::METHOD => match serde_json::from_value::<lsp_types::GotoDefinitionParams>(req.params) {
                Ok(p) => Response::new_ok(req.id, self.definition(&p.text_document_position_params)),
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            References::METHOD => match serde_json::from_value::<lsp_types::ReferenceParams>(req.params) {
                Ok(p) => Response::new_ok(req.id, self.references(&p)),
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            DocumentHighlightRequest::METHOD => {
                match serde_json::from_value::<lsp_types::DocumentHighlightParams>(req.params) {
                    Ok(p) => Response::new_ok(req.id, self.document_highlight(&p.text_document_position_params)),
                    Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
                }
            }
            PrepareRenameRequest::METHOD => {
                match serde_json::from_value::<lsp_types::TextDocumentPositionParams>(req.params) {
                    Ok(p) => Response::new_ok(req.id, self.prepare_rename(&p)),
                    Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
                }
            }
            HoverRequest::METHOD => match serde_json::from_value::<lsp_types::HoverParams>(req.params) {
                Ok(p) => Response::new_ok(req.id, self.hover(&p.text_document_position_params)),
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            Completion::METHOD => match serde_json::from_value::<lsp_types::CompletionParams>(req.params) {
                Ok(p) => {
                    let trigger = p.context.as_ref().and_then(|c| c.trigger_character.as_deref()).and_then(|t| t.chars().next());
                    Response::new_ok(req.id, self.completion(&p.text_document_position, trigger))
                }
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            SignatureHelpRequest::METHOD => match serde_json::from_value::<lsp_types::SignatureHelpParams>(req.params) {
                Ok(p) => Response::new_ok(req.id, self.signature_help(&p.text_document_position_params)),
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            InlayHintRequest::METHOD => match serde_json::from_value::<lsp_types::InlayHintParams>(req.params) {
                Ok(p) => Response::new_ok(req.id, self.inlay_hints(&p)),
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            WorkspaceSymbolRequest::METHOD => match serde_json::from_value::<lsp_types::WorkspaceSymbolParams>(req.params) {
                Ok(p) => Response::new_ok(req.id, self.workspace_symbols(&p.query)),
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            CodeActionRequest::METHOD => match serde_json::from_value::<lsp_types::CodeActionParams>(req.params) {
                Ok(p) => Response::new_ok(req.id, self.code_actions(&p)),
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            Rename::METHOD => match serde_json::from_value::<lsp_types::RenameParams>(req.params) {
                Ok(p) => match self.rename(&p) {
                    Ok(edit) => Response::new_ok(req.id, edit),
                    Err(msg) => Response::new_err(req.id, ErrorCode::RequestFailed as i32, msg),
                },
                Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
            },
            _ => {
                let msg = format!("unhandled method {}", req.method);
                Response::new_err(req.id, ErrorCode::MethodNotFound as i32, msg)
            }
        };
        self.connection.sender.send(Message::Response(response))?;
        Ok(())
    }

    /// `null` for a file the client has not opened or an escrow file.
    fn document_symbols(&self, uri: &Uri) -> Option<DocumentSymbolResponse> {
        let doc = self.docs.get(uri)?;
        let parse = doc.parse.as_ref()?;
        let symbols = cfx_lua_ide::document_symbols(parse, &doc.text);
        Some(DocumentSymbolResponse::Nested(symbols.into_iter().map(|s| self.symbol(doc, s)).collect()))
    }

    /// Line ranges only: every client supports them (`lineFoldingOnly`).
    fn folding_ranges(&self, uri: &Uri) -> Option<Vec<FoldingRange>> {
        let doc = self.docs.get(uri)?;
        let parse = doc.parse.as_ref()?;
        let line_of = |offset: cfx_lua_ide::TextSize| doc.index.line_col(offset.min(doc.index.len())).line;
        let ranges = cfx_lua_ide::folding_ranges(parse, &doc.text)
            .into_iter()
            .filter_map(|f| {
                let (start_line, end_line) = f.lines(line_of)?;
                let kind = match f.kind {
                    cfx_lua_ide::FoldKind::Region => FoldingRangeKind::Region,
                    cfx_lua_ide::FoldKind::Comment => FoldingRangeKind::Comment,
                };
                Some(FoldingRange { start_line, end_line, kind: Some(kind), ..Default::default() })
            })
            .collect();
        Some(ranges)
    }

    fn semantic_tokens(&self, uri: &Uri) -> Option<SemanticTokensResult> {
        let doc = self.docs.get(uri)?;
        let parse = doc.parse.as_ref()?;
        let tokens = cfx_lua_ide::semantic_tokens(parse);
        let data = semantic::encode(&doc.index, &doc.text, &tokens, self.encoding);
        Some(SemanticTokensResult::Tokens(SemanticTokens { result_id: None, data }))
    }

    /// One chain per position, in the order asked. A position in an empty file
    /// gets the empty range at its start.
    fn selection_ranges(&self, uri: &Uri, positions: &[lsp_types::Position]) -> Option<Vec<SelectionRange>> {
        let doc = self.docs.get(uri)?;
        let parse = doc.parse.as_ref()?;
        let chains = positions.iter().map(|&pos| {
            let offset = convert::offset(&doc.index, &doc.text, pos, self.encoding);
            let steps = cfx_lua_ide::selection_ranges(parse, offset);
            // Built from the outside in, so no step recurses.
            let mut outer: Option<SelectionRange> = None;
            for &r in steps.iter().rev() {
                let range = convert::range(&doc.index, r, self.encoding);
                outer = Some(SelectionRange { range, parent: outer.map(Box::new) });
            }
            outer.unwrap_or(SelectionRange { range: lsp_types::Range::new(pos, pos), parent: None })
        });
        Some(chains.collect())
    }

    /// The document and offset of a position, when the document has a tree.
    fn at(&self, p: &lsp_types::TextDocumentPositionParams) -> Option<(&Document, cfx_lua_ide::TextSize)> {
        let doc = self.docs.get(&p.text_document.uri)?;
        doc.parse.as_ref()?;
        Some((doc, convert::offset(&doc.index, &doc.text, p.position, self.encoding)))
    }

    fn locations(&self, uri: &Uri, doc: &Document, ranges: Vec<cfx_lua_ide::TextRange>) -> Vec<lsp_types::Location> {
        ranges
            .into_iter()
            .map(|r| lsp_types::Location { uri: uri.clone(), range: convert::range(&doc.index, r, self.encoding) })
            .collect()
    }

    /// What the editor features know about a document beyond its text.
    fn ide_context<'a>(&'a self, uri: &Uri, path: Option<&'a std::path::Path>) -> cfx_lua_ide::FileCtx<'a> {
        let (game, side) = self.file_context(uri);
        cfx_lua_ide::FileCtx { game, side, ws: path.map(|p| (&self.ws, p)) }
    }

    /// The game and side of a document (GTA V and unknown outside resources).
    fn file_context(&self, uri: &Uri) -> (cfx_lua_ide::library::Game, cfx_lua_ide::manifest::FileSide) {
        match format::file_path(uri) {
            Some(path) => {
                let info = self.ws.info(&path);
                (info.game, info.side)
            }
            None => (cfx_lua_ide::library::Game::Gta5, cfx_lua_ide::manifest::FileSide::Unknown),
        }
    }

    /// Off unless the client sets `Lua.hint.enable`, as in LuaLS;
    /// `Lua.hint.paramName: "Disable"` leaves out parameter names.
    fn inlay_hints(&self, p: &lsp_types::InlayHintParams) -> Option<Vec<lsp_types::InlayHint>> {
        let hint = self.client_settings.as_ref().and_then(|s| s.get("hint"));
        if !hint.and_then(|h| h.get("enable")).and_then(|e| e.as_bool()).unwrap_or(false) {
            return Some(Vec::new());
        }
        let names_on = hint.and_then(|h| h.get("paramName")).and_then(|v| v.as_str()) != Some("Disable");
        let doc = self.docs.get(&p.text_document.uri)?;
        let parse = doc.parse.as_ref()?;
        let start = convert::offset(&doc.index, &doc.text, p.range.start, self.encoding);
        let end = convert::offset(&doc.index, &doc.text, p.range.end, self.encoding);
        let path = format::file_path(&p.text_document.uri);
        let cx = self.ide_context(&p.text_document.uri, path.as_deref());
        let hints = cfx_lua_ide::inlay_hints(parse, doc.names()?, &doc.text, cfx_lua_ide::TextRange::new(start, end.max(start)), cx);
        Some(
            hints
                .into_iter()
                .filter(|h| names_on || h.kind != cfx_lua_ide::InlayKind::Parameter)
                .map(|h| {
                    let param = h.kind == cfx_lua_ide::InlayKind::Parameter;
                    lsp_types::InlayHint {
                        position: convert::range(&doc.index, cfx_lua_ide::TextRange::empty(h.offset), self.encoding).start,
                        label: lsp_types::InlayHintLabel::String(h.label),
                        kind: Some(if param { lsp_types::InlayHintKind::PARAMETER } else { lsp_types::InlayHintKind::TYPE }),
                        text_edits: None,
                        tooltip: None,
                        padding_left: None,
                        padding_right: Some(param),
                        data: None,
                    }
                })
                .collect(),
        )
    }

    fn hover(&self, p: &lsp_types::TextDocumentPositionParams) -> Option<lsp_types::Hover> {
        let (doc, offset) = self.at(p)?;
        let path = format::file_path(&p.text_document.uri);
        let cx = self.ide_context(&p.text_document.uri, path.as_deref());
        let h = cfx_lua_ide::hover(doc.parse.as_ref()?, doc.names()?, &doc.text, offset, cx)?;
        Some(lsp_types::Hover {
            contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
                kind: lsp_types::MarkupKind::Markdown,
                value: h.markdown,
            }),
            range: Some(convert::range(&doc.index, h.range, self.encoding)),
        })
    }

    fn completion(&self, p: &lsp_types::TextDocumentPositionParams, trigger: Option<char>) -> Option<lsp_types::CompletionResponse> {
        let (doc, offset) = self.at(p)?;
        let path = format::file_path(&p.text_document.uri);
        let cx = self.ide_context(&p.text_document.uri, path.as_deref());
        if let Some(ev) = cfx_lua_ide::event_at(doc.parse.as_ref()?, &doc.text, offset) {
            return Some(self.event_completion(doc, &ev, offset));
        }
        let opts = cfx_lua_ide::CompleteOptions { trigger, show_word: cfx_lua_ide::ShowWord::Fallback };
        let c = cfx_lua_ide::complete_with(doc.parse.as_ref()?, doc.names()?, &doc.text, offset, cx, opts)?;
        use cfx_lua_ide::CompletionKind as K;
        let items = c
            .items
            .into_iter()
            .map(|i| lsp_types::CompletionItem {
                kind: Some(match i.kind {
                    K::Keyword => lsp_types::CompletionItemKind::KEYWORD,
                    K::Local => lsp_types::CompletionItemKind::VARIABLE,
                    K::Function => lsp_types::CompletionItemKind::FUNCTION,
                    // LuaLS shows globals as fields of `_G`.
                    K::Global => lsp_types::CompletionItemKind::FIELD,
                    K::Field => lsp_types::CompletionItemKind::FIELD,
                    K::Class => lsp_types::CompletionItemKind::CLASS,
                    K::Enum => lsp_types::CompletionItemKind::ENUM,
                    K::EnumMember => lsp_types::CompletionItemKind::ENUM_MEMBER,
                    K::Property => lsp_types::CompletionItemKind::PROPERTY,
                    K::Interface => lsp_types::CompletionItemKind::INTERFACE,
                    K::Event => lsp_types::CompletionItemKind::EVENT,
                    K::Snippet => lsp_types::CompletionItemKind::SNIPPET,
                    K::Value => lsp_types::CompletionItemKind::VALUE,
                    K::Operator => lsp_types::CompletionItemKind::OPERATOR,
                    K::Text => lsp_types::CompletionItemKind::TEXT,
                }),
                insert_text: i.insert.clone(),
                additional_text_edits: (!i.edits.is_empty()).then(|| {
                    i.edits.iter().map(|(r, t)| lsp_types::TextEdit { range: convert::range(&doc.index, *r, self.encoding), new_text: t.clone() }).collect()
                }),
                text_edit: i.replace.map(|r| {
                    lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit { range: convert::range(&doc.index, r, self.encoding), new_text: i.label.clone() })
                }),
                label: i.label,
                detail: i.detail,
                tags: i.deprecated.then(|| vec![lsp_types::CompletionItemTag::DEPRECATED]),
                ..Default::default()
            })
            .collect();
        Some(lsp_types::CompletionResponse::List(lsp_types::CompletionList { is_incomplete: c.incomplete, items }))
    }

    /// Event names inside an event call's string: names with handlers first
    /// for a trigger, names something triggers first for a handler.
    fn event_completion(&self, doc: &Document, ev: &cfx_lua_ide::EventAt, offset: cfx_lua_ide::TextSize) -> lsp_types::CompletionResponse {
        const MAX: usize = 300;
        let typed = doc.text[usize::from(ev.content.start())..usize::from(offset)].to_ascii_lowercase();
        let mut names: Vec<(String, usize, usize)> =
            self.ws.event_names().into_iter().filter(|(n, _, _)| n.to_ascii_lowercase().contains(&typed) && *n != ev.name).collect();
        names.sort_by_key(|(n, h, t)| (if ev.handler { *t == 0 } else { *h == 0 }, !n.to_ascii_lowercase().starts_with(&typed)));
        let incomplete = names.len() > MAX;
        let range = convert::range(&doc.index, ev.content, self.encoding);
        let items = names
            .into_iter()
            .take(MAX)
            .enumerate()
            .map(|(i, (n, h, t))| lsp_types::CompletionItem {
                label: n.clone(),
                kind: Some(lsp_types::CompletionItemKind::EVENT),
                detail: Some(format!("{h} handler(s), {t} trigger(s)")),
                filter_text: Some(n.clone()),
                sort_text: Some(format!("{i:05}")),
                text_edit: Some(lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit { range, new_text: n })),
                ..Default::default()
            })
            .collect();
        lsp_types::CompletionResponse::List(lsp_types::CompletionList { is_incomplete: incomplete, items })
    }

    /// Places in other files, as locations (each file read once).
    fn file_locations(&self, sites: &[(std::path::PathBuf, u32)]) -> Vec<lsp_types::Location> {
        const MAX: usize = 200;
        let mut out = Vec::new();
        let mut by_file: std::collections::BTreeMap<&std::path::Path, Vec<u32>> = std::collections::BTreeMap::new();
        for (f, o) in sites.iter().take(MAX) {
            by_file.entry(f.as_path()).or_default().push(*o);
        }
        for (file, offsets) in by_file {
            let Ok(bytes) = std::fs::read(file) else { continue };
            let text = String::from_utf8_lossy(&bytes);
            let index = line_index::LineIndex::new(&text);
            let Some(uri) = format::file_uri(file) else { continue };
            for o in offsets {
                let at = cfx_lua_ide::TextSize::from(o);
                let range = convert::range(&index, cfx_lua_ide::TextRange::new(at, at), self.encoding);
                out.push(lsp_types::Location { uri: uri.clone(), range });
            }
        }
        out
    }

    /// Globals, fields, exports and event handlers across the workspace.
    fn workspace_symbols(&self, query: &str) -> lsp_types::WorkspaceSymbolResponse {
        use cfx_lua_ide::workspace::WsSymbolKind as K;
        const MAX: usize = 300;
        let found = self.ws.symbols(query, MAX);
        let mut by_file: std::collections::BTreeMap<&std::path::Path, Vec<&cfx_lua_ide::workspace::WsSymbol>> = std::collections::BTreeMap::new();
        for s in &found {
            by_file.entry(s.path.as_path()).or_default().push(s);
        }
        let mut out = Vec::new();
        for (file, symbols) in by_file {
            // The open document's text over the file on disk.
            let open = self.docs.iter().find(|(u, _)| format::file_path(u).is_some_and(|p| p == file)).map(|(_, d)| d.text.clone());
            let text = match open {
                Some(t) => t,
                None => match std::fs::read(file) {
                    Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                    Err(_) => continue,
                },
            };
            let index = line_index::LineIndex::new(&text);
            let Some(uri) = format::file_uri(file) else { continue };
            for s in symbols {
                let start = (s.offset as usize).min(text.len());
                let end = (start + s.name.len()).min(text.len());
                let r = cfx_lua_ide::TextRange::new(cfx_lua_ide::TextSize::from(start as u32), cfx_lua_ide::TextSize::from(end as u32));
                let range = convert::range(&index, r, self.encoding);
                let kind = match s.kind {
                    K::Function => SymbolKind::FUNCTION,
                    K::Variable => SymbolKind::VARIABLE,
                    K::Field => SymbolKind::FIELD,
                    K::Export => SymbolKind::INTERFACE,
                    K::Event => SymbolKind::EVENT,
                };
                out.push(lsp_types::WorkspaceSymbol {
                    name: s.name.clone(),
                    kind,
                    tags: None,
                    container_name: (!s.container.is_empty()).then(|| s.container.clone()),
                    location: OneOf::Left(lsp_types::Location { uri: uri.clone(), range }),
                    data: None,
                });
            }
        }
        lsp_types::WorkspaceSymbolResponse::Nested(out)
    }

    fn signature_help(&self, p: &lsp_types::TextDocumentPositionParams) -> Option<lsp_types::SignatureHelp> {
        let (doc, offset) = self.at(p)?;
        let path = format::file_path(&p.text_document.uri);
        let cx = self.ide_context(&p.text_document.uri, path.as_deref());
        let help = cfx_lua_ide::signature_help(doc.parse.as_ref()?, doc.names()?, &doc.text, offset, cx)?;
        let signatures = help
            .signatures
            .into_iter()
            .map(|s| lsp_types::SignatureInformation {
                parameters: Some(
                    s.params
                        .iter()
                        .map(|(name, ty)| lsp_types::ParameterInformation {
                            label: lsp_types::ParameterLabel::Simple(name.clone()),
                            documentation: ty.clone().map(lsp_types::Documentation::String),
                        })
                        .collect(),
                ),
                label: s.label,
                documentation: (!s.doc.is_empty()).then_some(lsp_types::Documentation::MarkupContent(
                    lsp_types::MarkupContent { kind: lsp_types::MarkupKind::Markdown, value: s.doc },
                )),
                active_parameter: None,
            })
            .collect();
        Some(lsp_types::SignatureHelp {
            signatures,
            active_signature: Some(0),
            active_parameter: Some(help.active_param as u32),
        })
    }

    fn definition(&self, p: &lsp_types::TextDocumentPositionParams) -> Option<lsp_types::GotoDefinitionResponse> {
        let (doc, offset) = self.at(p)?;
        let names = doc.names()?;
        let ranges = names.definition(&doc.text, offset);
        if !ranges.is_empty() {
            return Some(lsp_types::GotoDefinitionResponse::Array(self.locations(&p.text_document.uri, doc, ranges)));
        }
        // An event name: its handlers (from a trigger) or its triggers (from a handler).
        if let Some(ev) = cfx_lua_ide::event_at(doc.parse.as_ref()?, &doc.text, offset) {
            let sites = self.ws.event_sites(&ev.name);
            let mut pick: Vec<(std::path::PathBuf, u32)> =
                sites.iter().filter(|(_, _, h)| *h != ev.handler).map(|(f, o, _)| (f.clone(), *o)).collect();
            if pick.is_empty() {
                pick = sites.into_iter().map(|(f, o, _)| (f, o)).collect();
            }
            return Some(lsp_types::GotoDefinitionResponse::Array(self.file_locations(&pick)));
        }
        // Defined in another file or resource.
        let path = format::file_path(&p.text_document.uri)?;
        let (file, at) = cfx_lua_ide::api_definition(doc.parse.as_ref()?, names, &doc.text, offset, &self.ws, &path)?;
        let text = String::from_utf8_lossy(&std::fs::read(&file).ok()?).into_owned();
        let index = line_index::LineIndex::new(&text);
        let at = cfx_lua_ide::TextSize::from(at);
        let range = convert::range(&index, cfx_lua_ide::TextRange::new(at, at), self.encoding);
        let uri = format::file_uri(&file)?;
        Some(lsp_types::GotoDefinitionResponse::Scalar(lsp_types::Location { uri, range }))
    }

    fn references(&self, p: &lsp_types::ReferenceParams) -> Option<Vec<lsp_types::Location>> {
        let (doc, offset) = self.at(&p.text_document_position)?;
        if let Some(ev) = cfx_lua_ide::event_at(doc.parse.as_ref()?, &doc.text, offset) {
            let sites: Vec<(std::path::PathBuf, u32)> = self.ws.event_sites(&ev.name).into_iter().map(|(f, o, _)| (f, o)).collect();
            return Some(self.file_locations(&sites));
        }
        let names = doc.names()?;
        let mut ranges = names.references(&doc.text, offset);
        if ranges.is_empty()
            && let Some(path) = format::file_path(&p.text_document_position.text_document.uri)
            && let Some(target) = self.ws.type_target(&path, &doc.text, offset)
        {
            let found = self.ws.type_references(&path, &target, &|f| self.text_of(f));
            if !found.is_empty() {
                return Some(self.ranges_locations(found));
            }
        }
        if ranges.is_empty()
            && let Some(sites) = cfx_lua_ide::field_references(doc.parse.as_ref()?, names, offset)
        {
            return Some(self.locations(&p.text_document_position.text_document.uri, doc, sites.into_iter().map(|(r, _)| r).collect()));
        }
        if ranges.is_empty()
            && let Some(sites) = cfx_lua_ide::function_references(doc.parse.as_ref()?, names, offset)
        {
            return Some(self.locations(&p.text_document_position.text_document.uri, doc, sites));
        }
        if !p.context.include_declaration {
            let defs = names.definition(&doc.text, offset);
            ranges.retain(|r| !defs.contains(r));
        }
        Some(self.locations(&p.text_document_position.text_document.uri, doc, ranges))
    }

    fn document_highlight(&self, p: &lsp_types::TextDocumentPositionParams) -> Option<Vec<lsp_types::DocumentHighlight>> {
        let (doc, offset) = self.at(p)?;
        let found = cfx_lua_ide::highlights(doc.parse.as_ref()?, doc.names()?, &doc.text, offset);
        let kind = |k| match k {
            cfx_lua_ide::HighlightKind::Text => lsp_types::DocumentHighlightKind::TEXT,
            cfx_lua_ide::HighlightKind::Read => lsp_types::DocumentHighlightKind::READ,
            cfx_lua_ide::HighlightKind::Write => lsp_types::DocumentHighlightKind::WRITE,
        };
        Some(
            found
                .into_iter()
                .map(|(r, k)| lsp_types::DocumentHighlight {
                    range: convert::range(&doc.index, r, self.encoding),
                    kind: Some(kind(k)),
                })
                .collect(),
        )
    }

    /// A file's text: the open document over the file on disk.
    fn text_of(&self, file: &std::path::Path) -> Option<String> {
        if let Some((_, d)) = self.docs.iter().find(|(u, _)| format::file_path(u).is_some_and(|p| p == file)) {
            return Some(d.text.clone());
        }
        std::fs::read(file).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    fn ranges_locations(&self, found: cfx_lua_ide::workspace::FileRanges) -> Vec<lsp_types::Location> {
        let mut out = Vec::new();
        for (file, ranges) in found {
            let (Some(text), Some(uri)) = (self.text_of(&file), format::file_uri(&file)) else { continue };
            let index = line_index::LineIndex::new(&text);
            out.extend(ranges.into_iter().map(|r| lsp_types::Location { uri: uri.clone(), range: convert::range(&index, r, self.encoding) }));
        }
        out
    }

    /// The class, alias, enum or class field under the cursor, by type.
    fn type_target(&self, p: &lsp_types::TextDocumentPositionParams) -> Option<(std::path::PathBuf, cfx_lua_ide::workspace::TypeTarget)> {
        let (doc, offset) = self.at(p)?;
        let path = format::file_path(&p.text_document.uri)?;
        let target = self.ws.type_target(&path, &doc.text, offset)?;
        Some((path, target))
    }

    /// The name under the cursor, when it can be renamed.
    fn prepare_rename(&self, p: &lsp_types::TextDocumentPositionParams) -> Option<lsp_types::PrepareRenameResponse> {
        let (doc, offset) = self.at(p)?;
        if let Some(path) = format::file_path(&p.text_document.uri)
            && let Some((_, here)) = self.ws.type_target_at(&path, &doc.text, offset)
        {
            return Some(lsp_types::PrepareRenameResponse::Range(convert::range(&doc.index, here, self.encoding)));
        }
        let names = doc.names()?;
        let ranges = cfx_lua_ide::rename(doc.parse.as_ref()?, names, &doc.text, offset, "x").ok()?;
        let here = ranges.into_iter().find(|r| r.contains_inclusive(offset))?;
        Some(lsp_types::PrepareRenameResponse::Range(convert::range(&doc.index, here, self.encoding)))
    }

    fn rename(&self, p: &lsp_types::RenameParams) -> Result<Option<lsp_types::WorkspaceEdit>, String> {
        let Some((doc, offset)) = self.at(&p.text_document_position) else { return Ok(None) };
        let Some(names) = doc.names() else { return Ok(None) };
        let Some(parse) = doc.parse.as_ref() else { return Ok(None) };
        // A class, alias or field by type, across the workspace.
        if let Some((path, target)) = self.type_target(&p.text_document_position) {
            let found = self.ws.type_rename(&path, &target, &p.new_name, &|f| self.text_of(f))?;
            let edits = found.into_iter().filter_map(|(file, ranges)| {
                let (text, uri) = (self.text_of(&file)?, format::file_uri(&file)?);
                let index = line_index::LineIndex::new(&text);
                let edits: Vec<TextEdit> =
                    ranges.into_iter().map(|r| TextEdit { range: convert::range(&index, r, self.encoding), new_text: p.new_name.clone() }).collect();
                Some((uri, edits))
            });
            return Ok(Some(lsp_types::WorkspaceEdit { changes: Some(edits.collect()), ..Default::default() }));
        }
        let ranges = cfx_lua_ide::rename(parse, names, &doc.text, offset, &p.new_name)?;
        let edits = ranges
            .into_iter()
            .map(|r| TextEdit { range: convert::range(&doc.index, r, self.encoding), new_text: p.new_name.clone() })
            .collect();
        let uri = p.text_document_position.text_document.uri.clone();
        Ok(Some(lsp_types::WorkspaceEdit { changes: Some(HashMap::from([(uri, edits)])), ..Default::default() }))
    }

    /// One edit over the whole text, or `null` when nothing changes, formatting is
    /// off (`Lua.format.enable`), or the file has syntax errors or is escrow.
    fn formatting(&self, p: &DocumentFormattingParams) -> Option<Vec<TextEdit>> {
        let uri = &p.text_document.uri;
        let doc = self.docs.get(uri).filter(|d| self.format.enable && d.parse.is_some())?;
        let style = format::style(uri, &self.format, &self.workspace, &p.options);
        let text = cfx_lua_fmt::format(&doc.text, &style)?;
        if text == doc.text {
            return None;
        }
        let range = convert::range(&doc.index, cfx_lua_ide::TextRange::up_to(doc.index.len()), self.encoding);
        Some(vec![TextEdit { range, new_text: text }])
    }

    /// The asked lines grown to whole tokens, replaced from the start of the first
    /// line to the start of the line after the last.
    fn range_formatting(&self, p: &DocumentRangeFormattingParams) -> Option<Vec<TextEdit>> {
        let uri = &p.text_document.uri;
        let doc = self.docs.get(uri).filter(|d| self.format.enable && d.parse.is_some())?;
        let style = format::style(uri, &self.format, &self.workspace, &p.options);
        let (start, end) = (p.range.start.line as usize, p.range.end.line as usize);
        let edit = cfx_lua_fmt::format_range(&doc.text, &style, start, end)?;
        let doc_end = convert::position(&doc.index, doc.index.len(), self.encoding);
        let next_line = u32::try_from(edit.end_line + 1).unwrap_or(u32::MAX);
        let end = if next_line > doc_end.line { doc_end } else { lsp_types::Position::new(next_line, 0) };
        let start = lsp_types::Position::new(edit.start_line as u32, 0);
        Some(vec![TextEdit { range: lsp_types::Range::new(start, end), new_text: edit.text }])
    }

    #[allow(deprecated)] // `deprecated` is a required field of the LSP type.
    fn symbol(&self, doc: &Document, s: cfx_lua_ide::DocumentSymbol) -> lsp_types::DocumentSymbol {
        use cfx_lua_ide::SymbolKind as K;
        let kind = match s.kind {
            K::Package => SymbolKind::PACKAGE,
            K::Method => SymbolKind::METHOD,
            K::Function => SymbolKind::FUNCTION,
            K::Variable => SymbolKind::VARIABLE,
            K::Constant => SymbolKind::CONSTANT,
            K::String => SymbolKind::STRING,
            K::Number => SymbolKind::NUMBER,
            K::Boolean => SymbolKind::BOOLEAN,
            K::Array => SymbolKind::ARRAY,
            K::Object => SymbolKind::OBJECT,
        };
        lsp_types::DocumentSymbol {
            // An empty name becomes a space, as LuaLS does (`provider.lua`): clients
            // may reject a symbol without a name.
            name: if s.name.is_empty() { " ".into() } else { s.name },
            detail: (!s.detail.is_empty()).then_some(s.detail),
            kind,
            tags: None,
            deprecated: None,
            range: convert::range(&doc.index, s.range, self.encoding),
            selection_range: convert::range(&doc.index, s.selection_range, self.encoding),
            children: (!s.children.is_empty()).then(|| s.children.into_iter().map(|c| self.symbol(doc, c)).collect()),
        }
    }

    fn notification(&mut self, n: Notification) -> Result<(), Error> {
        match n.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let p: lsp_types::DidOpenTextDocumentParams = serde_json::from_value(n.params)?;
                let doc = Document::new(p.text_document.text, p.text_document.version);
                let changed = self.track(&p.text_document.uri, &doc.text);
                self.docs.insert(p.text_document.uri.clone(), doc);
                self.republish(changed, &p.text_document.uri)?;
            }
            DidChangeTextDocument::METHOD => {
                let p: lsp_types::DidChangeTextDocumentParams = serde_json::from_value(n.params)?;
                let uri = p.text_document.uri;
                if let Some(doc) = self.docs.get_mut(&uri) {
                    doc.change(p.content_changes, p.text_document.version, self.encoding);
                    let text = doc.text.clone();
                    let changed = self.track(&uri, &text);
                    self.republish(changed, &uri)?;
                }
            }
            DidChangeConfiguration::METHOD => {
                let p: lsp_types::DidChangeConfigurationParams = serde_json::from_value(n.params)?;
                self.format = format::settings(&p.settings);
                let hints_before = self.client_settings.as_ref().and_then(|s| s.get("hint")).cloned();
                self.client_settings = p.settings.get("Lua").cloned();
                self.reload_config()?;
                // Hints turned on or off show without waiting for an edit.
                if self.inlay_refresh && self.client_settings.as_ref().and_then(|s| s.get("hint")).cloned() != hints_before {
                    self.next_id += 1;
                    let req = Request::new(format!("cfx-lua-ls/{}", self.next_id).into(), "workspace/inlayHint/refresh".into(), serde_json::Value::Null);
                    self.connection.sender.send(Message::Request(req))?;
                }
            }
            DidChangeWatchedFiles::METHOD => {
                let p: lsp_types::DidChangeWatchedFilesParams = serde_json::from_value(n.params)?;
                let mut changed = false;
                let mut config = false;
                for event in p.changes {
                    let Some(path) = format::file_path(&event.uri) else { continue };
                    if crate::config::is_luarc(&path) {
                        config = true;
                    } else if event.typ == lsp_types::FileChangeType::DELETED {
                        changed |= self.ws.remove(&path);
                    } else if !self.docs.contains_key(&event.uri) {
                        // Open documents follow the editor, not the disk.
                        let text = std::fs::read_to_string(&path).unwrap_or_default();
                        changed |= self.ws.update(&path, &text);
                    }
                }
                if config {
                    self.reload_config()?;
                } else if changed {
                    self.publish_all()?;
                }
            }
            DidCloseTextDocument::METHOD => {
                let p: lsp_types::DidCloseTextDocumentParams = serde_json::from_value(n.params)?;
                self.docs.remove(&p.text_document.uri);
                self.publish(&p.text_document.uri, None)?;
                // Unsaved edits are gone: the file's globals come from the disk again.
                if let Some(path) = format::file_path(&p.text_document.uri)
                    && let Ok(text) = std::fs::read_to_string(&path)
                    && self.ws.update(&path, &text)
                {
                    self.publish_all()?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Feeds a document's text to the workspace; `true` when other files'
    /// diagnostics may change.
    fn track(&mut self, uri: &Uri, text: &str) -> bool {
        let Some(path) = format::file_path(uri) else { return false };
        self.ws.ensure(&path);
        self.ws.update(&path, text)
    }

    fn republish(&self, all: bool, uri: &Uri) -> Result<(), Error> {
        if all { self.publish_all() } else { self.publish(uri, self.docs.get(uri)) }
    }

    fn publish_all(&self) -> Result<(), Error> {
        for (uri, doc) in &self.docs {
            self.publish(uri, Some(doc))?;
        }
        Ok(())
    }

    fn reload_config(&mut self) -> Result<(), Error> {
        self.diagnostics = crate::config::load(&self.workspace, self.client_settings.as_ref());
        self.publish_all()
    }

    /// The lint results for an open document.
    fn lints(&self, uri: &Uri, doc: &Document) -> Vec<Lint> {
        let (Some(parse), Some(names)) = (doc.parse.as_ref(), doc.names()) else { return Vec::new() };
        if !self.diagnostics.enable {
            return Vec::new();
        }
        let config = &self.diagnostics.lint;
        match format::file_path(uri) {
            Some(path) => cfx_lua_ide::workspace::lint_file(&self.ws, &path, parse, &doc.text, names, config),
            None => {
                let ctx = cfx_lua_ide::lint::Context { config, ..Default::default() };
                cfx_lua_ide::lint::lint(parse, &doc.text, names, &ctx)
            }
        }
    }

    fn lint_diagnostic(&self, doc: &Document, l: &Lint) -> lsp_types::Diagnostic {
        lsp_types::Diagnostic {
            range: convert::range(&doc.index, l.range, self.encoding),
            severity: Some(match l.severity {
                Severity::Error => DiagnosticSeverity::ERROR,
                Severity::Warning => DiagnosticSeverity::WARNING,
                Severity::Information => DiagnosticSeverity::INFORMATION,
                Severity::Hint => DiagnosticSeverity::HINT,
            }),
            code: Some(NumberOrString::String(l.code.into())),
            source: Some(LINT_SOURCE.into()),
            message: l.message.clone(),
            tags: l.tag.map(|t| {
                vec![match t {
                    Tag::Unnecessary => lsp_types::DiagnosticTag::UNNECESSARY,
                    Tag::Deprecated => lsp_types::DiagnosticTag::DEPRECATED,
                }]
            }),
            ..Default::default()
        }
    }

    /// Quick fixes for the lints in the range, then "disable for this line" and
    /// "disable in this file" for each code.
    fn code_actions(&self, p: &lsp_types::CodeActionParams) -> Option<Vec<lsp_types::CodeActionOrCommand>> {
        let uri = &p.text_document.uri;
        let doc = self.docs.get(uri)?;
        let range = convert::text_range(&doc.index, &doc.text, p.range, self.encoding);
        let lints: Vec<Lint> =
            self.lints(uri, doc).into_iter().filter(|l| l.range.intersect(range).is_some()).collect();
        let edit = |edits: Vec<TextEdit>| lsp_types::WorkspaceEdit {
            changes: Some(HashMap::from([(uri.clone(), edits)])),
            ..Default::default()
        };
        let mut out = Vec::new();
        for l in &lints {
            let Some(fix) = &l.fix else { continue };
            let edits = fix
                .edits
                .iter()
                .map(|(r, t)| TextEdit { range: convert::range(&doc.index, *r, self.encoding), new_text: t.clone() })
                .collect();
            out.push(lsp_types::CodeActionOrCommand::CodeAction(lsp_types::CodeAction {
                title: fix.title.clone(),
                kind: Some(lsp_types::CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![self.lint_diagnostic(doc, l)]),
                edit: Some(edit(edits)),
                is_preferred: Some(true),
                ..Default::default()
            }));
        }
        let mut seen = std::collections::HashSet::new();
        for l in &lints {
            if !seen.insert(l.code) {
                continue;
            }
            let start = doc.index.line_col(l.range.start()).line;
            let line_start = doc.index.line(start).map_or(0.into(), |r| r.start());
            let indent: String =
                doc.text[usize::from(line_start)..].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            let at = convert::position(&doc.index, line_start, self.encoding);
            for (title, pos, text) in [
                (
                    format!("Disable {} for this line", l.code),
                    at,
                    format!("{indent}---@diagnostic disable-next-line: {}\n", l.code),
                ),
                (
                    format!("Disable {} in this file", l.code),
                    lsp_types::Position::new(0, 0),
                    format!("---@diagnostic disable: {}\n", l.code),
                ),
            ] {
                out.push(lsp_types::CodeActionOrCommand::CodeAction(lsp_types::CodeAction {
                    title,
                    kind: Some(lsp_types::CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![self.lint_diagnostic(doc, l)]),
                    edit: Some(edit(vec![TextEdit { range: lsp_types::Range::new(pos, pos), new_text: text }])),
                    ..Default::default()
                }));
            }
        }
        Some(out)
    }

    /// Sends the document's syntax and lint diagnostics; `None` clears them
    /// (closed file).
    fn publish(&self, uri: &Uri, doc: Option<&Document>) -> Result<(), Error> {
        let diagnostics = doc
            .and_then(|d| d.parse.as_ref().map(|p| (d, p)))
            .map(|(d, p)| {
                let syntax = cfx_lua_ide::syntax_diagnostics(p).into_iter().map(|diag| lsp_types::Diagnostic {
                    range: convert::range(&d.index, diag.range, self.encoding),
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: Some(NumberOrString::String(diag.code)),
                    source: Some(SYNTAX_SOURCE.into()),
                    message: diag.message,
                    ..Default::default()
                });
                let lints = self.lints(uri, d);
                syntax.chain(lints.iter().map(|l| self.lint_diagnostic(d, l))).collect()
            })
            .unwrap_or_default();
        let params = PublishDiagnosticsParams { uri: uri.clone(), diagnostics, version: doc.map(|d| d.version) };
        let n = Notification::new(PublishDiagnostics::METHOD.into(), params);
        self.connection.sender.send(Message::Notification(n))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
