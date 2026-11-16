//! L'application : les vidéos à gauche, ce qu'on en fait à droite, et les fils de travail derrière.
//!
//! L'interface ne fait que poser des `Action` ; `App::run_actions` les applique en fin d'image.
//! Les fils (analyse, encodage, téléchargements) répondent par des messages relevés dans `poll`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use egui::{Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::draw;

use crate::engine::{self, EncodeError, VIDEO_EXTS};
use crate::i18n::{self, Lang, tf, tr};
use crate::icons::{self, Icon};
use crate::instance::Waker;
use crate::player::Player;
use crate::queue::{self, Item, Job, Msg, State};
use crate::release::{self, Phase as UpdatePhase, Updater, VERSION};
use crate::settings::{Audio, Container, Encode, Fps, Kind as OutputKind, Mode, Resolution, Settings, Speed, VideoCodec, presets};
use crate::shell;
use crate::theme::{self, Tokens};
use crate::widgets::{self, Kind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    Prefs,
    Update,
}

pub enum Action {
    PickFiles,
    Add(Vec<PathBuf>),
    Remove(u64),
    Select(u64),
    ToggleFine,
    /// Lire la vidéo choisie dans l'aperçu, ou la mettre en pause.
    TogglePlay(u64),
    /// Sauter à cet instant de la vidéo lue, en secondes.
    SeekTo(f64),
    Retry(u64),
    Start,
    Cancel,
    Reveal(PathBuf),
    Open(PathBuf),
    Copy(String),
    MenuOn,
    MenuOff,
    DismissMenuPrompt,
    PickOutDir,
    Show(Dialog),
    CloseDialog,
    CheckUpdate,
    DownloadUpdate,
    Restart,
    Language(Option<Lang>),
}

pub(crate) struct Toast {
    icon: Icon,
    text: String,
    until: f64,
}

/// Le lot en cours : de quoi l'arrêter, et où il en est.
pub(crate) struct Batch {
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) total: usize,
    pub(crate) finished: usize,
    pub(crate) ok: usize,
    pub(crate) failed: usize,
    /// Les octets gagnés (négatif si les fichiers produits sont plus lourds).
    pub(crate) saved: i64,
}

pub struct App {
    pub settings: Settings,
    saved: Settings,
    pub items: Vec<Item>,
    next_id: u64,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pub(crate) batch: Option<Batch>,
    pub updater: Updater,
    pub menu_on: bool,
    inbox: Option<Receiver<Vec<PathBuf>>>,
    pub dialog: Option<Dialog>,
    /// La vidéo montrée en grand.
    pub selected: Option<u64>,
    /// Les réglages fins sont-ils dépliés ?
    pub fine_open: bool,
    /// Le lecteur de l'aperçu, quand la vidéo choisie est en lecture.
    pub(crate) player: Option<Player>,
    pub(crate) toast: Option<Toast>,
    pub(crate) actions: Vec<Action>,
    now: f64,
    /// En test : rien n'est lancé, lu ni écrit en dehors de la fenêtre.
    pub dry_run: bool,
}

impl App {
    /// Le démarrage réel : réglages enregistrés, mise à jour vérifiée, fichiers reçus du clic droit.
    pub fn new(cc: &eframe::CreationContext<'_>, inbox: Option<(Receiver<Vec<PathBuf>>, Waker)>, paths: Vec<PathBuf>) -> Self {
        let settings = Settings::load();
        i18n::set(settings.language.unwrap_or_else(i18n::detect));
        let mut app = Self::with_settings(cc, settings, false);
        if let Some((rx, waker)) = inbox {
            waker.set(&cc.egui_ctx);
            app.inbox = Some(rx);
        }
        release::cleanup_old_exes();
        if std::env::args().any(|a| a == release::AFTER_UPDATE) {
            app.notify(Icon::PackageCheck, tf("Video Compressor {VERSION} est installé", &[("VERSION", &VERSION)]));
        }
        // Une seule question à GitHub au démarrage : y a-t-il une version plus récente ?
        if app.settings.update_auto {
            app.updater.check(&cc.egui_ctx, app.settings.update_beta);
        }
        app.menu_on = shell::menu_registered();
        app.add_paths(&cc.egui_ctx, paths);
        app
    }

    /// Une application sur des réglages donnés. `dry_run` : pour les tests.
    pub fn with_settings(cc: &eframe::CreationContext<'_>, settings: Settings, dry_run: bool) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, settings.accent);
        let (tx, rx) = channel();
        Self {
            saved: settings.clone(),
            settings,
            items: Vec::new(),
            next_id: 1,
            tx,
            rx,
            batch: None,
            updater: Updater::default(),
            menu_on: false,
            inbox: None,
            dialog: None,
            selected: None,
            fine_open: false,
            player: None,
            toast: None,
            actions: Vec::new(),
            now: 0.0,
            dry_run,
        }
    }

    pub(crate) fn notify(&mut self, icon: Icon, text: impl Into<String>) {
        self.toast = Some(Toast { icon, text: text.into(), until: self.now + 5.0 });
    }

    pub fn running(&self) -> bool {
        self.batch.is_some()
    }

    /// Ajoute des fichiers à la liste (ceux d'un dossier, s'il y en a un), sans doublon.
    pub fn add_paths(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        let mut first_new = None;
        for path in queue::expand(paths) {
            if let Some(existing) = self.items.iter_mut().find(|i| i.path == path) {
                // Redemandé après avoir été traité : il repart avec les réglages du moment.
                if matches!(existing.state, State::Done { .. }) {
                    existing.state = State::Ready;
                }
                continue;
            }
            let item = Item::new(self.next_id, path);
            self.next_id += 1;
            first_new.get_or_insert(item.id);
            if !self.dry_run {
                queue::spawn_probe(item.id, item.path.clone(), self.tx.clone(), ctx.clone());
            }
            self.items.push(item);
        }
        // La première des nouvelles venues passe en grand.
        if first_new.is_some() {
            self.selected = first_new;
        }
    }

    /// Lance l'encodage de tous les fichiers prêts, avec les réglages du moment.
    fn start(&mut self, ctx: &egui::Context) {
        if self.batch.is_some() {
            return;
        }
        let mut taken = Vec::new();
        let mut jobs = Vec::new();
        for item in self.items.iter_mut().filter(|i| i.encodable()) {
            let Some(info) = item.info.clone() else { continue };
            let output = queue::output_path(&item.path, &self.settings.encode, &self.settings.out_dir, &self.settings.suffix, &taken);
            taken.push(output.clone());
            item.state = State::Queued;
            jobs.push(Job { id: item.id, input: item.path.clone(), info, output });
        }
        if jobs.is_empty() {
            return;
        }
        let total = jobs.len();
        let cancel = queue::spawn_batch(self.settings.encode, jobs, self.tx.clone(), ctx.clone());
        self.batch = Some(Batch { cancel, total, finished: 0, ok: 0, failed: 0, saved: 0 });
    }

