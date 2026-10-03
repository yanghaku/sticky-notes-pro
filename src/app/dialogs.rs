//! Destructive-action confirmations and recovery.

use super::*;

impl App {
    pub(super) fn confirm_permanent_delete(
        &mut self,
        shared: &Rc<RefCell<Self>>,
        id: &str,
    ) -> Result<()> {
        if let Some((_, ui)) = &self.permanent_dialog {
            platform::activate(ui.window());
            return Ok(());
        }
        if self.pending_notes.contains(id) || self.window_for_note(id).is_some() {
            return Ok(());
        }
        if !self.list.as_ref().is_some_and(|ui| ui.get_deleted_view()) {
            return Ok(());
        }
        let Some(card) = self
            .list_rows
            .get(id)
            .and_then(|&row| self.list_cards.row_data(row))
            .filter(|card| card.id == id)
        else {
            return Ok(());
        };
        let ui = PermanentDeleteDialog::new()?;
        self.configure_appearance(&ui);
        ui.set_preview(card.preview);
        self.permanent_dialog = Some((id.to_owned(), ui.clone_strong()));
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        ui.on_confirm(move || {
            dispatch(&weak, |app, _| app.submit_permanent_delete(&note_id));
        });
        let weak = Rc::downgrade(shared);
        ui.on_cancel(move || dispatch(&weak, |app, _| app.cancel_permanent_delete()));
        let weak = Rc::downgrade(shared);
        ui.window().on_close_requested(move || {
            dispatch(&weak, |app, _| app.cancel_permanent_delete());
            slint::CloseRequestResponse::KeepWindowShown
        });
        if let Err(error) = ui.show() {
            // Without a window the dialog is unreachable but still counted as open,
            // which would keep the application from ever exiting.
            self.permanent_dialog = None;
            return Err(error).context("cannot show the permanent delete dialog");
        }
        Ok(())
    }

    pub(super) fn submit_permanent_delete(&mut self, id: &str) -> Result<()> {
        let Some((target, ui)) = &self.permanent_dialog else {
            return Ok(());
        };
        if target != id || ui.get_busy() {
            return Ok(());
        }
        if self.pending_notes.contains(id) || self.window_for_note(id).is_some() {
            ui.set_error("这张便签正在进行其他操作，请稍后重试或取消。".into());
            return Ok(());
        }
        match self.worker.submit(Job::Purge { id: id.to_owned() }) {
            Ok(()) => {
                ui.set_busy(true);
                ui.set_error("".into());
                self.pending_notes.insert(id.to_owned());
                self.list_dirty_rows.insert(id.to_owned());
                self.list_dirty = true;
            }
            Err(error) => ui.set_error(format!("暂时无法彻底删除，请重试：{error}").into()),
        }
        Ok(())
    }

    pub(super) fn cancel_permanent_delete(&mut self) -> Result<()> {
        if self
            .permanent_dialog
            .as_ref()
            .is_some_and(|(_, ui)| ui.get_busy())
        {
            return Ok(());
        }
        if let Some((_, ui)) = self.permanent_dialog.take() {
            ui.hide()?;
        }
        self.check_exit()
    }

    pub(super) fn confirm_discard(&mut self, shared: &Rc<RefCell<Self>>, id: &str) -> Result<()> {
        if self.discard.is_some() {
            return Ok(());
        }
        if self.notes.get(id).is_some_and(|panel| panel.in_flight) {
            return Err(anyhow::anyhow!("请等待当前保存完成后，再确认放弃修改"));
        }
        if let Some(panel) = self.notes.get_mut(id) {
            panel.discarding = true;
            panel.ui.set_frozen(true);
            panel.set_ime_enabled(false);
        }
        let dialog = DiscardDialog::new()?;
        self.configure_appearance(&dialog);
        self.discard = Some(dialog.clone_strong());
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        dialog.on_confirm(move || {
            dispatch(&weak, |app, _| {
                if app.notes.get(&note_id).is_some_and(|panel| panel.in_flight) {
                    return Err(anyhow::anyhow!("当前保存尚未结束，暂时不能放弃修改"));
                }
                if let Some(panel) = app.notes.get_mut(&note_id) {
                    let state = platform::capture(panel.ui.window(), &panel.state);
                    let job = if panel.persisted {
                        Job::CloseWindow { state }
                    } else {
                        Job::CloseDraft {
                            id: note_id.clone(),
                        }
                    };
                    app.worker
                        .submit(job)
                        .map_err(|error| anyhow::anyhow!("cannot queue discard close: {error}"))?;
                    panel.in_flight = true;
                    panel.destructive_queued = true;
                }
                if let Some(dialog) = app.discard.take() {
                    dialog.hide()?;
                }
                Ok(())
            })
        });
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        dialog.on_cancel(move || {
            dispatch(&weak, |app, _| {
                if let Some(dialog) = app.discard.take() {
                    dialog.hide()?;
                }
                app.cancel_discard(&note_id);
                Ok(())
            })
        });
        let weak = Rc::downgrade(shared);
        let note_id = id.to_owned();
        dialog.window().on_close_requested(move || {
            dispatch(&weak, |app, _| {
                if let Some(dialog) = app.discard.take() {
                    dialog.hide()?;
                }
                app.cancel_discard(&note_id);
                Ok(())
            });
            slint::CloseRequestResponse::KeepWindowShown
        });
        if let Err(error) = dialog.show() {
            if let Some(dialog) = self.discard.take() {
                dialog.hide()?;
            }
            self.cancel_discard(id);
            return Err(error).context("cannot show the discard dialog");
        }
        Ok(())
    }

    pub(super) fn cancel_discard(&mut self, id: &str) {
        if let Some(panel) = self.notes.get_mut(id) {
            panel.discarding = false;
            panel.ui.set_frozen(false);
            panel.set_ime_enabled(panel.editor.is_some());
        }
    }
}
