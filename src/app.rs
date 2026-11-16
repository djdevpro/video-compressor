//! L'application : les fichiers à gauche, les réglages à droite, et les fils de travail derrière.
//!
//! L'interface ne fait que poser des `Action` ; `App::run_actions` les applique en fin d'image.
//! Les fils (analyse, encodage, téléchargements) répondent par des messages relevés dans `poll`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use egui::{Align, Align2, Color32, FontId, Frame, Id, Layout, Margin, RichText, ScrollArea, Stroke, Ui, vec2};

use crate::engine::{self, EncodeError, VIDEO_EXTS};
use crate::i18n::{self, Lang, tf, tr};
use crate::icons::{self, Icon};
use crate::instance::Waker;
use crate::queue::{self, Item, Job, Msg, State};
use crate::release::{self, Phase as UpdatePhase, Updater, VERSION};
use crate::settings::{Audio, Container, Encode, Fps, Kind as OutputKind, Mode, Resolution, Settings, Speed, presets};
use crate::shell;
use crate::theme::{self, Tokens};
use crate::widgets::{self, Kind, Tone};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    Prefs,
    Update,
}

pub enum Action {
    PickFiles,
    Add(Vec<PathBuf>),
    Remove(u64),
    Clear,
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
    pub(crate) toast: Option<Toast>,
    pub(crate) actions: Vec<Action>,
    logo: egui::TextureHandle,
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
        let icon = eframe::icon_data::from_png_bytes(crate::ICON_PNG).expect("icône de l'application");
        let image = egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
        let logo = cc.egui_ctx.load_texture("logo", image, egui::TextureOptions::LINEAR);
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
            toast: None,
            actions: Vec::new(),
            logo,
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
            if !self.dry_run {
                queue::spawn_probe(item.id, item.path.clone(), self.tx.clone(), ctx.clone());
            }
            self.items.push(item);
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
                Action::Remove(id) => self.items.retain(|i| i.id != id || matches!(i.state, State::Queued | State::Running { .. })),
                Action::Clear => self.items.retain(|i| matches!(i.state, State::Queued | State::Running { .. })),
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
            ui.add_space(16.0);
            ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(22.0, 22.0)));
            ui.add_space(2.0);
            ui.label(RichText::new("Video Compressor").font(FontId::new(15.0, theme::bold())).color(t.ink_strong));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                widgets::window_buttons(ui, theme::TOPBAR_H);
                ui.add_space(8.0);
                ui.spacing_mut().item_spacing.x = 2.0;
                if widgets::icon_button(ui, Icon::Settings, tr("Préférences"), 32.0).clicked() {
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
                let resp = widgets::icon_button(ui, Icon::CircleArrowUp, &tip, 32.0);
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
        let small = |text: String, color: Color32| RichText::new(text).size(12.0).color(color);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.add_space(16.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            let (color, text) = (t.good_fg, tf("FFmpeg {0} intégré", &[("0", &engine::version())]));
            let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 3.5, color);
            ui.label(small(text, t.muted));
            ui.add_space(8.0);
            widgets::vsep(ui, 14.0);
            ui.add_space(8.0);
            icons::show(ui, Icon::MousePointerClick, 13.0, if self.menu_on { t.good_fg } else { t.faint });
            ui.label(small(if self.menu_on { tr("Clic droit actif") } else { tr("Clic droit inactif") }.to_owned(), t.muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(16.0);
                ui.label(small(tf("Version {VERSION}", &[("VERSION", &VERSION)]), t.faint));
            });
        });
    }

    // ---- réglages (panneau de droite) --------------------------------------------------------------

    fn settings_panel(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let rect = ui.max_rect();
        ui.painter().vline(rect.left() + 0.5, rect.y_range(), t.hairline());
        egui::Panel::bottom("launch")
            .exact_size(112.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().inner_margin(Margin { left: 18, right: 18, top: 0, bottom: 14 }))
            .show(ui, |ui| self.launch(ui));
        egui::CentralPanel::default().frame(Frame::new().inner_margin(Margin { left: 18, right: 4, top: 16, bottom: 0 })).show(ui, |ui| {
            ScrollArea::vertical().id_salt("settings-scroll").auto_shrink(false).show(ui, |ui| {
                Frame::new().inner_margin(Margin { left: 0, right: 14, top: 0, bottom: 8 }).show(ui, |ui| {
                    let running = self.running();
                    ui.add_enabled_ui(!running, |ui| self.settings_form(ui));
                });
            });
        });
    }

    fn settings_form(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let enc = &mut self.settings.encode;
        widgets::caps(ui, tr("Préréglages"));
        ui.add_space(2.0);
        let tile_w = ((ui.available_width() - 12.0) / 3.0).floor();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for p in presets() {
                if widgets::preset_tile(ui, p.icon, p.name, *enc == p.encode, tile_w).on_hover_text(p.hint).clicked() {
                    *enc = p.encode;
                }
            }
        });

        ui.add_space(14.0);
        widgets::caps(ui, tr("Réglages"));
        ui.add_space(2.0);
        let kind = enc.container.kind();
        let combo_w = 196.0;
        group(ui, &t, |ui| {
            widgets::setting_row(ui, tr("Format"), |ui| {
                let options: Vec<(Container, String)> = Container::ALL.iter().map(|c| (*c, c.label().to_owned())).collect();
                widgets::combo(ui, tr("Format"), &mut enc.container, &options, combo_w);
            });
            enc.normalize();
            if kind == OutputKind::Video {
                widgets::separator(ui);
                widgets::setting_row(ui, tr("Codec vidéo"), |ui| {
                    let options: Vec<_> = enc.container.codecs().iter().map(|c| (*c, c.label().to_owned())).collect();
                    widgets::combo(ui, tr("Codec vidéo"), &mut enc.codec, &options, combo_w);
                });
            }
            if kind != OutputKind::Audio {
                widgets::separator(ui);
                widgets::setting_row(ui, tr("Résolution"), |ui| {
                    let options: Vec<_> = Resolution::ALL.iter().map(|r| (*r, r.label().to_owned())).collect();
                    widgets::combo(ui, tr("Résolution"), &mut enc.resolution, &options, combo_w);
                });
                widgets::separator(ui);
                widgets::setting_row(ui, tr("Images par seconde"), |ui| {
                    let options: Vec<_> = Fps::ALL.iter().map(|f| (*f, f.label())).collect();
                    widgets::combo(ui, tr("Images par seconde"), &mut enc.fps, &options, combo_w);
                });
                widgets::separator(ui);
                widgets::setting_row(ui, tr("Compression"), |ui| {
                    if kind == OutputKind::Video {
                        widgets::segmented(ui, "mode", &mut enc.mode, &[(Mode::Quality, tr("Qualité")), (Mode::TargetSize, tr("Taille cible"))], combo_w);
                    } else {
                        ui.label(RichText::new(enc.compression_label()).font(FontId::new(13.0, theme::semibold())).color(t.accent_ink));
                    }
                });
                compression_detail(ui, &t, enc);
            }
            if kind == OutputKind::Video || enc.container == Container::Webp {
                widgets::separator(ui);
                // Un encodage lent donne un fichier plus petit à qualité égale.
                widgets::setting_row(ui, tr("Vitesse d'encodage"), |ui| {
                    widgets::segmented(ui, "speed", &mut enc.speed, &[(Speed::Fast, tr("Rapide")), (Speed::Balanced, tr("Moyenne")), (Speed::Slow, tr("Lente"))], combo_w);
                });
            }
            if kind != OutputKind::Animation {
                widgets::separator(ui);
                widgets::setting_row(ui, tr("Son"), |ui| {
                    let options: Vec<_> = Audio::ALL.iter().filter(|a| kind == OutputKind::Video || **a != Audio::None).map(|a| (*a, a.label().to_owned())).collect();
                    widgets::combo(ui, tr("Son"), &mut enc.audio, &options, combo_w);
                });
            }
        });

    }

    /// Le résumé des réglages et le bouton qui lance (ou arrête) le travail.
    fn launch(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let w = ui.available_width();
        widgets::separator(ui);
        ui.add_space(8.0);
        let ready = self.items.iter().filter(|i| i.encodable()).count();
        ui.spacing_mut().item_spacing.y = 0.0;
        // Une ligne de texte, un filet de progression, le bouton : les mêmes places au repos et
        // pendant le travail, pour que rien ne saute.
        let (line, _) = ui.allocate_exact_size(vec2(w, 18.0), egui::Sense::hover());
        ui.add_space(7.0);
        match &self.batch {
            Some(batch) => {
                let current = self.items.iter().find_map(|i| if let State::Running { fraction, .. } = i.state { Some(fraction) } else { None }).unwrap_or(0.0);
                let overall = (batch.finished as f32 + current) / batch.total.max(1) as f32;
                let p = ui.painter();
                let text = tf("Fichier {n} sur {total}", &[("n", &(batch.finished + 1).min(batch.total)), ("total", &batch.total)]);
                p.text(line.left_center(), Align2::LEFT_CENTER, text, FontId::new(12.5, theme::medium()), t.muted);
                p.text(line.right_center(), Align2::RIGHT_CENTER, format!("{} %", (overall * 100.0).round() as u32), FontId::new(12.5, theme::semibold()), t.ink_strong);
                widgets::progress(ui, overall, t.accent, w, 4.0);
                ui.add_space(10.0);
                if widgets::button_sized(ui, Some(Icon::X), tr("Arrêter"), Kind::Danger, 42.0, Some(w)).clicked() {
                    self.actions.push(Action::Cancel);
                }
            }
            None => {
                let summary = widgets::elided(ui, &self.settings.encode.summary(), &FontId::new(12.5, theme::medium()), w);
                ui.painter().text(line.left_center(), Align2::LEFT_CENTER, summary, FontId::new(12.5, theme::medium()), t.muted);
                ui.add_space(14.0);
                let convert = self.settings.encode.container.kind() != OutputKind::Video;
                let label = match (ready, convert) {
                    (0 | 1, false) => tr("Compresser").to_owned(),
                    (0 | 1, true) => tr("Convertir").to_owned(),
                    (n, false) => tf("Compresser {n} fichiers", &[("n", &n)]),
                    (n, true) => tf("Convertir {n} fichiers", &[("n", &n)]),
                };
                let resp = ui.add_enabled_ui(ready > 0, |ui| widgets::button_sized(ui, Some(Icon::Zap), &label, Kind::Primary, 42.0, Some(w))).inner;
                if resp.clicked() {
                    self.actions.push(Action::Start);
                }
            }
        }
    }

    // ---- fichiers (zone centrale) ------------------------------------------------------------------

    fn files(&mut self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        let running = self.running();
        ui.horizontal(|ui| {
            widgets::title(ui, tr("Fichiers"), 19.0);
            if !self.items.is_empty() {
                ui.add_space(2.0);
                widgets::chip(ui, &self.items.len().to_string(), Tone::Neutral);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::button_sized(ui, Some(Icon::Plus), tr("Ajouter"), Kind::Secondary, 30.0, None).clicked() {
                    self.actions.push(Action::PickFiles);
                }
                if !self.items.is_empty() && !running && widgets::button_sized(ui, None, tr("Tout retirer"), Kind::Ghost, 30.0, None).clicked() {
                    self.actions.push(Action::Clear);
                }
            });
        });
        ui.add_space(8.0);
        if !self.menu_on && !self.settings.menu_prompt_dismissed {
            banner(ui, &t, Icon::MousePointerClick, t.accent_ink, tr("Ajouter Video Compressor au clic droit"), tr("Un clic droit sur une vidéo ouvrira cette fenêtre, le fichier déjà chargé."), |ui| {
                if widgets::button_sized(ui, None, tr("Plus tard"), Kind::Ghost, 30.0, None).clicked() {
                    self.actions.push(Action::DismissMenuPrompt);
                }
                if widgets::button_sized(ui, None, tr("Activer"), Kind::Secondary, 30.0, None).clicked() {
                    self.actions.push(Action::MenuOn);
                }
            });
        }

        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        if self.items.is_empty() {
            let size = vec2(ui.available_width(), (ui.available_height() - 2.0).max(180.0));
            let (rect, resp) = widgets::drop_zone(ui, size, hovering, tr("Choisir des vidéos"));
            let p = ui.painter();
            let c = rect.center();
            let tile = egui::Rect::from_center_size(c - vec2(0.0, 46.0), vec2(52.0, 52.0));
            p.rect(tile, egui::CornerRadius::same(12), t.surface, Stroke::new(1.0, t.line_strong), egui::StrokeKind::Inside);
            icons::paint(p, tile.center(), Icon::Clapperboard, 26.0, t.accent_ink);
            p.text(c + vec2(0.0, 2.0), Align2::CENTER_CENTER, tr("Dépose tes vidéos ici"), FontId::new(17.0, theme::bold()), t.ink_strong);
            p.text(c + vec2(0.0, 26.0), Align2::CENTER_CENTER, tr("ou clique pour les choisir"), FontId::new(13.0, egui::FontFamily::Proportional), t.muted);
            p.text(c + vec2(0.0, 58.0), Align2::CENTER_CENTER, tr("MP4, MOV, MKV, WebM, AVI, GIF, WebP animé et bien d'autres"), FontId::new(12.0, egui::FontFamily::Proportional), t.faint);
            if resp.clicked() {
                self.actions.push(Action::PickFiles);
            }
            return;
        }

        let list = ui.available_rect_before_wrap();
        ScrollArea::vertical().id_salt("files-scroll").auto_shrink(false).show(ui, |ui| {
            Frame::new().fill(t.surface).stroke(Stroke::new(1.0, t.line)).corner_radius(theme::R_CARD).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, item) in self.items.iter().enumerate() {
                    if i > 0 {
                        widgets::separator(ui);
                    }
                    file_row(ui, &t, item, &mut self.actions);
                }
            });
        });
        if hovering {
            let p = ui.painter();
            p.rect_filled(list, egui::CornerRadius::same(theme::R_CARD), theme::tint(t.bg, 0.86));
            p.rect_stroke(list.shrink(1.0), egui::CornerRadius::same(theme::R_CARD), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
            p.text(list.center(), Align2::CENTER_CENTER, tr("Lâche pour ajouter à la liste"), FontId::new(16.0, theme::bold()), t.ink_strong);
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
                    ui.label(RichText::new(&toast.text).size(13.5).color(t.ink_strong));
                });
            });
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
    }
}

