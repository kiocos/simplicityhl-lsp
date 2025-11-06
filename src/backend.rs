use ropey::Rope;
use serde_json::Value;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, Diagnostic, DiagnosticSeverity,
    DidChangeConfigurationParams, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, ExecuteCommandParams, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, Location, MarkupContent, MarkupKind, MessageType, OneOf, Range, SaveOptions,
    SemanticToken, SemanticTokens, SemanticTokensLegend, SemanticTokensOptions, SemanticTokensParams,
    SemanticTokensResult, ServerCapabilities, SemanticTokensServerCapabilities, SemanticTokensFullOptions,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, Uri, WorkDoneProgressOptions, WorkspaceFoldersServerCapabilities,
    WorkspaceServerCapabilities,
};
use tower_lsp_server::{Client, LanguageServer};

use simplicityhl::{
    ast,
    error::{RichError, WithFile},
    parse,
    parse::ParseFromStr,
};

use crate::completion::{self, CompletionProvider};
use crate::error::LspError;
use crate::function::Functions;
use crate::utils::{
    find_related_call, get_call_span, get_comments_from_lines, position_to_span, span_to_positions,
};

#[derive(Debug)]
struct Document {
    functions: Functions,
    text: Rope,
}

#[derive(Debug)]
pub struct Backend {
    client: Client,

    document_map: Arc<RwLock<HashMap<Uri, Document>>>,

    completion_provider: CompletionProvider,
}

struct TextDocumentItem<'a> {
    uri: Uri,
    text: &'a str,
    version: Option<i32>,
}

impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            server_info: None,
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(true),
                        })),
                        ..Default::default()
                    },
                )),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![":".to_string()]),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                    all_commit_characters: None,
                    completion_item: None,
                }),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    file_operations: None,
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            work_done_progress_options: WorkDoneProgressOptions::default(),
                            legend: SemanticTokensLegend {
                                token_types: vec![
                                    "keyword".into(),
                                    "string".into(),
                                    "comment".into(),
                                    "number".into(),
                                    "function".into(),
                                    "operator".into(),
                                    "namespace".into(),
                                ],
                                token_modifiers: vec![],
                            },
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            range: Some(false),
                        },
                    ),
                ),
                definition_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {}

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_change_workspace_folders(&self, _: DidChangeWorkspaceFoldersParams) {}

    async fn did_change_configuration(&self, _: DidChangeConfigurationParams) {}

    async fn did_change_watched_files(&self, _: DidChangeWatchedFilesParams) {}

    async fn execute_command(&self, _: ExecuteCommandParams) -> Result<Option<Value>> {
        Ok(None)
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.on_change(TextDocumentItem {
            uri: params.text_document.uri,
            text: &params.text_document.text,
            version: Some(params.text_document.version),
        })
        .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        self.on_change(TextDocumentItem {
            text: &params.content_changes[0].text,
            uri: params.text_document.uri,
            version: Some(params.text_document.version),
        })
        .await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if let Some(text) = params.text {
            self.on_change(TextDocumentItem {
                uri: params.text_document.uri,
                text: &text,
                version: None,
            })
            .await;
        }
    }

    async fn did_close(&self, _: DidCloseTextDocumentParams) {}

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        // Build semantic tokens for the requested document if present
        let documents = self.document_map.read().await;
        let uri = &params.text_document.uri;

        if let Some(doc) = documents.get(uri) {
            let text = doc
                .text
                .to_string();

            let data = build_semantic_tokens_data(&text);

            return Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                result_id: None,
                data,
            })));
        }

        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: vec![],
        })))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let documents = self.document_map.read().await;
        let uri = &params.text_document_position.text_document.uri;

        let doc = documents
            .get(uri)
            .ok_or(LspError::DocumentNotFound(uri.to_owned()))?;

        let pos = params.text_document_position.position;

        let line = doc
            .text
            .lines()
            .nth(pos.line as usize)
            .ok_or(LspError::Internal("Rope proccesing error".into()))?;

        let slice = line
            .get_slice(..pos.character as usize)
            .ok_or(LspError::ConversionFailed(
                "Rope to slice conversion failed".into(),
            ))?;

        let prefix = slice.as_str().ok_or(LspError::ConversionFailed(
            "RopeSlice to str conversion failed".into(),
        ))?;

        let trimmed_prefix = prefix.trim_end();

        if let Some(last) = trimmed_prefix
            .rsplit(|c: char| !c.is_alphanumeric() && c != ':')
            .next()
        {
            if last.starts_with("jet:::") {
                return Ok(Some(CompletionResponse::Array(vec![])));
            } else if last == "jet::" || last.starts_with("jet::") {
                return Ok(Some(CompletionResponse::Array(
                    self.completion_provider.jets().to_vec(),
                )));
            }
        // Completion after a colon is needed only for jets.
        } else if trimmed_prefix.ends_with(':') {
            return Ok(Some(CompletionResponse::Array(vec![])));
        }

        let mut completions =
            CompletionProvider::get_function_completions(&doc.functions.functions_and_docs());
        completions.extend_from_slice(self.completion_provider.builtins());
        completions.extend_from_slice(self.completion_provider.modules());

        Ok(Some(CompletionResponse::Array(completions)))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let documents = self.document_map.read().await;
        let uri = &params.text_document_position_params.text_document.uri;

        let doc = documents
            .get(uri)
            .ok_or(LspError::DocumentNotFound(uri.to_owned()))?;
        let functions = doc.functions.functions();

        let token_pos = params.text_document_position_params.position;

        let token_span = position_to_span(token_pos)?;
        let Ok(Some(call)) = find_related_call(&functions, token_span) else {
            return Ok(None);
        };

        let call_span = get_call_span(call)?;
        let (start, end) = span_to_positions(&call_span)?;

        let description = match call.name() {
            parse::CallName::Jet(jet) => {
                let element =
                    simplicityhl::simplicity::jet::Elements::from_str(format!("{jet}").as_str())
                        .map_err(|err| LspError::ConversionFailed(err.to_string()))?;

                let template = completion::jet::jet_to_template(element);
                format!(
                    "```simplicityhl\nfn jet::{}({}) -> {}\n```\n{}",
                    template.display_name,
                    template.args.join(", "),
                    template.return_type,
                    template.description
                )
            }
            parse::CallName::Custom(func) => {
                let (function, function_doc) =
                    doc.functions
                        .get(func.as_inner())
                        .ok_or(LspError::FunctionNotFound(format!(
                            "Function {func} is not found"
                        )))?;

                let template = completion::function_to_template(function, function_doc);
                format!(
                    "```simplicityhl\nfn {}({}) -> {}\n```\n{}",
                    template.display_name,
                    template.args.join(", "),
                    template.return_type,
                    template.description
                )
            }
            other => {
                let Some(template) = completion::builtin::match_callname(other) else {
                    return Ok(None);
                };
                format!(
                    "```simplicityhl\nfn {}({}) -> {}\n```\n{}",
                    template.display_name,
                    template.args.join(", "),
                    template.return_type,
                    template.description
                )
            }
        };

        Ok(Some(Hover {
            contents: tower_lsp_server::lsp_types::HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: description,
            }),
            range: Some(Range { start, end }),
        }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let documents = self.document_map.read().await;
        let uri = &params.text_document_position_params.text_document.uri;

        let doc = documents
            .get(uri)
            .ok_or(LspError::DocumentNotFound(uri.to_owned()))?;
        let functions = doc.functions.functions();

        let token_position = params.text_document_position_params.position;
        let token_span = position_to_span(token_position)?;

        let Ok(Some(call)) = find_related_call(&functions, token_span) else {
            return Ok(None);
        };

        match call.name() {
            simplicityhl::parse::CallName::Custom(func) => {
                let function =
                    doc.functions
                        .get_func(func.as_inner())
                        .ok_or(LspError::FunctionNotFound(format!(
                            "Function {func} is not found"
                        )))?;

                let (start, end) = span_to_positions(function.as_ref())?;
                Ok(Some(GotoDefinitionResponse::from(Location::new(
                    uri.clone(),
                    Range::new(start, end),
                ))))
            }
            _ => Ok(None),
        }
    }
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            document_map: Arc::new(RwLock::new(HashMap::new())),
            completion_provider: CompletionProvider::new(),
        }
    }

    /// Function which executed on change of file (`did_save`, `did_open` or `did_change` methods)
    async fn on_change(&self, params: TextDocumentItem<'_>) {
        let (err, document) = parse_program(params.text);

        let mut documents = self.document_map.write().await;
        if let Some(doc) = document {
            documents.insert(params.uri.clone(), doc);
        } else if let Some(doc) = documents.get_mut(&params.uri) {
            doc.text = Rope::from_str(params.text);
        }

        match err {
            None => {
                self.client
                    .publish_diagnostics(params.uri.clone(), vec![], params.version)
                    .await;
            }
            Some(err) => {
                let (start, end) = match span_to_positions(err.span()) {
                    Ok(result) => result,
                    Err(err) => {
                        self.client
                            .log_message(
                                MessageType::ERROR,
                                format!("Catch error while parsing span: {err}"),
                            )
                            .await;
                        return;
                    }
                };

                self.client
                    .publish_diagnostics(
                        params.uri.clone(),
                        vec![Diagnostic {
                            range: Range::new(start, end),
                            severity: Some(DiagnosticSeverity::ERROR),
                            code: None,
                            code_description: None,
                            source: Some("simplicityhl".to_string()),
                            message: err.error().to_string(),
                            related_information: None,
                            tags: None,
                            data: None,
                        }],
                        params.version,
                    )
                    .await;
            }
        }
    }
}

