//! File-pairing engine: keeps a notebook (.ipynb) and its paired percent
//! script (.nb.py) in sync.
//!
//! Policy (jupytext-style, two-way):
//! - Notebook changed (script untouched) -> regenerate the script. Lossless.
//! - Script changed (notebook untouched) -> import the script into the
//!   notebook, merging back outputs / execution counts / metadata from the
//!   notebook's cells by ordinal, so execution results are not lost.
//! - Both changed since the last sync -> pair is divergent; the user resolves
//!   it explicitly via code actions ("Sync notebook -> script" or
//!   "Sync script -> notebook"). Nothing is written in that state.
//!
//! Single-writer rule: all writes for a pair happen under its state mutex.

use tower_lsp::lsp_types;
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};
use notify::{RecursiveMode, Watcher};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::nbformat;
use crate::percent;

pub struct PairState {
    pub script_path: PathBuf,
    pub nb_hash: u64,
    pub script_hash: u64,
    pub diverged: bool,
    pub cancelled: Arc<AtomicBool>,
    _watcher: notify::RecommendedWatcher,
}

pub type PairMap = Arc<Mutex<HashMap<PathBuf, Arc<Mutex<PairState>>>>>;

pub const DIVERGENCE_CODE: &str = "pair-divergence";

pub async fn start_pair(
    pairs: PairMap,
    notebook_path: PathBuf,
    on_event: impl Fn(PairUpdate) + Send + 'static,
) -> Result<(), String> {
    let script_path = percent::script_path_for(&notebook_path);

    let mut map = pairs.lock().await;
    if map.contains_key(&notebook_path) {
        return Ok(());
    }

    let watch_dir = notebook_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let (watcher, mut events) =
        spawn_watcher(watch_dir).map_err(|e| format!("failed to start file watcher: {e}"))?;

    let (nb_hash, script_hash) = tokio::task::spawn_blocking({
        let nb = notebook_path.clone();
        let script = script_path.clone();
        move || (hash_of(&nb), hash_of(&script))
    })
    .await
    .map_err(|e| e.to_string())?;

    let cancelled = Arc::new(AtomicBool::new(false));
    let state = Arc::new(Mutex::new(PairState {
        script_path: script_path.clone(),
        nb_hash,
        script_hash,
        diverged: false,
        cancelled: cancelled.clone(),
        _watcher: watcher,
    }));
    map.insert(notebook_path.clone(), state);

    // Sync loop: settle file-event bursts (300ms of quiet), then evaluate
    // both files by content hash. Only events that touch the notebook, the
    // script, or their directory matter. All writes for this pair happen in
    // evaluate_pair under the state mutex (single-writer rule).
    let nb_path = notebook_path.clone();
    let script_path = script_path.clone();
    tokio::spawn(async move {
        while let Some(paths) = events.recv().await {
            if !paths.iter().any(|p| p == &nb_path || p == &script_path) {
                continue;
            }

            if cancelled.load(Ordering::Relaxed) {
                break;
            }

            // Settle window: wait until the burst ends, ignoring unrelated
            // events, so a rapid burst of edits triggers one evaluation.
            while let Ok(Some(more)) =
                tokio::time::timeout(std::time::Duration::from_millis(300), events.recv()).await
            {
                if !more.iter().any(|p| p == &nb_path || p == &script_path) {
                    continue;
                }
            }

            on_event(PairUpdate { notebook_path: nb_path.clone() });
        }
    });

    Ok(())
}

/// Fired after a file event for a notebook.
#[derive(Debug)]
pub struct PairUpdate {
    pub notebook_path: PathBuf,
}

