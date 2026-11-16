//! Les deux fenêtres modales : les préférences, et la mise à jour (la dernière release publiée sur
//! GitHub, ses points de version, le téléchargement vérifié, puis le redémarrage qui l'installe).

use egui::{Align, FontId, Id, Layout, RichText, Ui};

use crate::app::{Action, App};
use crate::i18n::{self, Lang, tf, tr};
use crate::icons::{self, Icon};
use crate::release::{self, Phase, VERSION};
use crate::shell;
use crate::theme::{self, Accent, Tokens};
use crate::widgets::{self, Kind, Tone};

/// L'en-tête d'une fenêtre : tuile d'icône, titre, sous-titre, et la croix. Renvoie `true` si on ferme.
fn header(ui: &mut Ui, t: &Tokens, icon: Icon, title: &str, subtitle: &str) -> bool {
    let mut close = false;
    ui.horizontal_top(|ui| {
        widgets::icon_tile(ui, icon, t.accent_ink, 40.0);
        ui.add_space(2.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            widgets::title(ui, title, 19.0);
            ui.label(RichText::new(subtitle).size(12.5).color(t.muted));
        });
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            close = widgets::icon_button(ui, Icon::X, tr("Fermer"), 30.0).clicked();
        });
    });
    ui.add_space(14.0);
    close
}

/// Un bloc des préférences : un petit titre, puis son contenu dans un cadre.
fn section(ui: &mut Ui, t: &Tokens, title: &str, add: impl FnOnce(&mut Ui)) {
    widgets::caps(ui, title);
    ui.add_space(2.0);
    widgets::card(t).fill(t.bg).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 10.0;
        add(ui);
    });
    ui.add_space(12.0);
}

pub fn prefs(ctx: &egui::Context, app: &mut App) {
    let t = Tokens::get(ctx);
    let mut open = true;
    let modal = egui::Modal::new(Id::unique("prefs")).backdrop_color(theme::scrim()).frame(widgets::modal_frame(&t)).show(ctx, |ui| {
        ui.set_width(540.0);
        if header(ui, &t, Icon::Settings, tr("Préférences"), &tf("Video Compressor {VERSION}", &[("VERSION", &VERSION)])) {
            open = false;
        }
        // Une fenêtre modale d'egui ne dépasse pas 400 points de haut si on ne lui dit rien : la
        // zone défilante réclame la hauteur de la fenêtre, moins de quoi garder l'en-tête visible.
        let height = (ctx.viewport_rect().height() - 190.0).clamp(220.0, 620.0);
        egui::ScrollArea::vertical().id_salt("prefs-scroll").max_height(height).min_scrolled_height(height).auto_shrink([false, true]).show(ui, |ui| {
            // La barre de défilement ne colle pas aux cadres.
            ui.set_width(ui.available_width() - 12.0);
            section(ui, &t, tr("Clic droit"), |ui| {
                let mut on = app.menu_on;
                let label = tf("Proposer « {0} » sur les vidéos", &[("0", &shell::menu_label())]);
                if widgets::toggle_row(ui, &mut on, &label, Some(shell::menu_hint())).changed() {
                    app.actions.push(if on { Action::MenuOn } else { Action::MenuOff });
                }
            });

            section(ui, &t, tr("Fichiers produits"), |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr("Dossier")).font(FontId::new(13.5, theme::semibold())).color(t.ink_strong));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let custom = !app.settings.out_dir.is_empty();
                        if custom && widgets::icon_button(ui, Icon::X, tr("Revenir au dossier de l'original"), 28.0).clicked() {
                            app.settings.out_dir.clear();
                        }
                        if widgets::icon_button(ui, Icon::FolderOpen, tr("Choisir un dossier"), 28.0).clicked() {
                            app.actions.push(Action::PickOutDir);
                        }
                        let shown = if custom { app.settings.out_dir.clone() } else { tr("Celui de l'original").to_owned() };
                        let font = if custom { widgets::mono(11.5) } else { FontId::new(13.0, egui::FontFamily::Proportional) };
                        let max_w = (ui.available_width() - 8.0).max(40.0);
                        widgets::one_line(ui, &shown, font, t.muted, max_w).on_hover_text(shown.as_str());
                    });
                });
                widgets::separator(ui);
                ui.horizontal(|ui| {
                    let width = ui.available_width();
                    ui.vertical(|ui| {
                        ui.set_width(width - 170.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new(tr("Ajouté au nom")).font(FontId::new(13.5, theme::semibold())).color(t.ink_strong));
                        ui.label(RichText::new(tr("« vacances.mov » devient « vacances-compresse.mp4 ». L'original n'est jamais modifié.")).size(12.5).color(t.muted));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        widgets::named(widgets::text_input(ui, &mut app.settings.suffix, "", 150.0, true), egui::Role::TextInput, tr("Ajouté au nom"));
                    });
                });
            });

            section(ui, &t, tr("Mises à jour"), |ui| {
                widgets::toggle_row(ui, &mut app.settings.update_auto, tr("Vérifier au démarrage"), Some(tr("Une seule question à GitHub, à l'ouverture de la fenêtre.")));
                widgets::separator(ui);
                widgets::toggle_row(ui, &mut app.settings.update_beta, tr("Recevoir les versions bêta"), Some(tr("Les nouveautés plus tôt, avec le risque d'un défaut.")));
            });

            section(ui, &t, tr("Apparence"), |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr("Couleur d'accent")).font(FontId::new(13.5, theme::semibold())).color(t.ink_strong));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for accent in Accent::ALL.into_iter().rev() {
                            if widgets::swatch(ui, accent.swatch(), app.settings.accent == accent, accent.label()).clicked() {
                                app.settings.accent = accent;
                            }
                        }
                    });
                });
                widgets::separator(ui);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr("Langue")).font(FontId::new(13.5, theme::semibold())).color(t.ink_strong));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let mut options: Vec<(Option<Lang>, String)> = vec![(None, tr("Celle du système").to_owned())];
                        options.extend(Lang::ALL.into_iter().map(|l| (Some(l), l.native().to_owned())));
                        let mut lang = app.settings.language;
                        if widgets::combo(ui, tr("Langue"), &mut lang, &options, 180.0) {
                            app.actions.push(Action::Language(lang));
                        }
                    });
                });
            });
        });
    });
    if !open || modal.should_close() {
        app.actions.push(Action::CloseDialog);
    }
}