/// Create [`Document`] using parsed program and code.
fn create_document(program: &simplicityhl::parse::Program, text: &str) -> Document {
    let mut document = Document {
        functions: Functions::new(),
        text: Rope::from_str(text),
    };

    program
        .items()
        .iter()
        .filter_map(|item| {
            if let parse::Item::Function(func) = item {
                Some(func)
            } else {
                None
            }
        })
        .for_each(|func| {
            let start_line = u32::try_from(func.as_ref().start.line.get()).unwrap_or_default() - 1;

            document.functions.insert(
                func.name().to_string(),
                func.to_owned(),
                get_comments_from_lines(start_line, &document.text),
            );
        });

    document
}

/// Parse program using [`simplicityhl`] compiler and return [`RichError`],
/// which used in Diagnostic. Also create [`Document`] from parsed program.
fn parse_program(text: &str) -> (Option<RichError>, Option<Document>) {
    let program = match parse::Program::parse_from_str(text) {
        Ok(p) => p,
        Err(e) => return (Some(e), None),
    };

    (
        ast::Program::analyze(&program).with_file(text).err(),
        Some(create_document(&program, text)),
    )
}

// ----------------------------------------------------------------------------
// Semantic tokens builder (very lightweight lexer)
// ----------------------------------------------------------------------------