    /// Relève ce que les fils de travail ont envoyé depuis la dernière image.
    fn poll(&mut self, ctx: &egui::Context) {
        let received: Vec<Vec<PathBuf>> = self.inbox.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for paths in received {
            self.add_paths(ctx, paths);
        }
        let msgs: Vec<Msg> = self.rx.try_iter().collect();
        for msg in msgs {
            match msg {
                Msg::Probed { id, result, thumb } => {
                    let Some(item) = self.items.iter_mut().find(|i| i.id == id) else { continue };
                    match result {
                        Ok(info) => {
                            item.info = Some(info);
                            item.state = State::Ready;
                            item.thumb = thumb.map(|(w, h, rgba)| ctx.load_texture(format!("thumb-{id}"), egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba), egui::TextureOptions::LINEAR));
                        }
                        Err(e) => item.state = State::Failed(e),
                    }
                }
                Msg::Started { id } => {
                    if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
                        item.state = State::Running { fraction: 0.0, speed: 0.0, started: Instant::now() };
                    }
                }
                Msg::Progress { id, fraction, speed } => {
                    if let Some(item) = self.items.iter_mut().find(|i| i.id == id)
                        && let State::Running { fraction: f, speed: s, .. } = &mut item.state
                    {
                        (*f, *s) = (fraction, speed);
                    }
                }
                Msg::Finished { id, result } => {
                    let item = self.items.iter_mut().find(|i| i.id == id);
                    let before = item.as_ref().and_then(|i| i.info.as_ref()).map(|i| i.size).unwrap_or(0);
                    if let Some(batch) = &mut self.batch {
                        batch.finished += 1;
                        match &result {
                            Ok((_, size)) => {
                                batch.ok += 1;
                                batch.saved += before as i64 - *size as i64;
                            }
                            Err(EncodeError::Failed(_)) => batch.failed += 1,
                            Err(EncodeError::Cancelled) => {}
                        }
                    }
                    if let Some(item) = item {
                        item.state = match result {
                            Ok((output, size)) => State::Done { output, size },
                            Err(EncodeError::Failed(e)) => State::Failed(e),
                            Err(EncodeError::Cancelled) => State::Ready,
                        };
                    }
                }
                Msg::BatchDone => {
                    for item in &mut self.items {
                        if matches!(item.state, State::Queued | State::Running { .. }) {
                            item.state = State::Ready;
                        }
                    }
                    if let Some(batch) = self.batch.take() {
                        let (icon, text) = match (batch.ok, batch.failed) {
                            (0, 0) => (Icon::CircleSlash, tr("Compression annulée").to_owned()),
                            (0, _) => (Icon::TriangleAlert, tr("Aucun fichier n'a pu être traité").to_owned()),
                            (ok, _) if batch.saved > 0 => (Icon::CircleCheck, tf("{n} fichiers traités, {size} gagnés", &[("n", &ok), ("size", &i18n::size(batch.saved as u64))])),
                            (ok, _) => (Icon::CircleCheck, tf("{n} fichiers traités", &[("n", &ok)])),
                        };
                        self.notify(icon, text);
                    }
                }
            }
        }
        self.updater.poll();
        self.settings.encode.normalize();
        if self.settings != self.saved {
            if self.settings.accent != self.saved.accent {
                theme::apply(ctx, self.settings.accent);
            }
            self.saved = self.settings.clone();
            if !self.dry_run
                && let Err(e) = self.settings.save()
            {
                self.notify(Icon::TriangleAlert, tf("Les réglages n'ont pas pu être enregistrés ({0})", &[("0", &e)]));
            }
        }
    }

    fn run_actions(&mut self, ctx: &egui::Context) {
        for action in std::mem::take(&mut self.actions) {
            match action {
                Action::PickFiles => {
                    if self.dry_run {
                        continue;
                    }
                    let picked = rfd::FileDialog::new().set_title(tr("Choisir des vidéos")).add_filter(tr("Vidéos"), VIDEO_EXTS).add_filter(tr("Tous les fichiers"), &["*"]).pick_files();
                    if let Some(paths) = picked {
                        self.add_paths(ctx, paths);
                    }
                }
                Action::Add(paths) => self.add_paths(ctx, paths),
                Action::Remove(id) => {
                    let index = self.items.iter().position(|i| i.id == id);
                    self.items.retain(|i| i.id != id || matches!(i.state, State::Queued | State::Running { .. }));
                    if self.selected == Some(id) && !self.items.iter().any(|i| i.id == id) {
                        self.selected = index.and_then(|i| self.items.get(i).or(self.items.last())).map(|i| i.id);
                    }
                }
                Action::Select(id) => self.selected = Some(id),
                Action::ToggleFine => self.fine_open = !self.fine_open,
                Action::TogglePlay(id) => {
                    if self.dry_run {
                        continue;
                    }
                    match &self.player {
                        Some(player) if player.item == id => player.toggle(),
                        _ => {
                            let item = self.items.iter().find(|i| i.id == id);
                            self.player = item.and_then(|item| Some(Player::open(id, item.path.clone(), item.info.as_ref()?, ctx.clone())));
                        }
                    }
                }
                Action::SeekTo(seconds) => {
                    if let Some(player) = &self.player {
                        player.seek(seconds);
                    }
                }
                Action::Retry(id) => {
                    if let Some(item) = self.items.iter_mut().find(|i| i.id == id)
                        && item.info.is_some()
                    {
                        item.state = State::Ready;
                    }
                }
                Action::Start => self.start(ctx),
                Action::Cancel => {
                    if let Some(batch) = &self.batch {
                        batch.cancel.store(true, Ordering::Relaxed);
                    }
                }
                Action::Reveal(path) => {
                    if !self.dry_run {
                        shell::reveal(&path);
                    }
                }
                Action::Open(path) => {
                    if !self.dry_run {
                        shell::open(&path);
                    }
                }
                Action::Copy(text) => {
                    ctx.copy_text(text);
                    self.notify(Icon::ClipboardCheck, tr("Copié dans le presse-papiers"));
                }
                Action::MenuOn => {
                    if self.dry_run {
                        self.menu_on = true;
                        continue;
                    }
                    match shell::register_menu() {
                        Ok(()) => {
                            self.menu_on = true;
                            self.notify(Icon::CircleCheck, tr("Le clic droit est en place"));
                        }
                        Err(e) => self.notify(Icon::TriangleAlert, e),
                    }
                }
                Action::MenuOff => {
                    if self.dry_run {
                        self.menu_on = false;
                        continue;
                    }
                    match shell::unregister_menu() {
                        Ok(()) => self.menu_on = false,
                        Err(e) => self.notify(Icon::TriangleAlert, e),
                    }
                }
                Action::DismissMenuPrompt => self.settings.menu_prompt_dismissed = true,
                Action::PickOutDir => {
                    if self.dry_run {
                        continue;
                    }
                    if let Some(dir) = rfd::FileDialog::new().set_title(tr("Choisir le dossier des fichiers produits")).pick_folder() {
                        self.settings.out_dir = dir.display().to_string();
                    }
                }
                Action::Show(dialog) => {
                    self.dialog = Some(dialog);
                    if dialog == Dialog::Update && !self.dry_run && matches!(self.updater.phase, UpdatePhase::Idle | UpdatePhase::UpToDate | UpdatePhase::Failed) {
                        self.updater.check(ctx, self.settings.update_beta);
                    }
                }
                Action::CloseDialog => self.dialog = None,
                Action::CheckUpdate => {
                    if !self.dry_run {
                        self.updater.check(ctx, self.settings.update_beta);
                    }
                }
                Action::DownloadUpdate => {
                    if !self.dry_run {
                        self.updater.download(ctx, crate::settings::data_dir().join("updates"));
                    }
                }
                Action::Restart => {
                    // On met la nouvelle version en place (si elle est téléchargée), on relance
                    // l'exécutable, puis on ferme celui-ci.
                    if self.dry_run {
                        continue;
                    }
                    if let (UpdatePhase::Ready, Some(path)) = (self.updater.phase, self.updater.downloaded.clone())
                        && let Err(why) = release::install(&path)
                    {
                        self.notify(Icon::TriangleAlert, why);
                        continue;
                    }
                    match release::restart() {
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(why) => self.notify(Icon::TriangleAlert, why),
                    }
                }
                Action::Language(lang) => {
                    self.settings.language = lang;
                    if !self.dry_run {
                        i18n::set(lang.unwrap_or_else(i18n::detect));
                    }
                }
            }
        }
    }

    // ---- barres ------------------------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut Ui, on_edge: bool) {
        let t = Tokens::get(ui.ctx());
        if !on_edge {
            widgets::title_bar_drag(ui, ui.max_rect());
        }
        ui.painter().hline(ui.max_rect().x_range(), ui.max_rect().bottom() - 0.5, t.hairline());
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.add_space(14.0);
            // La marque : un petit carré, le triangle de lecture dans la couleur d'accent.
            let (mark, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
            ui.painter().rect(mark, CornerRadius::same(6), theme::rgb(0x1e1e1e), Stroke::new(1.0, theme::rgb(0x3a3a3a)), StrokeKind::Inside);
            let c = mark.center();
            ui.painter().add(egui::Shape::convex_polygon(vec![c + vec2(-3.0, -4.5), c + vec2(4.5, 0.0), c + vec2(-3.0, 4.5)], t.accent, Stroke::NONE));
            ui.add_space(2.0);
            ui.label(RichText::new("Video Compressor").font(FontId::new(13.0, theme::semibold())).color(t.ink_strong));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                widgets::window_buttons(ui, theme::TOPBAR_H);
                ui.add_space(6.0);
                ui.spacing_mut().item_spacing.x = 2.0;
                if widgets::icon_button(ui, Icon::SlidersHorizontal, tr("Préférences"), 30.0).clicked() {
                    self.actions.push(Action::Show(Dialog::Prefs));
                }
                // Mise à jour : un point d'accent quand une version attend.
                let latest = self.updater.release.as_ref().map(|r| r.version.clone()).unwrap_or_default();
                let (tip, pending) = match self.updater.phase {
                    UpdatePhase::Available => (tf("Mise à jour disponible, version {latest}", &[("latest", &latest)]), true),
                    UpdatePhase::Ready => (tr("Mise à jour prête, redémarrer pour l'appliquer").to_owned(), true),
                    UpdatePhase::Downloading => (tr("Téléchargement de la mise à jour…").to_owned(), false),
                    UpdatePhase::Checking => (tr("Recherche d'une nouvelle version…").to_owned(), false),
                    UpdatePhase::Failed => (tr("Vérification des mises à jour impossible").to_owned(), false),
                    _ => (tr("Mises à jour").to_owned(), false),
                };
                let resp = widgets::icon_button(ui, Icon::CircleArrowUp, &tip, 30.0);
                if pending {
                    ui.painter().circle_filled(resp.rect.right_top() + vec2(-7.0, 7.0), 4.0, t.accent);
                    ui.painter().circle_stroke(resp.rect.right_top() + vec2(-7.0, 7.0), 4.5, Stroke::new(1.5, t.bg));
                }
                if resp.clicked() {
                    self.actions.push(Action::Show(Dialog::Update));
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        ui.painter().hline(ui.max_rect().x_range(), ui.max_rect().top() + 0.5, t.hairline());
        let small = |text: String| RichText::new(text).size(11.5).color(t.muted);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.add_space(14.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            let (dot, _) = ui.allocate_exact_size(vec2(6.0, 6.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 3.0, t.accent);
            ui.label(small(tf("FFmpeg {0} intégré", &[("0", &engine::version())])));
            ui.add_space(6.0);
            widgets::vsep(ui, 12.0);
            ui.add_space(6.0);
            ui.label(small(if self.menu_on { tr("Clic droit actif") } else { tr("Clic droit inactif") }.to_owned()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(14.0);
                ui.label(RichText::new(tf("Version {VERSION}", &[("VERSION", &VERSION)])).font(widgets::mono(11.0)).color(t.faint));
            });
        });
    }

    // ---- colonne de gauche : les vidéos ------------------------------------------------------------

    fn videos(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let inner = ui.max_rect();
        let full = Rect::from_min_max(inner.min - vec2(PAD_X, PAD_TOP), inner.max + vec2(PAD_X, PAD_BOTTOM));
        {
            // La trame de points derrière le haut de la colonne, et le halo gris venu de la droite.
            let p = ui.painter().with_clip_rect(full);
            draw::dot_grid(&p, Rect::from_min_size(full.min, vec2(full.width(), full.height().min(380.0))), t.dot);
            draw::halo(&p, pos2(full.right() - 140.0, full.top() + 110.0), vec2(-0.469, 0.883), 340.0, 150.0, 12);
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let selected = self.selected.and_then(|id| self.items.iter().position(|i| i.id == id));
        let others = self.items.len().saturating_sub(usize::from(selected.is_some()));
        let prompt = !self.menu_on && !self.settings.menu_prompt_dismissed;

        // La hauteur de l'image : en 16/9 si la place le permet, sinon ce qui reste.
        let width = ui.available_width() - 8.0;
        let below = if selected.is_some() { 118.0 } else { 0.0 } + if others > 0 { 46.0 + 52.0 * others.min(3) as f32 } else if selected.is_some() { 40.0 } else { 0.0 } + if prompt { 66.0 } else { 0.0 };
        let picture_h = ((width - 2.0) * 9.0 / 16.0).min(ui.available_height() - below - 30.0).max(150.0);
        let (picture, frame) = film_frame(ui, width, picture_h, if selected.is_none() { Sense::click() } else { Sense::hover() }, tr("Choisir des vidéos"));
        let p = ui.painter().clone();
        match selected.map(|i| &self.items[i]) {
            Some(item) => {
                let mut player = self.player.as_mut().filter(|p| p.item == item.id);
                let live = player.as_mut().and_then(|p| p.texture(ui.ctx()));
                let playback = player.map(|p| (p.playing(), p.position(), p.duration, p.can_seek));
                match live.as_ref().or(item.thumb.as_ref()) {
                    Some(texture) => {
                        let size = texture.size_vec2();
                        let k = (picture.width() / size.x).min(picture.height() / size.y);
                        egui::Image::new(texture).paint_at(ui, Rect::from_center_size(picture.center(), size * k));
                    }
                    None => {
                        let icon = if item.info.as_ref().is_some_and(|i| !i.has_video) { Icon::AudioLines } else { Icon::Film };
                        icons::paint(&p, picture.center(), icon, 34.0, theme::rgb(0x4a4a4a));
                    }
                }
                if let Some(info) = &item.info {
                    let badge = |anchor: Align2, pos: egui::Pos2, text: String| {
                        let galley = p.layout_no_wrap(text, widgets::mono(10.5), t.ink);
                        let rect = anchor.anchor_size(pos, galley.size() + vec2(12.0, 6.0));
                        p.rect_filled(rect, CornerRadius::same(3), Color32::from_black_alpha(165));
                        p.galley(rect.min + vec2(6.0, 3.0), galley, t.ink);
                    };
                    if info.has_video && info.width > 0 {
                        badge(Align2::LEFT_BOTTOM, picture.left_bottom() + vec2(12.0, -12.0), format!("{} × {}", info.width, info.height));
                    }
                    if info.duration > 0.0 {
                        let time = match playback {
                            Some((_, position, ..)) => format!("{} / {}", i18n::clock(position), i18n::clock(info.duration)),
                            None => i18n::clock(info.duration),
                        };
                        badge(Align2::RIGHT_BOTTOM, picture.right_bottom() + vec2(-12.0, -12.0), time);
                    }
                }
                // Le lecteur : un clic sur l'image lit ou met en pause, la barre du bas déplace.
                if item.info.as_ref().is_some_and(|i| i.has_video) {
                    let playing = playback.is_some_and(|p| p.0);
                    let label = if playing { tr("Mettre en pause") } else { tr("Lire la vidéo") };
                    let resp = ui.interact(picture, Id::unique("preview-play"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
                    if resp.clicked() {
                        self.actions.push(Action::TogglePlay(item.id));
                    }
                    if !playing {
                        let (c, hover) = (picture.center(), resp.hovered());
                        p.circle(c, if hover { 31.0 } else { 29.0 }, Color32::from_black_alpha(if hover { 185 } else { 150 }), Stroke::new(1.0, Color32::from_white_alpha(if hover { 110 } else { 60 })));
                        p.add(egui::Shape::convex_polygon(vec![c + vec2(-7.5, -12.0), c + vec2(13.0, 0.0), c + vec2(-7.5, 12.0)], if hover { t.accent } else { t.ink_strong }, Stroke::NONE));
                    }
                    resp.on_hover_text(tr("L'aperçu se lit sans le son"));
                    if let Some((_, position, duration, can_seek)) = playback.filter(|p| p.2 > 0.0) {
                        let zone = Rect::from_min_max(pos2(picture.left(), picture.bottom() - 10.0), picture.max);
                        let mut active = false;
                        if can_seek {
                            let scrub = ui.interact(zone, Id::unique("preview-scrub"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
                            scrub.widget_info(|| egui::WidgetInfo::slider(true, position, tr("Position dans la vidéo")));
                            active = scrub.hovered() || scrub.dragged();
                            if let Some(pointer) = scrub.interact_pointer_pos().filter(|_| scrub.clicked() || scrub.dragged()) {
                                self.actions.push(Action::SeekTo(f64::from(((pointer.x - zone.left()) / zone.width()).clamp(0.0, 1.0)) * duration));
                            }
                        }
                        let bar = Rect::from_min_max(pos2(zone.left(), zone.bottom() - if active { 5.0 } else { 3.0 }), zone.max);
                        let done = bar.left() + bar.width() * (position / duration).clamp(0.0, 1.0) as f32;
                        p.rect_filled(bar, CornerRadius::ZERO, Color32::from_white_alpha(46));
                        p.rect_filled(Rect::from_min_max(bar.min, pos2(done, bar.bottom())), CornerRadius::ZERO, t.accent);
                        if active {
                            p.circle_filled(pos2(done, bar.center().y), 5.0, t.ink_strong);
                        }
                    }
                }
                // Retirer cette vidéo : un bouton posé sur l'image, sauf pendant qu'elle est traitée.
                if !matches!(item.state, State::Queued | State::Running { .. }) {
                    let spot = Rect::from_min_size(picture.right_top() + vec2(-40.0, 12.0), vec2(28.0, 28.0));
                    p.rect_filled(spot, CornerRadius::same(6), Color32::from_black_alpha(165));
                    let mut over = ui.new_child(egui::UiBuilder::new().max_rect(spot));
                    if widgets::icon_button(&mut over, Icon::X, tr("Retirer de la liste"), 28.0).clicked() {
                        self.actions.push(Action::Remove(item.id));
                    }
                }
            }
            None => {
                let c = picture.center();
                icons::paint(&p, c - vec2(0.0, 42.0), Icon::Clapperboard, 30.0, t.accent);
                p.text(c + vec2(0.0, 2.0), Align2::CENTER_CENTER, tr("Dépose tes vidéos ici"), FontId::new(17.0, theme::bold()), t.ink_strong);
                p.text(c + vec2(0.0, 26.0), Align2::CENTER_CENTER, tr("ou clique pour les choisir"), FontId::new(13.0, egui::FontFamily::Proportional), t.muted);
                p.text(c + vec2(0.0, 56.0), Align2::CENTER_CENTER, tr("MP4, MOV, MKV, WebM, AVI, GIF, WebP animé et bien d'autres"), FontId::new(12.0, egui::FontFamily::Proportional), t.faint);
                if frame.clicked() {
                    self.actions.push(Action::PickFiles);
                }
            }
        }

        if let Some(index) = selected {
            ui.add_space(14.0);
            selected_info(ui, &t, &self.items[index], &mut self.actions);
            ui.add_space(14.0);
            // Les autres vidéos de la sélection.
            ui.horizontal(|ui| {
                ui.set_height(26.0);
                if others > 0 {
                    widgets::caps(ui, tr("Aussi dans la sélection"));
                    ui.label(RichText::new(others.to_string()).font(widgets::mono(10.5)).color(t.muted));
                } else {
                    widgets::caps(ui, tr("Une seule vidéo"));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::button_sized(ui, Some(Icon::Plus), tr("Ajouter"), Kind::Secondary, 26.0, None).clicked() {
                        self.actions.push(Action::PickFiles);
                    }
                });
            });
            if others > 0 {
                ui.add_space(8.0);
                let room = (ui.available_height() - if prompt { 66.0 } else { 0.0 }).max(52.0);
                Frame::new().fill(t.well).stroke(Stroke::new(1.0, theme::rgb(0x2a2a2a))).corner_radius(8).show(ui, |ui| {
                    ScrollArea::vertical().id_salt("others").max_height(room - 2.0).auto_shrink([false, true]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let mut first = true;
                        for item in self.items.iter().filter(|i| Some(i.id) != self.selected) {
                            if !first {
                                widgets::separator(ui);
                            }
                            first = false;
                            if other_row(ui, &t, item).clicked() {
                                self.actions.push(Action::Select(item.id));
                            }
                        }
                    });
                });
            }
        }

        if prompt {
            let gap = ui.available_height() - 52.0;
            if gap > 0.0 {
                ui.add_space(gap);
            }
            Frame::new().fill(t.well).stroke(Stroke::new(1.0, theme::rgb(0x2a2a2a))).corner_radius(8).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.set_height(34.0);
                    let text_w = (ui.available_width() - 190.0).max(100.0);
                    ui.vertical(|ui| {
                        ui.set_width(text_w);
                        ui.spacing_mut().item_spacing.y = 1.0;
                        widgets::one_line(ui, tr("Ajouter Video Compressor au clic droit"), FontId::new(12.5, theme::semibold()), t.ink_strong, text_w);
                        widgets::one_line(ui, tr("Un clic droit sur une vidéo l'ouvrira ici."), FontId::new(11.5, egui::FontFamily::Proportional), t.muted, text_w);
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if widgets::button_sized(ui, None, tr("Plus tard"), Kind::Ghost, 28.0, None).clicked() {
                            self.actions.push(Action::DismissMenuPrompt);
                        }
                        if widgets::button_sized(ui, None, tr("Activer"), Kind::Secondary, 28.0, None).clicked() {
                            self.actions.push(Action::MenuOn);
                        }
                    });
                });
            });
        }

        if hovering {
            let p = ui.painter().with_clip_rect(full);
            p.rect_filled(full, CornerRadius::ZERO, theme::tint(t.bg, 0.84));
            p.rect_stroke(full.shrink(10.0), CornerRadius::same(10), Stroke::new(1.5, t.accent), StrokeKind::Inside);
            p.text(full.center(), Align2::CENTER_CENTER, tr("Lâche pour ajouter à la liste"), FontId::new(16.0, theme::bold()), t.ink_strong);
        }
    }

    // ---- colonne de droite : ce qu'on en fait ------------------------------------------------------

    fn choices(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let inner = ui.max_rect();
        ui.painter().vline(inner.left() - PAD_X + 0.5, (inner.top() - PAD_TOP)..=(inner.bottom() + PAD_BOTTOM), t.hairline());
        egui::Panel::bottom("launch").exact_size(98.0).resizable(false).show_separator_line(false).frame(Frame::new()).show(ui, |ui| self.launch(ui));
        egui::CentralPanel::default().frame(Frame::new()).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new(tr("Que veux-tu en faire")).font(FontId::new(14.0, theme::semibold())).color(t.ink_strong));
            ui.add_space(14.0);
            let running = self.running();
            // Sur une fenêtre basse, les réglages dépliés défilent plutôt que de recouvrir le bouton.
            ScrollArea::vertical().id_salt("choices").auto_shrink([false, false]).show(ui, |ui| {
                ui.add_enabled_ui(!running, |ui| {
                    if self.fine_open { self.preset_row(ui) } else { self.preset_grid(ui) }
                    ui.add_space(if self.fine_open { 10.0 } else { 14.0 });
                    self.fine_settings(ui);
                });
            });
        });
    }

    /// Les choix en grand : quatre cases jointives, chacune avec son dessin, et le son seul dessous.
    /// Le cadre a des angles vifs, repérés par des croix.
    fn preset_grid(&mut self, ui: &mut Ui) {
        const CELL_H: f32 = 136.0;
        const ROW_H: f32 = 44.0;
        let t = Tokens::get(ui.ctx());
        let list = presets();
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, CELL_H * 2.0 + ROW_H), Sense::hover());
        let p = ui.painter().clone();
        p.rect(rect, CornerRadius::ZERO, t.well, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
        let half = (w / 2.0).round();
        let rule = Stroke::new(1.0, t.rule);
        p.line_segment([pos2(rect.left() + half + 0.5, rect.top() + 1.0), pos2(rect.left() + half + 0.5, rect.top() + CELL_H * 2.0)], rule);
        for y in [CELL_H, CELL_H * 2.0] {
            p.line_segment([pos2(rect.left() + 1.0, rect.top() + y + 0.5), pos2(rect.right() - 1.0, rect.top() + y + 0.5)], rule);
        }
        for (i, preset) in list.iter().enumerate() {
            let cell = match i {
                0..=3 => Rect::from_min_size(rect.min + vec2(if i % 2 == 1 { half } else { 0.0 }, if i >= 2 { CELL_H } else { 0.0 }), vec2(if i % 2 == 1 { w - half } else { half }, CELL_H)),
                _ => Rect::from_min_size(rect.min + vec2(0.0, CELL_H * 2.0), vec2(w, ROW_H)),
            };
            let resp = ui.interact(cell, Id::unique(("preset", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            let selected = self.settings.encode == preset.encode;
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, preset.name));
            if resp.clicked() {
                self.settings.encode = preset.encode;
            }
            if selected {
                // Le choix actif : une teinte d'accent, de fines hachures, un contour.
                p.rect_filled(cell, CornerRadius::ZERO, theme::tint(t.accent, 0.05));
                draw::hatch(&p, cell, theme::tint(t.accent, 0.06), 9.0);
                p.rect_stroke(cell, CornerRadius::ZERO, Stroke::new(1.0, theme::tint(t.accent, 0.5)), StrokeKind::Inside);
            } else if resp.hovered() {
                p.rect_filled(cell.shrink(1.0), CornerRadius::ZERO, theme::tint(t.ink, 0.03));
            }
            let origin = cell.min + vec2(14.0, 14.0);
            match i {
                0 => draw::web(&p, origin, &t, selected),
                1 => draw::light(&p, origin, &t, selected),
                2 => draw::limit(&p, origin, &t, selected),
                3 => draw::gif(&p, origin, &t, selected),
                _ => {
                    draw::sound(&p, pos2(cell.left() + 14.0, cell.center().y), &t, selected);
                    p.text(pos2(cell.left() + 52.0, cell.center().y), Align2::LEFT_CENTER, preset.name, FontId::new(13.0, theme::medium()), t.ink);
                    p.text(pos2(cell.right() - 14.0, cell.center().y), Align2::RIGHT_CENTER, preset.spec, widgets::mono(11.0), t.muted);
                    continue;
                }
            }
            p.text(pos2(cell.left() + 14.0, cell.bottom() - 36.0), Align2::LEFT_BOTTOM, preset.name, FontId::new(14.0, theme::semibold()), t.ink_strong);
            p.text(pos2(cell.left() + 14.0, cell.bottom() - 14.0), Align2::LEFT_BOTTOM, preset.spec, FontId::new(12.0, egui::FontFamily::Proportional), if selected { theme::rgb(0xc8c8c8) } else { t.muted });
        }
        // Les croix : aux quatre angles du cadre, et au croisement de ses filets.
        let mut marks = p.clone();
        marks.set_clip_rect(rect.expand(8.0));
        for corner in [rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom(), pos2(rect.left() + half, rect.top() + CELL_H)] {
            draw::cross(&marks, corner, t.mark);
        }
    }

    /// Les mêmes choix sur une seule ligne, quand les réglages fins sont ouverts.
    fn preset_row(&mut self, ui: &mut Ui) {
        let list = presets();
        let mut current = list.iter().position(|p| p.encode == self.settings.encode).unwrap_or(usize::MAX);
        let options: Vec<(usize, &str)> = list.iter().enumerate().map(|(i, p)| (i, p.short)).collect();
        let w = ui.available_width();
        if widgets::seg(ui, "preset-row", &mut current, &options, w, false)
            && let Some(preset) = list.get(current)
        {
            self.settings.encode = preset.encode;
        }
    }

    /// La ligne « Réglages fins » : repliée, elle résume les réglages ; dépliée, elle les montre tous.
    fn fine_settings(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let w = ui.available_width();
        let open = self.fine_open;
        let (rect, resp) = ui.allocate_exact_size(vec2(w, if open { 40.0 } else { 44.0 }), Sense::click());
        let enabled = ui.is_enabled();
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, enabled, open, tr("Réglages fins")));
        let p = ui.painter().clone();
        let radius = if open { CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 } } else { CornerRadius::same(8) };
        let border = Stroke::new(1.0, theme::rgb(0x2e2e2e));
        p.rect(rect, radius, if resp.hovered() { theme::rgb(0x1c1c1c) } else { t.surface }, border, StrokeKind::Inside);
        let title = p.layout_no_wrap(tr("Réglages fins").to_owned(), FontId::new(13.0, theme::medium()), t.ink);
        let title_w = title.size().x;
        p.galley(pos2(rect.left() + 14.0, rect.center().y - title.size().y / 2.0), title, t.ink);
        icons::paint(&p, pos2(rect.right() - 20.0, rect.center().y), if open { Icon::ChevronUp } else { Icon::ChevronDown }, 15.0, t.muted);
        let summary = self.summary();
        let summary = widgets::elided(ui, &summary, &widgets::mono(11.0), rect.width() - title_w - 70.0);
        p.text(pos2(rect.right() - 38.0, rect.center().y), Align2::RIGHT_CENTER, summary, widgets::mono(11.0), t.faint);
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.actions.push(Action::ToggleFine);
        }
        if !open {
            return;
        }
        let enc = &mut self.settings.encode;
        // Le corps se raccorde à la ligne du dessus : son filet haut recouvre le filet bas de celle-ci.
        ui.add_space(-1.0);
        Frame::new().fill(t.well).stroke(border).corner_radius(CornerRadius { nw: 0, ne: 0, sw: 8, se: 8 }).inner_margin(Margin { left: 13, right: 13, top: 10, bottom: 10 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            fine_row(ui, &t, tr("Format"), |ui, w| {
                let options: Vec<(Container, &str)> = Container::ALL.iter().zip(["MP4", "MKV", "MOV", "WebM", "GIF", "WebP", "MP3"]).map(|(c, l)| (*c, l)).collect();
                widgets::seg(ui, "format", &mut enc.container, &options, w, true);
            });
            enc.normalize();
            let kind_now = enc.container.kind();
            if kind_now == OutputKind::Video {
                fine_row(ui, &t, tr("Codec"), |ui, w| {
                    let options: Vec<(VideoCodec, &str)> = enc.container.codecs().iter().map(|c| (*c, c.short())).collect();
                    widgets::seg(ui, "codec", &mut enc.codec, &options, w, false);
                });
            }
            if kind_now != OutputKind::Audio {
                fine_row(ui, &t, tr("Résolution"), |ui, w| {
                    let labels = [tr("Orig."), "2160", "1440", "1080", "720", "480", "360", "240"];
                    let options: Vec<(Resolution, &str)> = Resolution::ALL.iter().zip(labels).map(|(r, l)| (*r, l)).collect();
                    widgets::seg(ui, "resolution", &mut enc.resolution, &options, w, true);
                });
                fine_row(ui, &t, tr("Images/s"), |ui, w| {
                    let labels = [tr("Orig."), "60", "50", "30", "25", "24", "15", "12", "10"];
                    let options: Vec<(Fps, &str)> = Fps::ALL.iter().zip(labels).map(|(f, l)| (*f, l)).collect();
                    widgets::seg(ui, "fps", &mut enc.fps, &options, w, true);
                });
                if kind_now == OutputKind::Video {
                    fine_row(ui, &t, tr("Poids"), |ui, w| {
                        widgets::seg(ui, "mode", &mut enc.mode, &[(Mode::Quality, tr("Par la qualité")), (Mode::TargetSize, tr("Taille cible"))], w, false);
                    });
                }
                compression_block(ui, &t, enc);
            }
            if kind_now == OutputKind::Video || enc.container == Container::Webp {
                fine_row(ui, &t, tr("Vitesse"), |ui, w| {
                    widgets::seg(ui, "speed", &mut enc.speed, &[(Speed::Fast, tr("Rapide")), (Speed::Balanced, tr("Moyenne")), (Speed::Slow, tr("Lente"))], w, false);
                });
            }
            if kind_now != OutputKind::Animation {
                fine_row(ui, &t, tr("Son"), |ui, w| {
                    let labels = ["192", "128", "96", "64", tr("Sans")];
                    let options: Vec<(Audio, &str)> = Audio::ALL.iter().zip(labels).filter(|(a, _)| kind_now == OutputKind::Video || **a != Audio::None).map(|(a, l)| (*a, l)).collect();
                    widgets::seg(ui, "audio", &mut enc.audio, &options, w, true);
                });
            }
        });
    }

    /// Les réglages en une ligne, avec le facteur de qualité quand il s'applique.
    fn summary(&self) -> String {
        let enc = &self.settings.encode;
        let mut text = enc.summary();
        if enc.container.kind() == OutputKind::Video && enc.mode == Mode::Quality {
            text.push_str(&format!(" · CRF {}", engine::crf(enc)));
        }
        if enc.container.kind() != OutputKind::Animation
            && let Some(kbps) = enc.audio.kbps()
        {
            text.push_str(&format!(" · {kbps} kb/s"));
        }
        text
    }

    /// Le bas de la colonne : où ira le fichier, puis le bouton qui lance (ou arrête) le travail.
    fn launch(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let w = ui.available_width();
        ui.spacing_mut().item_spacing.y = 0.0;
        // Un filet gradué, comme une règle, au-dessus de la destination.
        let (rule, _) = ui.allocate_exact_size(vec2(w, 5.0), Sense::hover());
        draw::ruler(ui.painter(), rule, theme::rgb(0x2a2a2a), theme::rgb(0x2e2e2e), 10.0);
        ui.add_space(8.0);
        let (line, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        let ready = self.items.iter().filter(|i| i.encodable()).count();
        let p = ui.painter().clone();
        match &self.batch {
            Some(batch) => {
                let current = self.items.iter().find_map(|i| if let State::Running { fraction, .. } = i.state { Some(fraction) } else { None }).unwrap_or(0.0);
                let overall = (batch.finished as f32 + current) / batch.total.max(1) as f32;
                p.text(line.left_center(), Align2::LEFT_CENTER, tf("Fichier {n} sur {total}", &[("n", &(batch.finished + 1).min(batch.total)), ("total", &batch.total)]), FontId::new(12.0, egui::FontFamily::Proportional), t.muted);
                p.text(line.right_center(), Align2::RIGHT_CENTER, format!("{} %", (overall * 100.0).round() as u32), FontId::new(12.5, theme::semibold()), t.ink_strong);
                ui.add_space(6.0);
                widgets::progress(ui, overall, t.accent, w, 4.0);
                ui.add_space(9.0);
                if widgets::launch_button(ui, Icon::X, tr("Arrêter"), w, true).clicked() {
                    self.actions.push(Action::Cancel);
                }
            }
            None => {
                let place = match (self.settings.out_dir.as_str(), ready) {
                    ("", 0 | 1) => tr("Ira à côté de l'original").to_owned(),
                    ("", n) => tf("{n} fichiers, à côté des originaux", &[("n", &n)]),
                    (dir, _) => tf("Ira dans {0}", &[("0", &std::path::Path::new(dir).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.to_owned()))]),
                };
                let place = p.text(line.left_center(), Align2::LEFT_CENTER, place, FontId::new(12.0, egui::FontFamily::Proportional), t.muted);
                // Le nom du fichier que la vidéo choisie produira.
                let name = self.selected.and_then(|id| self.items.iter().find(|i| i.id == id)).and_then(|item| queue::output_path(&item.path, &self.settings.encode, &self.settings.out_dir, &self.settings.suffix, &[]).file_name().map(|n| n.to_string_lossy().into_owned()));
                if let Some(name) = name {
                    let name = widgets::elided(ui, &name, &widgets::mono(11.5), w - place.width() - 16.0);
                    p.text(line.right_center(), Align2::RIGHT_CENTER, name, widgets::mono(11.5), t.ink);
                }
                ui.add_space(19.0);
                let convert = self.settings.encode.container.kind() != OutputKind::Video;
                let label = match (ready, convert) {
                    (0 | 1, false) => tr("Compresser").to_owned(),
                    (0 | 1, true) => tr("Convertir").to_owned(),
                    (n, false) => tf("Compresser {n} fichiers", &[("n", &n)]),
                    (n, true) => tf("Convertir {n} fichiers", &[("n", &n)]),
                };
                let resp = ui.add_enabled_ui(ready > 0, |ui| widgets::launch_button(ui, Icon::Zap, &label, w, false)).inner;
                if resp.clicked() {
                    self.actions.push(Action::Start);
                }
            }
        }
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        if self.toast.as_ref().is_some_and(|toast| self.now > toast.until) {
            self.toast = None;
        }
        let Some(toast) = &self.toast else { return };
        let t = Tokens::get(ctx);
        egui::Area::new(Id::unique("toast")).order(egui::Order::Foreground).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -(theme::STATUS_H + 14.0))).show(ctx, |ui| {
            Frame::new().fill(t.raised).stroke(Stroke::new(1.0, t.line_strong)).corner_radius(theme::R_CARD).shadow(theme::shadow()).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    icons::show(ui, toast.icon, 16.0, t.accent_ink);
                    ui.label(RichText::new(&toast.text).size(13.0).color(t.ink_strong));
                });
            });
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
    }
}

