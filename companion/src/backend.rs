//! LSP backend: nbformat diagnostics, document-level code actions, and the
//! pairing workflow for .ipynb files.
//!
//! Every code action is a pure WorkspaceEdit (no server-side side effects on
//! selection): Zed applies `document_changes`, including resource create and
//! delete operations, which is how pairing creates/removes the script file.

use tower_lsp::lsp_types;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse,
    CreateFile, CreateFileOptions, DeleteFile, Diagnostic, DocumentChangeOperation,
    DocumentChanges, InitializeParams, InitializeResult, MessageType, OneOf,
    OptionalVersionedTextDocumentIdentifier, Position, Range, ResourceOp, ResourceOperationKind,
    ServerCapabilities, TextDocumentContentChangeEvent, TextDocumentEdit, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextEdit, Url, WorkspaceEdit,
};
use std::collections::HashMap;

use std::sync::Arc;
use tokio::sync::RwLock;
use tower_lsp::async_trait;
use tower_lsp::{Client, LanguageServer};

use crate::nbformat;
use crate::pairing;
use crate::percent;

pub struct Backend {
    state: Arc<BackendState>,
}

struct BackendState {
    client: Client,
    docs: RwLock<HashMap<Url, DocState>>,
    pairs: pairing::PairMap,
    supports_document_changes: RwLock<bool>,
}

struct DocState {
    version: i32,
    text: String,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Backend {
            state: Arc::new(BackendState {
                client,
                docs: RwLock::new(HashMap::new()),
                pairs: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
                supports_document_changes: RwLock::new(false),
            }),
        }
    }
}

impl BackendState {
    async fn refresh_diagnostics(&self, uri: &Url) {
        let (version, text) = {
            let docs = self.docs.read().await;
            match docs.get(uri) {
                Some(doc) => (Some(doc.version), doc.text.clone()),
                None => (None, String::new()),
            }
        };

        let mut diagnostics: Vec<Diagnostic> = nbformat::validate(&text);

        if let Ok(notebook_path) = uri.to_file_path() {
            let pairs = self.pairs.lock().await;
            if let Some(pair) = pairs.get(&notebook_path) {
                let state = pair.lock().await;
                if state.diverged {
                    diagnostics.push(Diagnostic {
                        range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                        severity: Some(lsp_types::DiagnosticSeverity::WARNING),
                        code: Some(lsp_types::NumberOrString::String(
                            pairing::DIVERGENCE_CODE.to_string(),
                        )),
                        source: Some(nbformat::SOURCE.to_string()),
                        message: "Notebook and paired script have diverged; resolve via code \
                                  actions (\"Sync notebook -> script\" or \"Sync script -> \
                                  notebook\")."
                            .to_string(),
                        ..Diagnostic::default()
                    });
                }
            }
        }

        self.client.publish_diagnostics(uri.clone(), diagnostics, version).await;
    }

    async fn handle_pair_event(&self, update: pairing::PairUpdate) {
        let uri = match pairing::uri_for_path(&update.notebook_path) {
            Some(uri) => uri,
            None => return,
        };

        let pair = {
            let pairs = self.pairs.lock().await;
            pairs.get(&update.notebook_path).cloned()
        };
        let Some(pair) = pair else { return };

        pairing::evaluate_pair(&pair, &update.notebook_path).await;
        self.refresh_diagnostics(&uri).await;
    }
}

fn whole_document_range(text: &str) -> Range {
    let mut line = 0u32;
    let mut character = 0u32;
    for ch in text.chars() {
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            // approximate UTF-16 length with char count (identical for BMP)
            character += ch.len_utf16() as u32;
        }
    }
    Range::new(Position::new(0, 0), Position::new(line, character))
}

fn replace_document_edit(uri: &Url, old_text: &str, new_text: String) -> WorkspaceEdit {
    WorkspaceEdit {
        changes: None,
        document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: None,
            },
            edits: vec![OneOf::Left(TextEdit {
                range: whole_document_range(old_text),
                new_text,
            })],
        }])),
        change_annotations: None,
    }
}