/// Un groupe de lignes de réglage : un seul contour, des filets entre les lignes.
fn group(ui: &mut Ui, t: &Tokens, add: impl FnOnce(&mut Ui)) {
    Frame::new().fill(t.bg).stroke(Stroke::new(1.0, t.line)).corner_radius(theme::R_CARD).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        add(ui);
    });
}

/// Sous la ligne « Compression » : le curseur de qualité, ou la taille à ne pas dépasser.
fn compression_detail(ui: &mut Ui, t: &Tokens, enc: &mut Encode) {
    Frame::new().inner_margin(Margin { left: 14, right: 12, top: 0, bottom: 12 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        if enc.mode == Mode::TargetSize {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("Le fichier ne dépassera pas")).size(12.5).color(t.muted));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let field = egui::DragValue::new(&mut enc.target_mb).range(1.0..=50_000.0).speed(0.5).max_decimals(1).suffix(format!(" {}", tr("Mo")));
                    widgets::named(ui.add_sized(vec2(96.0, 28.0), field), egui::Role::SpinButton, tr("Taille à ne pas dépasser"));
                });
            });
            return;
        }
        ui.spacing_mut().slider_width = ui.available_width();
        let mut value = f32::from(enc.compression);
        let slider = egui::Slider::new(&mut value, 0.0..=100.0).step_by(1.0).show_value(false).trailing_fill(true);
        if widgets::named(ui.add(slider), egui::Role::Slider, tr("Degré de compression")).changed() {
            enc.compression = value.round() as u8;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Plus net, plus lourd")).size(11.5).color(t.faint));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(tr("Plus léger")).size(11.5).color(t.faint));
                if enc.container.kind() == OutputKind::Video {
                    // Au milieu : le mot, et le facteur que les habitués de FFmpeg reconnaissent.
                    let text = format!("{} · CRF {}", enc.compression_label(), engine::crf(enc));
                    let w = ui.available_width();
                    ui.allocate_ui_with_layout(vec2(w, 16.0), Layout::centered_and_justified(egui::Direction::LeftToRight), |ui| {
                        ui.label(RichText::new(text).font(FontId::new(11.5, theme::semibold())).color(t.accent_ink));
                    });
                }
            });
        });
    });
}