/// Evaluates a pair after file changes; returns diagnostics to publish
/// (empty = no pair-level problems).
pub async fn evaluate_pair(
    pair: &Mutex<PairState>,
    notebook_path: &Path,
) -> Vec<lsp_types::Diagnostic> {


    let mut state = pair.lock().await;

    let nb_text = match tokio::fs::read_to_string(notebook_path).await {
        Ok(text) => text,
        Err(_) => return Vec::new(), // notebook gone; nothing to do
    };
    let script_text = tokio::fs::read_to_string(&state.script_path).await.ok();

    let Some(script) = script_text else {
        state.diverged = false;
        return Vec::new();
    };

    let new_nb_hash = hash_str(&nb_text);
    let new_script_hash = hash_str(&script);

    let nb_changed = new_nb_hash != state.nb_hash;
    let script_changed = new_script_hash != state.script_hash;
    eprintln!("EVAL: nb_changed={} script_changed={} script_len={}", nb_changed, script_changed, script.len());

    if !nb_changed && !script_changed {
        state.diverged = false;
        return Vec::new();
    }

    if nb_changed && !script_changed {
        // Notebook is the fresh side -> regenerate the script.
        if let Ok(nb_value) = nbformat::parse(&nb_text) {
            if let Ok(new_script) = percent::notebook_to_percent(&nb_value) {
                if new_script != script
                    && tokio::fs::write(&state.script_path, new_script.as_bytes())
                        .await
                        .is_ok()
                {
                    state.script_hash = hash_str(&new_script);
                }
                state.nb_hash = new_nb_hash;
                state.diverged = false;
            }
        }
        return Vec::new();
    }

    if script_changed && !nb_changed {
        // Script is the fresh side -> import into the notebook with a
        // merge that preserves outputs, execution counts and metadata.
        let current_nb = nbformat::parse(&nb_text).ok();
        let imported = percent::percent_to_notebook(&script, current_nb.as_ref().and_then(|nb| nb.get("metadata"))).ok();

        let script_cells = imported.as_ref().and_then(|nb| nb.get("cells")).and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        let current_cells = current_nb.as_ref().and_then(|nb| nb.get("cells")).and_then(Value::as_array).map(Vec::len).unwrap_or(0);

        if script_cells == 0 && current_cells > 0 {
            // An empty script must never wipe a non-empty notebook.
            state.script_hash = new_script_hash;
            state.diverged = true;
            return vec![divergence_diagnostic(
                "Paired script became empty; refusing to overwrite the notebook. \
                 Use code actions to resolve.",
                &state.script_path,
            )];
        }

        if let (Some(imported), Some(current)) = (&imported, &current_nb) {
            let merged = percent::merge_import_with_notebook(imported, current);
            let text = nbformat::serialize_pretty(&merged);
            if text != nb_text
                && tokio::fs::write(notebook_path, text.as_bytes()).await.is_ok()
            {
                state.nb_hash = hash_str(&text);
            }
            state.script_hash = new_script_hash;
            state.diverged = false;
            return Vec::new();
        }

        state.diverged = true;
        return vec![divergence_diagnostic(
            "Failed to import paired script; pair is divergent. Use code actions to resolve.",
            &state.script_path,
        )];
    }

    // Both sides changed -> divergence.
    state.nb_hash = new_nb_hash;
    state.script_hash = new_script_hash;
    state.diverged = true;
    vec![divergence_diagnostic(
        "Notebook and paired script changed at the same time. Use code actions to resolve.",
        &state.script_path,
    )]
}

fn divergence_diagnostic(message: &str, script_path: &Path) -> Diagnostic {
    Diagnostic {
        range: Range::new(Position::new(0, 0), Position::new(0, 0)),
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String(DIVERGENCE_CODE.to_string())),
        source: Some(nbformat::SOURCE.to_string()),
        message: format!("{message} ({})", script_path.display()),
        ..Diagnostic::default()
    }
}

/// Removes a pair (stops the watcher).
pub async fn remove_pair(pairs: &PairMap, notebook_path: &Path) {
    let mut map = pairs.lock().await;
    if let Some(state) = map.remove(notebook_path) {
        state.lock().await.cancelled.store(true, Ordering::Relaxed);
    }
}

// -- helpers ----------------------------------------------------------------

fn spawn_watcher(
    dir: PathBuf,
) -> Result<
    (
        notify::RecommendedWatcher,
        tokio::sync::mpsc::Receiver<Vec<PathBuf>>,
    ),
    notify::Error,
> {
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // Only content-level changes matter. The default inotify mask includes
        // OPEN, which would fire on our own reads and create a loop.
        let Ok(event) = event else { return };
        let relevant = matches!(
            event.kind,
            notify::EventKind::Modify(_) | notify::EventKind::Create(_) | notify::EventKind::Remove(_)
        );
        if !relevant {
            return;
        }
        let _ = tx.try_send(event.paths);
    })?;
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;
    Ok((watcher, rx))
}

fn hash_of(path: &Path) -> u64 {
    match std::fs::read_to_string(path) {
        Ok(text) => hash_str(&text),
        Err(_) => 0,
    }
}

pub fn hash_str(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Convenience: file:// Url for a path (for diagnostics on scripts).
pub fn uri_for_path(path: &Path) -> Option<Url> {
    Url::from_file_path(path).ok()
}
