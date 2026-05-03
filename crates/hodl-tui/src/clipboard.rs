//! Thin wrapper around [`hjkl_clipboard::Clipboard`].
//!
//! Decouples the rest of the UI from the clipboard backend — callers
//! only see `yank(text)`. Errors are swallowed and logged via `tracing`
//! so a clipboard failure never crashes the TUI.

use hjkl_clipboard::{Clipboard, MimeType, Selection};
use tracing::debug;

pub struct ClipboardHandle {
    inner: Clipboard,
}

impl ClipboardHandle {
    /// Probe the best available backend (Wayland → X11 → OSC 52).
    pub fn new() -> anyhow::Result<Self> {
        let inner = Clipboard::new().map_err(|e| anyhow::anyhow!("clipboard init: {e}"))?;
        Ok(Self { inner })
    }

    /// Write `text` to the system clipboard (selection = Clipboard).
    /// Uses OSC 52 automatically when the active backend is the fallback,
    /// which works correctly over SSH sessions.
    pub fn yank(&self, text: &str) {
        if let Err(e) = self
            .inner
            .set(Selection::Clipboard, MimeType::Text, text.as_bytes())
        {
            debug!("clipboard yank failed: {e}");
        }
    }
}