/// Un bandeau d'information au-dessus de la liste : icône, titre, phrase, et ses boutons à droite.
fn banner(ui: &mut Ui, t: &Tokens, icon: Icon, color: Color32, title: &str, description: &str, buttons: impl FnOnce(&mut Ui)) {
    widgets::card(t).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            widgets::icon_tile(ui, icon, color, 34.0);
            ui.add_space(2.0);
            let text_w = (ui.available_width() - 250.0).max(120.0);
            ui.vertical(|ui| {
                ui.set_width(text_w);
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(RichText::new(title).font(FontId::new(13.5, theme::semibold())).color(t.ink_strong));
                ui.label(RichText::new(description).size(12.5).color(t.muted));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                buttons(ui);
            });
        });
    });
    ui.add_space(8.0);
}

/// « 1920 × 1080 · 30 i/s · 1:23 · 48,2 Mo »
fn meta_line(item: &Item) -> String {
    let Some(info) = &item.info else { return item.path.parent().map(|p| p.display().to_string()).unwrap_or_default() };
    let mut parts = Vec::new();
    if info.has_video && info.width > 0 {
        parts.push(format!("{} × {}", info.width, info.height));
        if info.fps > 0.0 {
            parts.push(tf("{n} i/s", &[("n", &i18n::decimal(info.fps, if (info.fps - info.fps.round()).abs() < 0.01 { 0 } else { 2 }))]));
        }
    } else if info.has_audio {
        parts.push(tr("Son seul").to_owned());
    }
    if info.duration > 0.0 {
        parts.push(i18n::clock(info.duration));
    }
    parts.push(i18n::size(info.size));
    if info.has_video && !info.vcodec.is_empty() {
        parts.push(if info.webp.is_some() { tr("WebP animé").to_owned() } else { info.vcodec.to_uppercase() });
    }
    parts.join("  ·  ")
}

