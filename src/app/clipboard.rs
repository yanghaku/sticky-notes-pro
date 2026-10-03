//! Text clipboard access through the window's existing Slint backend.
//!
//! On Wayland this reuses winit's display and its seat input serials via Slint's
//! built-in Smithay clipboard; no data-control extension or XWayland is needed.

use super::*;

/// Keep native error reporting on Windows/X11 while reusing Slint on Wayland.
#[derive(Default)]
pub(super) struct Clipboard {
    native: Option<arboard::Clipboard>,
}

impl Clipboard {
    fn native(&mut self) -> Result<&mut arboard::Clipboard> {
        if self.native.is_none() {
            self.native = Some(arboard::Clipboard::new().context("clipboard unavailable")?);
        }
        self.native.as_mut().context("clipboard unavailable")
    }

    /// Submit text; Wayland's backend does not expose write acknowledgements.
    pub(super) fn set_text(&mut self, window: &slint::Window, text: &str) -> Result<()> {
        if crate::platform::capabilities(window).is_wayland {
            crate::platform::set_clipboard_text(window, text).context("cannot copy text")
        } else {
            self.native()?.set_text(text).context("cannot copy text")
        }
    }

    pub(super) fn text(&mut self, window: &slint::Window) -> Result<String> {
        if crate::platform::capabilities(window).is_wayland {
            crate::platform::clipboard_text(window)?
                .context("clipboard has no text or the native clipboard is unavailable")
        } else {
            self.native()?.get_text().context("clipboard has no text")
        }
    }
}

struct CutRequest {
    window: slint::Weak<NoteWindow>,
    note_id: String,
    text: String,
    revision: u64,
    selection: (crate::editor::Position, crate::editor::Position),
}

impl App {
    /// Check after the compositor has had a chance to announce the new offer.
    pub(super) fn check_wayland_cut(&self, shared: &Rc<RefCell<Self>>, id: &str, text: String) {
        let Some(panel) = self.notes.get(id) else {
            return;
        };
        let Some(editor) = &panel.editor else {
            return;
        };
        Self::schedule_cut_check(
            shared,
            CutRequest {
                window: panel.ui.as_weak(),
                note_id: id.to_owned(),
                text,
                revision: editor.revision(),
                selection: editor.selection(),
            },
            3,
        );
    }

    fn schedule_cut_check(shared: &Rc<RefCell<Self>>, request: CutRequest, attempts: u8) {
        let weak = Rc::downgrade(shared);
        slint::Timer::single_shot(Duration::from_millis(30), move || {
            dispatch(&weak, |app, shared| {
                app.verify_wayland_cut(shared, request, attempts)
            });
        });
    }

    fn verify_wayland_cut(
        &mut self,
        shared: &Rc<RefCell<Self>>,
        request: CutRequest,
        attempts: u8,
    ) -> Result<()> {
        let Some(original_ui) = request.window.upgrade() else {
            return Ok(());
        };
        let Some(panel) = self.notes.get(&request.note_id) else {
            return Ok(());
        };
        let Some(editor) = &panel.editor else {
            return Ok(());
        };
        // A pending cut must never remove later edits, another selection, IME
        // composition, or a freshly reopened window that happens to share an id.
        if !std::ptr::eq(panel.ui.window(), original_ui.window())
            || panel.ui.get_frozen()
            || !panel.preedit.is_empty()
            || panel.waiting_ime
            || editor.revision() != request.revision
            || editor.selection() != request.selection
        {
            return Ok(());
        }

        // Smithay reads the compositor's current offer, not its local store
        // buffer. An identical older offer is safe: it already holds this text.
        // TODO: Slint/Smithay's load blocks without a timeout. Delaying/retrying
        // avoids racing an offer update but does not make this read asynchronous.
        let verified = self.clipboard.text(panel.ui.window());
        if matches!(&verified, Ok(text) if text == &request.text) {
            if let Some(panel) = self.notes.get_mut(&request.note_id)
                && let Some(editor) = &mut panel.editor
                && editor.backspace()
            {
                panel.changed();
                self.list_dirty = true;
                self.list_dirty_rows.insert(request.note_id);
            }
            return Ok(());
        }
        if attempts > 1 {
            Self::schedule_cut_check(shared, request, attempts - 1);
            return Ok(());
        }
        match verified {
            Err(error) => Err(error.context("未能确认剪贴板写入，内容未删除")),
            Ok(_) => anyhow::bail!("未能确认剪贴板写入，内容未删除"),
        }
    }
}