fn replace_changes_edit(uri: &Url, old_text: &str, new_text: String) -> WorkspaceEdit {
    WorkspaceEdit {
        changes: Some(
            [(
                uri.clone(),
                vec![TextEdit {
                    range: whole_document_range(old_text),
                    new_text,
                }],
            )]
            .into_iter()
            .collect(),
        ),
        document_changes: None,
        change_annotations: None,
    }
}

fn edit_action(title: &str, kind: CodeActionKind, edit: WorkspaceEdit) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title: title.to_string(),
        kind: Some(kind),
        edit: Some(edit),
        ..CodeAction::default()
    })
}

#[async_trait]
impl LanguageServer for Backend {
    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> tower_lsp::jsonrpc::Result<InitializeResult> {
        let supports = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.workspace_edit.as_ref())
            .map(|edit: &lsp_types::WorkspaceEditClientCapabilities| {
                edit.document_changes.unwrap_or(false)
                    && edit
                        .resource_operations
                        .as_ref()
                        .map(|ops: &Vec<ResourceOperationKind>| {
                            ops.contains(&ResourceOperationKind::Create)
                                && ops.contains(&ResourceOperationKind::Delete)
                        })
                        .unwrap_or(false)
            })
            .unwrap_or(false);

        *self.state.supports_document_changes.write().await = supports;

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                position_encoding: Some(lsp_types::PositionEncodingKind::UTF8),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                code_action_provider: Some(lsp_types::CodeActionProviderCapability::Simple(true)),
                ..ServerCapabilities::default()
            },
            server_info: Some(lsp_types::ServerInfo {
                name: "ipynb-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: lsp_types::InitializedParams) {
        self.state
            .client
            .log_message(MessageType::INFO, "ipynb-lsp ready")
            .await;
    }

    async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: lsp_types::DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;
        let text = params.text_document.text;

        self.state
            .docs
            .write()
            .await
            .insert(uri.clone(), DocState { version, text });
        self.state.refresh_diagnostics(&uri).await;

        if let Ok(path) = uri.to_file_path() {
            if path.extension().and_then(|e| e.to_str()) == Some("ipynb") {
                let state = self.state.clone();
                let _ = pairing::start_pair(state.pairs.clone(), path, move |update| {
                    let state = state.clone();
                    tokio::spawn(async move {
                        state.handle_pair_event(update).await;
                    });
                })
                .await;
            }
        }
    }

    async fn did_change(&self, params: lsp_types::DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;

        if let Some(TextDocumentContentChangeEvent { text, .. }) =
            params.content_changes.into_iter().last()
        {
            self.state
                .docs
                .write()
                .await
                .insert(uri.clone(), DocState { version, text });
        }

        self.state.refresh_diagnostics(&uri).await;
    }

    async fn did_close(&self, params: lsp_types::DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.state.docs.write().await.remove(&uri);
        self.state
            .client
            .publish_diagnostics(uri.clone(), Vec::new(), None)
            .await;

        if let Ok(path) = uri.to_file_path() {
            pairing::remove_pair(&self.state.pairs, &path).await;
        }
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> tower_lsp::jsonrpc::Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;

        let (text, notebook) = {
            let docs = self.state.docs.read().await;
            let Some(doc) = docs.get(&uri) else {
                return Ok(Some(Vec::new()));
            };
            let notebook = nbformat::parse(&doc.text).ok();
            (doc.text.clone(), notebook)
        };

        let Some(notebook) = notebook else {
            return Ok(Some(Vec::new()));
        };

        let path = match uri.to_file_path() {
            Ok(path) => path,
            Err(_) => return Ok(Some(Vec::new())),
        };
        let script_path = percent::script_path_for(&path);
        let script_uri = match Url::from_file_path(&script_path) {
            Ok(uri) => uri,
            Err(_) => return Ok(Some(Vec::new())),
        };
        let script_exists = script_path.is_file();
        let script_text = script_exists.then(|| std::fs::read_to_string(&script_path).ok().unwrap_or_default());

        let supports_document_changes = *self.state.supports_document_changes.read().await;
        let mut actions: Vec<CodeActionOrCommand> = Vec::new();

        // -- notebook surgery (whole-document canonicalizing edits) --------

        if nbformat::has_outputs(&notebook) {
            let cleared = nbformat::clear_outputs(&notebook);
            actions.push(edit_action(
                "ipynb: Clear all outputs",
                CodeActionKind::QUICKFIX,
                replace_document_edit(&uri, &text, nbformat::serialize_pretty(&cleared)),
            ));
        }
        if nbformat::empty_cell_count(&notebook) > 0 {
            let stripped = nbformat::strip_empty_cells(&notebook);
            actions.push(edit_action(
                "ipynb: Strip empty cells",
                CodeActionKind::QUICKFIX,
                replace_document_edit(&uri, &text, nbformat::serialize_pretty(&stripped)),
            ));
        }

        for cell_type in ["code", "markdown"] {
            let added = nbformat::add_cell(&notebook, cell_type);
            actions.push(edit_action(
                &format!("ipynb: Add {cell_type} cell"),
                CodeActionKind::REFACTOR,
                replace_document_edit(&uri, &text, nbformat::serialize_pretty(&added)),
            ));
        }

        // -- pairing --------------------------------------------------------

        let expected_script = percent::notebook_to_percent(&notebook).unwrap_or_default();

        if !script_exists {
            if supports_document_changes {
                let create = DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                    uri: script_uri.clone(),
                    options: Some(CreateFileOptions {
                        overwrite: Some(false),
                        ignore_if_exists: Some(false),
                    }),
                    annotation_id: None,
                }));
                let insert = DocumentChangeOperation::Edit(TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        uri: script_uri.clone(),
                        version: None,
                    },
                    edits: vec![OneOf::Left(TextEdit {
                        range: whole_document_range(""),
                        new_text: expected_script,
                    })],
                });
                actions.push(edit_action(
                    &format!("ipynb: Pair with percent script ({})", script_path.display()),
                    CodeActionKind::REFACTOR,
                    WorkspaceEdit {
                        changes: None,
                        document_changes: Some(DocumentChanges::Operations(vec![create, insert])),
                        change_annotations: None,
                    },
                ));
            }
        } else {
            if script_text.as_deref().unwrap_or("").trim() != expected_script.trim() {
                actions.push(edit_action(
                    "ipynb: Sync notebook -> script",
                    CodeActionKind::REFACTOR,
                    replace_changes_edit(&script_uri, script_text.as_deref().unwrap_or(""), expected_script),
                ));
            }

            let imported_text = script_text
                .as_deref()
                .and_then(|s| percent::percent_to_notebook(s, notebook.get("metadata")).ok())
                .map(|script| percent::merge_import_with_notebook(&script, &notebook))
                .map(|imported| nbformat::serialize_pretty(&imported));

            if let Some(imported_text) = imported_text.filter(|t| *t != text) {
                actions.push(edit_action(
                    "ipynb: Sync script -> notebook (outputs merged by cell order)",
                    CodeActionKind::REFACTOR,
                    replace_document_edit(&uri, &text, imported_text),
                ));
            }

            if supports_document_changes {
                actions.push(edit_action(
                    &format!("ipynb: Unpair (delete {})", script_path.display()),
                    CodeActionKind::REFACTOR,
                    WorkspaceEdit {
                        changes: None,
                        document_changes: Some(DocumentChanges::Operations(vec![
                            DocumentChangeOperation::Op(ResourceOp::Delete(DeleteFile {
                                uri: script_uri.clone(),
                                options: None,
                                
                            })),
                        ])),
                        change_annotations: None,
                    },
                ));
            }
        }

        Ok(Some(actions))
    }
}
