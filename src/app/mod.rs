//! Application ownership and startup. Dedicated modules implement each interaction.

mod appearance;
mod clipboard;
mod colors;
mod dialogs;
mod formatting;
mod ime;
mod input;
mod list;
mod maintenance;
mod notes;
mod panel;
mod rendering;
mod replies;
mod settings;
mod window_state;

use colors::ColorDraft;
use ime::ImeState;
use window_state::{AuxiliaryWindowState, WindowSaveTimer};

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use winit::{
    event::{ElementState, Ime, WindowEvent},
    keyboard::{Key, ModifiersState, NamedKey},
};

use crate::{
    AppAppearance, DiscardDialog, NoteCard, NoteWindow, NotesListWindow, PermanentDeleteDialog,
    SettingsWindow,
    editor::Editor,
    instance::Instance,
    model::{Document, Note, THEMES},
    persistence::{
        AppearanceMode, Paths, Repository, Settings, StoredNote, WindowState, now_millis,
    },
    platform,
    shortcuts::{self, NoteShortcut},
    text_layout::{TextLayout, TextResources},
    worker::{Job, ReplyKind, SaveAction, Worker},
};

/// Editable text lives only in Editor; save jobs receive an owned snapshot.
struct NoteMetadata {
    id: Option<String>,
    theme_id: String,
    created_at: i64,
    updated_at: i64,
    deleted_at: Option<i64>,
}

struct Panel {
    ui: NoteWindow,
    /// Metadata is absent only for damaged, read-only notes; drafts have no ID yet.
    note: Option<NoteMetadata>,
    persisted: bool,
    editor: Option<Editor>,
    fallback: String,
    state: WindowState,
    layout: TextLayout,
    preedit: String,
    ime: ImeState,
    modifiers: ModifiersState,
    scroll: f32,
    revision: u64,
    saved_revision: u64,
    in_flight: bool,
    destructive_queued: bool,
    pending_action: Option<SaveAction>,
    waiting_ime: bool,
    ime_wait_started: Option<Instant>,
    dirty_since: Option<Instant>,
    last_edit: Instant,
    last_submit: Instant,
    render_dirty: bool,
    follow_caret: bool,
    state_dirty: Option<Instant>,
    state_in_flight: bool,
    /// Only restore a failed window-state banner if no newer status replaced it.
    state_failure_status: Option<slint::SharedString>,
    state_save_timer: WindowSaveTimer,
    discarding: bool,
    view: (f32, f32, f32),
    restored: bool,
}

struct App {
    paths: Paths,
    repository: Repository,
    worker: Worker,
    instance: Instance,
    settings: Settings,
    settings_writable: bool,
    settings_warning: Option<String>,
    dark: Option<bool>,
    /// Runtime window keys remain stable when a draft receives its persisted ID.
    notes: HashMap<String, Panel>,
    list: Option<NotesListWindow>,
    settings_ui: Option<SettingsWindow>,
    color_draft: Option<ColorDraft>,
    discard: Option<DiscardDialog>,
    permanent_dialog: Option<(String, PermanentDeleteDialog)>,
    font_families: Vec<String>,
    text_resources: Rc<RefCell<TextResources>>,
    pending_notes: HashSet<String>,
    last_active: Option<String>,
    clipboard: clipboard::Clipboard,
    list_dirty: bool,
    list_reload: bool,
    list_dirty_rows: HashSet<String>,
    list_cards: Rc<VecModel<NoteCard>>,
    list_rows: HashMap<String, usize>,
    list_next_refresh: Instant,
    closing_list: bool,
    closing_settings: bool,
    restore_list: Option<WindowState>,
    restore_settings: Option<WindowState>,
    list_state: Option<AuxiliaryWindowState>,
    settings_state: Option<AuxiliaryWindowState>,
}