fn file_row(ui: &mut Ui, t: &Tokens, item: &Item, actions: &mut Vec<Action>) {
    const H: f32 = 54.0;
    Frame::new().inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.set_height(H);
            let placeholder = if item.info.as_ref().is_some_and(|i| !i.has_video) { Icon::Music } else { Icon::Film };
            widgets::thumbnail(ui, item.thumb.as_ref(), vec2(96.0, H), placeholder);
            ui.add_space(4.0);
            // La place des contrôles de droite dépend de l'état : le nom garde le reste.
            let reserved = match item.state {
                State::Running { .. } => 190.0,
                State::Done { .. } => 140.0,
                State::Failed(_) => 176.0,
                _ => 150.0,
            };
            let text_w = (ui.available_width() - reserved).max(80.0);
            ui.vertical(|ui| {
                ui.set_width(text_w);
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.add_space(8.0);
                widgets::one_line(ui, &item.name, FontId::new(14.0, theme::semibold()), t.ink_strong, text_w).on_hover_text(item.path.display().to_string());
                match (&item.state, &item.info) {
                    // Terminé : le poids avant, le poids après, et le gain.
                    (State::Done { output, size }, Some(info)) => {
                        let (gain, color) = if *size <= info.size {
                            (tf("{n} % de moins", &[("n", &(((info.size - *size) as f64 / info.size.max(1) as f64 * 100.0).round() as u32))]), t.good_fg)
                        } else {
                            (tf("{n} % de plus", &[("n", &(((*size - info.size) as f64 / info.size.max(1) as f64 * 100.0).round() as u32))]), t.accent_ink)
                        };
                        let text = format!("{}  →  {}  ·  {gain}", i18n::size(info.size), i18n::size(*size));
                        widgets::one_line(ui, &text, FontId::new(12.0, theme::medium()), color, text_w).on_hover_text(output.display().to_string());
                    }
                    _ => {
                        widgets::one_line(ui, &meta_line(item), FontId::new(12.0, egui::FontFamily::Proportional), t.muted, text_w);
                    }
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let busy = matches!(item.state, State::Queued | State::Running { .. });
                if !busy && widgets::icon_button(ui, Icon::X, tr("Retirer de la liste"), 28.0).clicked() {
                    actions.push(Action::Remove(item.id));
                }
                match &item.state {
                    State::Probing => {
                        ui.label(RichText::new(tr("Analyse…")).size(12.5).color(t.muted));
                        ui.add(egui::Spinner::new().size(14.0).color(t.muted));
                    }
                    State::Ready => {
                        widgets::chip(ui, tr("Prêt"), Tone::Neutral);
                    }
                    State::Queued => {
                        widgets::chip(ui, tr("En attente"), Tone::Neutral);
                    }
                    State::Running { fraction, started, .. } => {
                        let elapsed = started.elapsed().as_secs_f64();
                        let left = if *fraction > 0.02 { Some(elapsed / f64::from(*fraction) * f64::from(1.0 - *fraction)) } else { None };
                        let percent = format!("{} %", (fraction * 100.0).round() as u32);
                        let text = match left {
                            Some(s) => tf("{percent}, reste {left}", &[("percent", &percent), ("left", &i18n::clock(s))]),
                            None => percent,
                        };
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.set_width(170.0);
                            ui.spacing_mut().item_spacing.y = 5.0;
                            ui.add_space(14.0);
                            ui.with_layout(Layout::top_down(Align::RIGHT), |ui| {
                                ui.label(RichText::new(text).font(FontId::new(12.0, theme::medium())).color(t.ink));
                                widgets::progress(ui, *fraction, t.accent, 170.0, 5.0);
                            });
                        });
                    }
                    State::Done { output, .. } => {
                        if widgets::icon_button(ui, Icon::RotateCcw, tr("Recommencer avec les réglages du moment"), 28.0).clicked() {
                            actions.push(Action::Retry(item.id));
                        }
                        if widgets::icon_button(ui, Icon::FolderOpen, tr("Montrer dans le dossier"), 28.0).clicked() {
                            actions.push(Action::Reveal(output.clone()));
                        }
                        if widgets::icon_button(ui, Icon::Play, tr("Ouvrir le fichier"), 28.0).clicked() {
                            actions.push(Action::Open(output.clone()));
                        }
                    }
                    State::Failed(why) => {
                        if item.info.is_some() && widgets::icon_button(ui, Icon::RotateCcw, tr("Réessayer"), 28.0).clicked() {
                            actions.push(Action::Retry(item.id));
                        }
                        if widgets::icon_button(ui, Icon::Copy, tr("Copier le détail de l'erreur"), 28.0).clicked() {
                            actions.push(Action::Copy(why.clone()));
                        }
                        widgets::chip(ui, tr("Échec"), Tone::Danger).on_hover_text(why.as_str());
                    }
                }
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
        // Le temps restant avance même quand FFmpeg ne dit rien.
        if self.running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        let edge = widgets::edge_under_pointer(&ctx);
        let t = Tokens::get(&ctx);

        egui::Panel::top("topbar").exact_size(theme::TOPBAR_H).resizable(false).show_separator_line(false).frame(Frame::new().fill(t.bg)).show(ui, |ui| self.top_bar(ui, edge.is_some()));
        egui::Panel::bottom("statusbar").exact_size(theme::STATUS_H).resizable(false).show_separator_line(false).frame(Frame::new().fill(t.bg)).show(ui, |ui| self.status_bar(ui));
        egui::Panel::right("settings").exact_size(theme::PANEL_W).resizable(false).show_separator_line(false).frame(Frame::new().fill(t.surface)).show(ui, |ui| self.settings_panel(ui));
        egui::CentralPanel::default().frame(Frame::new().fill(t.bg).inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 16 })).show(ui, |ui| self.files(ui));

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