/// Les marges des deux colonnes.
const PAD_X: f32 = 20.0;
const PAD_TOP: f32 = 18.0;
const PAD_BOTTOM: f32 = 16.0;

/// Le cadre de la vidéo choisie : une image de pellicule, perforée en haut et en bas, posée sur
/// une plaque hachurée décalée. Renvoie la place de l'image et la réponse du cadre.
fn film_frame(ui: &mut Ui, width: f32, picture_h: f32, sense: Sense, name: &str) -> (Rect, egui::Response) {
    const STRIP: f32 = 11.0;
    let (outer, resp) = ui.allocate_exact_size(vec2(width + 8.0, picture_h + STRIP * 2.0 + 8.0), sense);
    if sense.senses_click() {
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    }
    let card = Rect::from_min_size(outer.min, vec2(width, picture_h + STRIP * 2.0));
    let plate = card.translate(vec2(8.0, 8.0));
    let p = ui.painter();
    draw::hatch(p, plate.shrink(2.0), theme::rgb(0x242424), 8.0);
    p.rect_stroke(plate, CornerRadius::same(10), Stroke::new(1.0, theme::rgb(0x2a2a2a)), StrokeKind::Inside);
    p.rect_filled(card, CornerRadius::same(10), theme::rgb(0x0d0d0d));
    let hole = theme::rgb(0x2e2e2e);
    draw::perforations(p, Rect::from_min_size(card.min + vec2(4.0, 0.0), vec2(width - 8.0, STRIP)), hole);
    draw::perforations(p, Rect::from_min_size(pos2(card.left() + 4.0, card.bottom() - STRIP), vec2(width - 8.0, STRIP)), hole);
    let picture = Rect::from_min_max(pos2(card.left() + 1.0, card.top() + STRIP), pos2(card.right() - 1.0, card.bottom() - STRIP));
    p.rect_filled(picture, CornerRadius::ZERO, theme::rgb(0x151515));
    draw::gradient_frame(p, card, 10.0, [theme::rgb(0x5c5c5c), theme::rgb(0x2a2a2a), theme::rgb(0x404040)]);
    let resp = if sense.senses_click() { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
    (picture, resp)
}

/// « 29,97 i/s, HEVC » : ce que l'image ne dit pas déjà.
fn meta_line(item: &Item) -> String {
    let Some(info) = &item.info else { return String::new() };
    let mut parts = Vec::new();
    if info.has_video {
        if info.fps > 0.0 {
            parts.push(tf("{n} i/s", &[("n", &i18n::decimal(info.fps, if (info.fps - info.fps.round()).abs() < 0.01 { 0 } else { 2 }))]));
        }
        parts.push(if info.webp.is_some() { tr("WebP animé").to_owned() } else { info.vcodec.to_uppercase() });
        if !info.has_audio && info.webp.is_none() {
            parts.push(tr("sans son").to_owned());
        }
    } else if info.has_audio {
        parts.push(tr("Son seul").to_owned());
    }
    parts.join(", ")
}

/// Sous l'image : le nom, les mesures et le poids de la vidéo choisie, puis où elle en est.
fn selected_info(ui: &mut Ui, t: &Tokens, item: &Item, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.set_height(58.0);
        let text_w = (ui.available_width() - 190.0).max(100.0);
        ui.vertical(|ui| {
            ui.set_width(text_w);
            ui.spacing_mut().item_spacing.y = 4.0;
            widgets::one_line(ui, &item.name, FontId::new(16.0, theme::semibold()), t.ink_strong, text_w).on_hover_text(item.path.display().to_string());
            widgets::one_line(ui, &meta_line(item), FontId::new(12.0, egui::FontFamily::Proportional), t.muted, text_w);
            widgets::one_line(ui, &item.path.parent().map(|p| p.display().to_string()).unwrap_or_default(), widgets::mono(11.0), t.faint, text_w);
        });
        ui.with_layout(Layout::right_to_left(Align::Max), |ui| {
            if let Some(info) = &item.info {
                ui.label(RichText::new(i18n::size(info.size)).font(FontId::new(26.0, theme::semibold())).color(t.ink_strong));
            }
        });
    });
    ui.add_space(10.0);
    // Où en est cette vidéo : une ligne, toujours de la même hauteur.
    ui.horizontal(|ui| {
        ui.set_height(28.0);
        ui.spacing_mut().item_spacing.x = 6.0;
        match &item.state {
            State::Probing => {
                ui.add(egui::Spinner::new().size(14.0).color(t.muted));
                ui.label(RichText::new(tr("Analyse…")).size(12.5).color(t.muted));
            }
            State::Ready => {
                let (dot, _) = ui.allocate_exact_size(vec2(6.0, 6.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 3.0, t.faint);
                ui.label(RichText::new(tr("Prêt")).size(12.5).color(t.muted));
            }
            State::Queued => {
                ui.label(RichText::new(tr("En attente")).size(12.5).color(t.muted));
            }
            State::Running { fraction, started, .. } => {
                let elapsed = started.elapsed().as_secs_f64();
                let percent = format!("{} %", (fraction * 100.0).round() as u32);
                ui.label(RichText::new(&percent).font(FontId::new(13.0, theme::semibold())).color(t.ink_strong));
                if *fraction > 0.02 {
                    let left = elapsed / f64::from(*fraction) * f64::from(1.0 - *fraction);
                    ui.label(RichText::new(tf("reste {left}", &[("left", &i18n::clock(left))])).size(12.5).color(t.muted));
                }
                let w = ui.available_width();
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    widgets::progress(ui, *fraction, t.accent, (w - 12.0).max(60.0), 4.0);
                });
            }
            State::Done { output, size } => {
                let before = item.info.as_ref().map(|i| i.size).unwrap_or(0);
                let (gain, color) = if *size <= before {
                    (tf("{n} % de moins", &[("n", &(((before - *size) as f64 / before.max(1) as f64 * 100.0).round() as u32))]), t.accent_ink)
                } else {
                    (tf("{n} % de plus", &[("n", &(((*size - before) as f64 / before.max(1) as f64 * 100.0).round() as u32))]), t.muted)
                };
                // Le poids de départ est écrit en grand juste au-dessus : la ligne dit où on arrive.
                let text = format!("→  {}  ·  {gain}", i18n::size(*size));
                let buttons_w = 222.0;
                widgets::one_line(ui, &text, FontId::new(13.0, theme::semibold()), color, (ui.available_width() - buttons_w).max(80.0)).on_hover_text(output.display().to_string());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if widgets::icon_button(ui, Icon::RotateCcw, tr("Recommencer avec les réglages du moment"), 28.0).clicked() {
                        actions.push(Action::Retry(item.id));
                    }
                    if widgets::button_sized(ui, Some(Icon::FolderOpen), tr("Montrer"), Kind::Secondary, 28.0, None).clicked() {
                        actions.push(Action::Reveal(output.clone()));
                    }
                    if widgets::button_sized(ui, Some(Icon::Play), tr("Lire"), Kind::Secondary, 28.0, None).clicked() {
                        actions.push(Action::Open(output.clone()));
                    }
                });
            }
            State::Failed(why) => {
                let (dot, _) = ui.allocate_exact_size(vec2(6.0, 6.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 3.0, t.danger);
                ui.label(RichText::new(tr("Échec")).font(FontId::new(12.5, theme::semibold())).color(t.danger));
                let buttons_w = 200.0;
                widgets::one_line(ui, why.lines().last().unwrap_or(why), FontId::new(12.0, egui::FontFamily::Proportional), t.muted, (ui.available_width() - buttons_w).max(60.0)).on_hover_text(why.as_str());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if widgets::icon_button(ui, Icon::Copy, tr("Copier le détail de l'erreur"), 28.0).clicked() {
                        actions.push(Action::Copy(why.clone()));
                    }
                    if item.info.is_some() && widgets::button_sized(ui, Some(Icon::RotateCcw), tr("Réessayer"), Kind::Secondary, 28.0, None).clicked() {
                        actions.push(Action::Retry(item.id));
                    }
                });
            }
        }
    });
}