pub fn update(ctx: &egui::Context, app: &mut App) {
    let t = Tokens::get(ctx);
    let u = &app.updater;
    let act = &mut app.actions;
    let mut open = true;
    let modal = egui::Modal::new(Id::unique("update")).backdrop_color(theme::scrim()).frame(widgets::modal_frame(&t)).show(ctx, |ui| {
        ui.set_width(500.0);
        if header(ui, &t, Icon::CircleArrowUp, tr("Mise à jour"), &tf("Version installée {VERSION}", &[("VERSION", &VERSION)])) {
            open = false;
        }
        let w = ui.available_width();
        let dev = release::is_dev_build();
        match u.phase {
            Phase::Idle | Phase::UpToDate | Phase::Failed => {
                ui.horizontal(|ui| match u.phase {
                    Phase::Failed => {
                        icons::show(ui, Icon::TriangleAlert, 18.0, t.danger);
                        ui.label(RichText::new(u.error.as_deref().unwrap_or(tr("Vérification impossible"))).size(13.5).color(t.ink));
                    }
                    Phase::UpToDate => {
                        icons::show(ui, Icon::CircleCheck, 18.0, t.good_fg);
                        ui.label(RichText::new(tf("Video Compressor {VERSION} est à jour.", &[("VERSION", &VERSION)])).size(14.0).color(t.ink));
                    }
                    _ => {
                        icons::show(ui, Icon::Info, 18.0, t.muted);
                        ui.label(RichText::new(tr("Pas encore vérifié.")).size(14.0).color(t.ink));
                    }
                });
                ui.add_space(10.0);
                let label = if u.phase == Phase::Failed { tr("Réessayer") } else { tr("Vérifier maintenant") };
                if widgets::button(ui, Some(Icon::RefreshCw), label, Kind::Secondary).clicked() {
                    act.push(Action::CheckUpdate);
                }
            }
            Phase::Checking => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(16.0).color(t.accent));
                    ui.label(RichText::new(tr("Recherche d'une nouvelle version…")).size(14.0).color(t.ink));
                });
            }
            Phase::Available | Phase::Downloading => {
                let Some(r) = &u.release else { return };
                widgets::card(&t).fill(t.bg).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(tf("Version {0}", &[("0", &r.version)])).font(FontId::new(16.5, theme::bold())).color(t.ink_strong));
                        widgets::chip(ui, if r.prerelease { tr("Bêta") } else { tr("Nouvelle") }, Tone::Accent);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let size = r.asset.as_ref().map(|a| i18n::size(a.size)).unwrap_or_default();
                            let when = if r.published.is_empty() { String::new() } else { tf("publiée le {0}", &[("0", &r.published)]) };
                            ui.label(RichText::new([size, when].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("  ·  ")).size(12.0).color(t.faint));
                        });
                    });
                    ui.add_space(2.0);
                    if r.notes.is_empty() {
                        ui.label(RichText::new(tr("Pas de notes pour cette version.")).size(13.0).color(t.muted));
                    }
                    for note in &r.notes {
                        ui.label(RichText::new(format!("•  {note}")).size(13.5).color(t.ink));
                    }
                    if !r.page.is_empty() {
                        ui.add_space(2.0);
                        ui.hyperlink_to(RichText::new(tr("Voir la version sur GitHub")).size(12.5), &r.page);
                    }
                });
                ui.add_space(12.0);
                if u.phase == Phase::Downloading {
                    let (done, total) = u.progress;
                    ui.label(RichText::new(tf("Téléchargement, {done} sur {total}", &[("done", &i18n::size(done)), ("total", &i18n::size(total))])).size(13.0).color(t.ink));
                    widgets::progress(ui, done as f32 / total.max(1) as f32, t.accent, w, 6.0);
                } else {
                    ui.horizontal(|ui| {
                        let installable = !dev && r.asset.is_some();
                        ui.add_enabled_ui(installable, |ui| {
                            if widgets::button(ui, Some(Icon::Download), tr("Télécharger et installer"), Kind::Primary).clicked() {
                                act.push(Action::DownloadUpdate);
                            }
                        });
                        if widgets::button(ui, None, tr("Plus tard"), Kind::Ghost).clicked() {
                            open = false;
                        }
                    });
                    if dev {
                        ui.label(RichText::new(tr("Version de développement, compilée sur place. Les mises à jour s'installent sur l'exécutable d'une release.")).size(12.0).color(t.faint));
                    } else if r.asset.is_none() {
                        ui.label(RichText::new(tr("Cette version n'a pas de fichier pour cette plateforme.")).size(12.0).color(t.faint));
                    }
                }
            }
            Phase::Ready => {
                let version = u.release.as_ref().map(|r| r.version.as_str()).unwrap_or("");
                ui.horizontal(|ui| {
                    icons::show(ui, Icon::PackageCheck, 20.0, t.good_fg);
                    ui.label(RichText::new(tf("La version {version} est prête.", &[("version", &version)])).font(FontId::new(14.5, theme::semibold())).color(t.ink_strong));
                });
                ui.label(RichText::new(tr("Video Compressor va se fermer et se rouvrir pour l'appliquer. Tes réglages sont conservés.")).size(13.0).color(t.muted));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if widgets::button(ui, Some(Icon::RotateCw), tr("Redémarrer maintenant"), Kind::Primary).clicked() {
                        act.push(Action::Restart);
                    }
                    if widgets::button(ui, None, tr("Au prochain lancement"), Kind::Ghost).clicked() {
                        open = false;
                    }
                });
            }
        }
        ui.add_space(12.0);
        widgets::separator(ui);
        ui.add_space(4.0);
        ui.label(RichText::new(tf("Versions publiées sur github.com/{0}", &[("0", &release::REPO)])).size(12.0).color(t.faint));
    });
    if !open || modal.should_close() {
        app.actions.push(Action::CloseDialog);
    }
}