pub fn run() -> Result<()> {
    let paths = Paths::discover()?;
    let Some(instance) = Instance::acquire(&paths.data_dir)? else {
        return Ok(());
    };
    let loaded = Settings::load(&paths.settings_file)?;
    let repository = Repository::open(&paths.database_file)?;
    let worker = Worker::new(&paths.database_file)?;
    let text_resources = Rc::new(RefCell::new(TextResources::new()));
    let mut font_families = vec![String::new()];
    font_families.extend(text_resources.borrow().available_font_families());
    let app = Rc::new(RefCell::new(App {
        paths,
        repository,
        worker,
        instance,
        settings: loaded.settings,
        settings_writable: loaded.writable,
        settings_warning: loaded.warning,
        dark: None,
        notes: HashMap::new(),
        list: None,
        settings_ui: None,
        color_draft: None,
        discard: None,
        permanent_dialog: None,
        font_families,
        text_resources,
        pending_notes: HashSet::new(),
        last_active: None,
        clipboard: clipboard::Clipboard::default(),
        list_dirty: true,
        list_reload: true,
        list_dirty_rows: HashSet::new(),
        list_cards: Rc::new(VecModel::default()),
        list_rows: HashMap::new(),
        list_next_refresh: Instant::now(),
        closing_list: false,
        closing_settings: false,
        restore_list: None,
        restore_settings: None,
        list_state: None,
        settings_state: None,
    }));
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .with_winit_custom_application_handler(rendering::RenderCoordinator(Rc::downgrade(&app)))
        .select()?;
    let startup = app.borrow().repository.startup_note()?;
    if let Some(note) = startup {
        App::open_note(&app, note)?;
    } else {
        App::new_note(&app)?;
    }
    let timer = slint::Timer::default();
    let weak = Rc::downgrade(&app);
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(50),
        move || {
            dispatch(&weak, |app, shared| app.tick(shared));
        },
    );
    slint::run_event_loop()?;
    timer.stop();
    Ok(())
}

fn dispatch(weak: &Weak<RefCell<App>>, f: impl FnOnce(&mut App, &Rc<RefCell<App>>) -> Result<()>) {
    let Some(shared) = weak.upgrade() else {
        return;
    };
    let Ok(mut app) = shared.try_borrow_mut() else {
        // A callback fired while App is borrowed would otherwise vanish without a
        // trace: the window silently ignores its own close or save request.
        eprintln!("application callback dropped: App is already borrowed");
        return;
    };
    if let Err(error) = f(&mut app, &shared) {
        app.report(&format!("{error:#}"));
    }
    app.schedule_render();
}

impl App {
    fn window_for_note(&self, note_id: &str) -> Option<&str> {
        self.notes.iter().find_map(|(key, panel)| {
            (panel.state.note_id.as_deref() == Some(note_id)).then_some(key.as_str())
        })
    }

    fn reply_window(&self, id: &str) -> Option<String> {
        if self.notes.contains_key(id) {
            Some(id.to_owned())
        } else {
            self.window_for_note(id.strip_prefix("note:").unwrap_or(id))
                .map(str::to_owned)
        }
    }

    fn report(&self, message: &str) {
        eprintln!("{message}");
        if let Some(list) = &self.list {
            list.set_error(message.into());
        } else if let Some(panel) = self.last_active.as_ref().and_then(|id| self.notes.get(id)) {
            panel.ui.set_error(message.into());
        }
    }

    fn remove_note(&mut self, id: &str) -> Result<()> {
        self.pending_notes.remove(id);
        if let Some(panel) = self.notes.remove(id) {
            if let Some(note_id) = &panel.state.note_id {
                self.pending_notes.remove(note_id);
            }
            panel.ui.hide()?;
        }
        self.list_dirty = true;
        self.list_reload = true;
        self.check_exit()
    }

    fn check_exit(&self) -> Result<()> {
        if self.notes.is_empty()
            && self.list.is_none()
            && self.settings_ui.is_none()
            && self.permanent_dialog.is_none()
        {
            slint::quit_event_loop()?;
        }
        Ok(())
    }
}