/// Une autre vidéo de la sélection : vignette, nom, mesures, et où elle en est. Un clic la choisit.
fn other_row(ui: &mut Ui, t: &Tokens, item: &Item) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 52.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &item.name));
    let p = ui.painter().clone();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::ZERO, theme::tint(t.ink, 0.04));
    }
    let thumb = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - 18.0), vec2(64.0, 36.0));
    p.rect_filled(thumb, CornerRadius::same(4), theme::rgb(0x202020));
    match &item.thumb {
        Some(texture) => {
            let size = texture.size_vec2();
            let k = (thumb.width() / size.x).min(thumb.height() / size.y);
            egui::Image::new(texture).corner_radius(CornerRadius::same(3)).paint_at(ui, Rect::from_center_size(thumb.center(), size * k));
        }
        None => icons::paint(&p, thumb.center(), if item.info.as_ref().is_some_and(|i| !i.has_video) { Icon::AudioLines } else { Icon::Film }, 16.0, t.faint),
    }
    let (state, color) = match &item.state {
        State::Probing => (tr("Analyse…").to_owned(), t.muted),
        State::Ready => (item.info.as_ref().map(|i| i18n::size(i.size)).unwrap_or_default(), t.muted),
        State::Queued => (tr("En attente").to_owned(), t.muted),
        State::Running { fraction, .. } => (format!("{} %", (fraction * 100.0).round() as u32), t.ink_strong),
        State::Done { size, .. } => {
            let before = item.info.as_ref().map(|i| i.size).unwrap_or(0);
            if *size <= before { (tf("{n} % de moins", &[("n", &(((before - *size) as f64 / before.max(1) as f64 * 100.0).round() as u32))]), t.accent_ink) } else { (i18n::size(*size), t.muted) }
        }
        State::Failed(_) => (tr("Échec").to_owned(), t.danger),
    };
    let state = p.text(pos2(rect.right() - 12.0, rect.center().y), Align2::RIGHT_CENTER, state, widgets::mono(11.5), color);
    let text_w = (state.left() - thumb.right() - 22.0).max(40.0);
    let name = widgets::elided(ui, &item.name, &FontId::new(13.0, theme::medium()), text_w);
    p.text(pos2(thumb.right() + 10.0, rect.center().y - 1.0), Align2::LEFT_BOTTOM, name, FontId::new(13.0, theme::medium()), t.ink);
    let mut meta = item.info.as_ref().filter(|i| i.has_video && i.width > 0).map(|i| format!("{} × {}", i.width, i.height)).unwrap_or_default();
    let more = meta_line(item);
    if !more.is_empty() {
        if !meta.is_empty() {
            meta.push_str(", ");
        }
        meta.push_str(&more);
    }
    let meta = widgets::elided(ui, &meta, &FontId::new(11.5, egui::FontFamily::Proportional), text_w);
    p.text(pos2(thumb.right() + 10.0, rect.center().y + 2.0), Align2::LEFT_TOP, meta, FontId::new(11.5, egui::FontFamily::Proportional), t.muted);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Une ligne des réglages fins : le nom à gauche, le contrôle sur le reste de la largeur.