// Keep this order in sync with the WebIDE client mapping
// 0 keyword, 1 string, 2 comment, 3 number, 4 function, 5 operator, 6 namespace
const TOKEN_TYPE_KEYWORD: u32 = 0;
const TOKEN_TYPE_STRING: u32 = 1;
const TOKEN_TYPE_COMMENT: u32 = 2;
const TOKEN_TYPE_NUMBER: u32 = 3;
const TOKEN_TYPE_FUNCTION: u32 = 4;
const TOKEN_TYPE_OPERATOR: u32 = 5;
const TOKEN_TYPE_NAMESPACE: u32 = 6;

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn build_semantic_tokens_data(text: &str) -> Vec<SemanticToken> {
    let keywords: &[&str] = &[
        "let", "mut", "if", "else", "match", "fn", "return", "pub", "struct", "enum",
        "impl", "use", "as", "for", "while", "loop", "break", "continue", "const",
        "static", "trait", "where", "type", "mod", "crate", "self", "super", "in",
        "true", "false", "Some", "None", "Ok", "Err", "Option", "Result",
    ];

    // Collect absolute tokens as (line, start, len, type)
    let mut abs_tokens: Vec<(u32, u32, u32, u32)> = Vec::new();
    let mut in_block_comment = false;

    for (line_idx, line) in text.split('\n').enumerate() {
        let mut i = 0usize;
        let bytes = line.as_bytes();
        let len = bytes.len();

        if in_block_comment {
            // Look for end of block comment
            if let Some(end_pos) = line.find("*/") {
                // Mark from 0..=end_pos+2 as comment
                abs_tokens.push((line_idx as u32, 0, (end_pos + 2) as u32, TOKEN_TYPE_COMMENT));
                in_block_comment = false;
                i = end_pos + 2;
            } else {
                // Entire line is comment
                abs_tokens.push((line_idx as u32, 0, len as u32, TOKEN_TYPE_COMMENT));
                continue;
            }
        }

        while i < len {
            // Line comment
            if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
                abs_tokens.push((line_idx as u32, i as u32, (len - i) as u32, TOKEN_TYPE_COMMENT));
                break;
            }
            // Block comment start
            if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                if let Some(end_pos) = line[i + 2..].find("*/") {
                    let tok_len = 2 + end_pos + 2; // /* + content + */
                    abs_tokens.push((line_idx as u32, i as u32, tok_len as u32, TOKEN_TYPE_COMMENT));
                    i += tok_len;
                    continue;
                } else {
                    // Rest of line is comment; mark and carry state
                    abs_tokens.push((line_idx as u32, i as u32, (len - i) as u32, TOKEN_TYPE_COMMENT));
                    in_block_comment = true;
                    break;
                }
            }

            // String literal starting with '"'
            if bytes[i] == b'"' {
                let mut j = i + 1;
                let mut escaped = false;
                while j < len {
                    let c = bytes[j];
                    if escaped {
                        escaped = false;
                    } else if c == b'\\' {
                        escaped = true;
                    } else if c == b'"' {
                        j += 1; // include closing quote
                        break;
                    }
                    j += 1;
                }
                abs_tokens.push((line_idx as u32, i as u32, (j - i) as u32, TOKEN_TYPE_STRING));
                i = j;
                continue;
            }

            // jet::namespace path
            if i + 5 <= len && line[i..].starts_with("jet::") {
                let mut j = i + 5;
                while j < len {
                    let ch = line[j..].chars().next().unwrap_or('\0');
                    if is_ident_char(ch) || ch == ':' {
                        j += ch.len_utf8();
                    } else {
                        break;
                    }
                }
                abs_tokens.push((line_idx as u32, i as u32, (j - i) as u32, TOKEN_TYPE_NAMESPACE));
                i = j;
                continue;
            }

            // Operators (single char)
            let ch = line[i..].chars().next().unwrap_or('\0');
            if matches!(ch, '+' | '-' | '*' | '/' | '%' | '=' | '!' | '<' | '>' | '&' | '|' | '^' | '~' | '?' | ':') {
                abs_tokens.push((line_idx as u32, i as u32, ch.len_utf8() as u32, TOKEN_TYPE_OPERATOR));
                i += ch.len_utf8();
                continue;
            }

            // Word / number
            if is_ident_char(ch) {
                // Read word (identifier or number)
                let start = i;
                let mut j = i + ch.len_utf8();
                while j < len {
                    let cj = line[j..].chars().next().unwrap_or('\0');
                    if is_ident_char(cj) {
                        j += cj.len_utf8();
                    } else {
                        break;
                    }
                }
                let word = &line[start..j];
                // fn keyword and function name
                if word == "fn" {
                    abs_tokens.push((line_idx as u32, start as u32, (j - start) as u32, TOKEN_TYPE_KEYWORD));
                    // Lookahead for function name
                    let mut k = j;
                    while k < len && line[k..].chars().next().unwrap_or(' ') == ' ' {
                        k += 1;
                    }
                    let mut t = k;
                    while t < len {
                        let ct = line[t..].chars().next().unwrap_or('\0');
                        if is_ident_char(ct) {
                            t += ct.len_utf8();
                        } else {
                            break;
                        }
                    }
                    if t > k {
                        abs_tokens.push((line_idx as u32, k as u32, (t - k) as u32, TOKEN_TYPE_FUNCTION));
                    }
                    i = j;
                    continue;
                }

                if keywords.contains(&word) {
                    abs_tokens.push((line_idx as u32, start as u32, (j - start) as u32, TOKEN_TYPE_KEYWORD));
                } else if word.starts_with("0x") || word.starts_with("0b") || word.starts_with("0o") || word.chars().all(|c| c.is_ascii_digit()) {
                    abs_tokens.push((line_idx as u32, start as u32, (j - start) as u32, TOKEN_TYPE_NUMBER));
                }
                i = j;
                continue;
            }

            // Advance by one char when nothing matched
            i += ch.len_utf8().max(1);
        }
    }

    // Convert to LSP delta-encoded SemanticToken list
    abs_tokens.sort_by_key(|(l, s, _len, _t)| (*l, *s));
    let mut out: Vec<SemanticToken> = Vec::with_capacity(abs_tokens.len());
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;
    for (line, start, length, token_type) in abs_tokens.into_iter() {
        let delta_line = line.saturating_sub(prev_line);
        let delta_start = if delta_line == 0 { start.saturating_sub(prev_start) } else { start };
        out.push(SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type,
            token_modifiers_bitset: 0,
        });
        prev_line = line;
        prev_start = start;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_program() -> &'static str {
        "fn add(a: u32, b: u32) -> u32 { let (_, res): (bool, u32) = jet::add_32(a, b); res }
         fn main() {}"
    }

    fn invalid_program_on_ast() -> &'static str {
        "fn add(a: u32, b: u32) -> u32 {}"
    }

    fn invalid_program_on_parsing() -> &'static str {
        "fn add(a: u32 b: u32) -> u32 {}"
    }

    #[test]
    fn test_parse_program_valid() {
        let (err, doc) = parse_program(sample_program());
        assert!(err.is_none(), "Expected no parsing error");
        let doc = doc.expect("Expected Some(Document)");
        assert_eq!(doc.functions.map.len(), 2);
    }

    #[test]
    fn test_parse_program_invalid_ast() {
        let (err, doc) = parse_program(invalid_program_on_ast());
        assert!(
            err.unwrap()
                .to_string()
                .contains("Expected expression of type `u32`, found type `()`"),
            "Expected error on return type"
        );
        assert!(doc.is_some(), "Expected problem in AST build, not parse");
    }

    #[test]
    fn test_parse_program_invalid_parse() {
        let (err, doc) = parse_program(invalid_program_on_parsing());
        assert!(
            err.unwrap().to_string().contains("Grammar error"),
            "Expected `Grammar error`"
        );
        assert!(doc.is_none(), "Expected no document to return");
    }
}