fn fine_row(ui: &mut Ui, t: &Tokens, label: &str, add: impl FnOnce(&mut Ui, f32)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (cell, resp) = ui.allocate_exact_size(vec2(84.0, 30.0), Sense::hover());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, label));
        ui.painter().text(cell.left_center(), Align2::LEFT_CENTER, label, FontId::new(12.5, egui::FontFamily::Proportional), t.muted);
        let w = ui.available_width();
        add(ui, w);
    });
}

/// Le degré de compression : le curseur gradué, ou la taille à ne pas dépasser.
fn compression_block(ui: &mut Ui, t: &Tokens, enc: &mut Encode) {
    Frame::new().fill(t.bg).stroke(Stroke::new(1.0, theme::rgb(0x2a2a2a))).corner_radius(8).inner_margin(Margin { left: 12, right: 12, top: 8, bottom: 8 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 3.0;
        if enc.mode == Mode::TargetSize {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("Le fichier ne dépassera pas")).size(12.5).color(t.muted));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let field = egui::DragValue::new(&mut enc.target_mb).range(1.0..=50_000.0).speed(0.5).max_decimals(1).suffix(format!(" {}", tr("Mo")));
                    widgets::named(ui.add_sized(vec2(96.0, 26.0), field), egui::Role::SpinButton, tr("Taille à ne pas dépasser"));
                });
            });
            return;
        }
        ui.horizontal(|ui| {
            // « Compression équilibrée », « Balanced compression » : la majuscule suit la langue.
            let text = tf("Compression {0}", &[("0", &enc.compression_label().to_lowercase())]);
            let mut chars = text.chars();
            let text: String = chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default();
            ui.label(RichText::new(text).font(FontId::new(12.5, theme::semibold())).color(t.ink_strong));
            if enc.container.kind() == OutputKind::Video {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(format!("CRF {}", engine::crf(enc))).font(widgets::mono(11.0)).color(t.accent_ink));
                });
            }
        });
        let w = ui.available_width();
        widgets::graduated_slider(ui, tr("Degré de compression"), &mut enc.compression, w);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Plus net, plus lourd")).size(11.0).color(t.faint));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(tr("Plus léger")).size(11.0).color(t.faint));
            });
        });
    });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.now = ctx.input(|i| i.time);
        self.poll(&ctx);
        // Les fichiers lâchés sur la fenêtre.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if !dropped.is_empty() {
            self.actions.push(Action::Add(dropped));
        }
        // Toujours une vidéo choisie, tant qu'il y en a une.
        if self.selected.is_none_or(|id| !self.items.iter().any(|i| i.id == id)) {
            self.selected = self.items.first().map(|i| i.id);
        }
        // Le lecteur suit la vidéo choisie : une autre est choisie, il se ferme.
        if self.player.as_ref().is_some_and(|p| Some(p.item) != self.selected) {
            self.player = None;
        }
        if let Some(why) = self.player.as_ref().and_then(|p| p.error()) {
            self.player = None;
            self.notify(Icon::TriangleAlert, tf("Cette vidéo ne peut pas être lue ici ({0})", &[("0", &why)]));
        }
        // La barre d'espace lit ou met en pause, hors d'un champ de saisie ou d'une fenêtre ouverte.
        if self.dialog.is_none()
            && let Some(id) = self.selected
            && ctx.memory(|m| m.focused().is_none())
            && ctx.input(|i| i.key_pressed(egui::Key::Space))
        {
            self.actions.push(Action::TogglePlay(id));
        }
        // Le temps restant avance même quand l'encodeur ne dit rien.
        if self.running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        let edge = widgets::edge_under_pointer(&ctx);
        let t = Tokens::get(&ctx);
        let margin = Margin { left: PAD_X as i8, right: PAD_X as i8, top: PAD_TOP as i8, bottom: PAD_BOTTOM as i8 };

        egui::Panel::top("topbar").exact_size(theme::TOPBAR_H).resizable(false).show_separator_line(false).frame(Frame::new().fill(t.bg)).show(ui, |ui| self.top_bar(ui, edge.is_some()));
        egui::Panel::bottom("statusbar").exact_size(theme::STATUS_H).resizable(false).show_separator_line(false).frame(Frame::new().fill(t.bg)).show(ui, |ui| self.status_bar(ui));
        egui::Panel::right("choices").exact_size(theme::PANEL_W).resizable(false).show_separator_line(false).frame(Frame::new().fill(t.panel).inner_margin(margin)).show(ui, |ui| self.choices(ui));
        egui::CentralPanel::default().frame(Frame::new().fill(t.bg).inner_margin(margin)).show(ui, |ui| self.videos(ui));

        match self.dialog {
            Some(Dialog::Prefs) => crate::dialogs::prefs(&ctx, self),
            Some(Dialog::Update) => crate::dialogs::update(&ctx, self),
            None => {}
        }
        self.toast_ui(&ctx);
        self.run_actions(&ctx);
        widgets::resize_from_edge(&ctx, edge);
    }
}
