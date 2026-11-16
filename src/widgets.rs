//! La boîte à widgets, reprise de Rust Project. Chaque widget suit le même schéma : réserver un
//! rectangle, lire l'interaction, peindre avec les tokens, et se décrire (`widget_info`) pour les
//! lecteurs d'écran et les tests.

use egui::{
    Align, Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Frame, Id, Layout, Margin, Rect, ResizeDirection, Response, Role, Sense, Stroke,
    StrokeKind, Ui, Vec2, ViewportCommand, WidgetInfo, pos2, vec2,
};

use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::theme::{self, R_CARD, R_CHIP, R_CTRL, Tokens, mix, tint};

fn tok(ui: &Ui) -> Tokens {
    Tokens::get(ui.ctx())
}

fn text_size(ui: &Ui, text: &str, font: &FontId) -> Vec2 {
    ui.painter().layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER).size()
}

// ---- textes ------------------------------------------------------------------------------------

/// Titre de section.
pub fn title(ui: &mut Ui, text: &str, size: f32) -> Response {
    let t = tok(ui);
    ui.label(egui::RichText::new(text).font(FontId::new(size, theme::bold())).color(t.ink_strong))
}

/// Petit label en capitales espacées (« PRÉRÉGLAGES »).
pub fn caps(ui: &mut Ui, text: &str) -> Response {
    let t = tok(ui);
    let format = egui::TextFormat { font_id: FontId::new(10.5, theme::semibold()), extra_letter_spacing: 1.1, color: t.faint, ..Default::default() };
    let galley = ui.painter().layout_job(egui::text::LayoutJob::single_section(text.to_uppercase(), format));
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, t.faint);
    resp
}

/// Un texte d'une ligne, coupé par « … » s'il dépasse `max_w`.
pub fn one_line(ui: &mut Ui, text: &str, font: FontId, color: Color32, max_w: f32) -> Response {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap.max_width = max_w.max(10.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let galley = ui.painter().layout_job(job);
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::hover());
    resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, text));
    ui.painter().galley(rect.min, galley, color);
    resp
}

/// `text` coupé par « … » pour tenir dans `max_w` (pour un texte peint à la main).
pub fn elided(ui: &Ui, text: &str, font: &FontId, max_w: f32) -> String {
    let width = |s: &str| text_size(ui, s, font).x;
    if width(text) <= max_w {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let s: String = chars[..mid].iter().chain(['…'].iter()).collect();
        if width(&s) <= max_w { lo = mid } else { hi = mid - 1 }
    }
    chars[..lo].iter().chain(['…'].iter()).collect()
}

// ---- pastilles ---------------------------------------------------------------------------------

/// Une pastille dans la couleur d'accent (« Nouvelle », « Bêta »).
pub fn chip(ui: &mut Ui, text: &str) -> Response {
    let t = tok(ui);
    let (bg, fg, stroke) = (t.accent_soft, t.accent_ink, Stroke::new(1.0, tint(t.accent, 0.55)));
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(12.0, theme::semibold()), fg);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 16.0, 22.0), Sense::hover());
    resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, text));
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(R_CHIP), bg, stroke, StrokeKind::Inside);
    p.galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

// ---- boutons -----------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Plein, couleur d'accent.
    Primary,
    /// Contour.
    Secondary,
    /// Texte seul, fond au survol.
    Ghost,
}

pub fn button(ui: &mut Ui, icon: Option<Icon>, text: &str, kind: Kind) -> Response {
    button_sized(ui, icon, text, kind, theme::CONTROL_H, None)
}

/// Bouton complet : icône optionnelle, hauteur, largeur imposée (sinon ajustée au texte).
pub fn button_sized(ui: &mut Ui, icon: Option<Icon>, text: &str, kind: Kind, height: f32, width: Option<f32>) -> Response {
    let t = tok(ui);
    let big = height >= 42.0;
    let font = FontId::new(if big { 15.0 } else { 13.5 }, theme::semibold());
    let text_w = text_size(ui, text, &font).x;
    let icon_size = if big { 18.0 } else { 16.0 };
    let icon_w = if icon.is_some() { icon_size + if text.is_empty() { 0.0 } else { 8.0 } } else { 0.0 };
    let natural = text_w + icon_w + if text.is_empty() { 0.0 } else { 26.0 };
    let w = width.unwrap_or(natural.max(height));
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, text));
    let hover = ui.ctx().animate_bool(resp.id.with("hover"), resp.hovered());
    let (mut fill, mut fg, mut stroke) = match kind {
        Kind::Primary => (mix(t.accent, t.accent_hover, hover), t.on_accent, Stroke::NONE),
        Kind::Secondary => (mix(Color32::TRANSPARENT, t.raised, hover), t.ink_strong, Stroke::new(1.0, mix(t.line_strong, t.faint, hover))),
        Kind::Ghost => (tint(t.ink, 0.07 * hover), t.ink, Stroke::NONE),
    };
    if !enabled {
        // Désactivé : neutre et lisible, plutôt qu'un accent délavé.
        fill = t.raised;
        fg = t.faint;
        stroke = Stroke::NONE;
    }
    let rect = if resp.is_pointer_button_down_on() { rect.translate(vec2(0.0, 1.0)) } else { rect };
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(R_CTRL), fill, stroke, StrokeKind::Inside);
    if kind == Kind::Primary && enabled {
        // Un reflet d'un pixel en haut du bouton principal : il prend du relief sans ombre.
        p.hline(rect.left() + 5.0..=rect.right() - 5.0, rect.top() + 1.0, Stroke::new(1.0, Color32::from_white_alpha(70)));
    }
    let galley = p.layout_no_wrap(text.to_owned(), font, fg);
    let content_w = icon_w + galley.size().x;
    let mut x = rect.center().x - content_w / 2.0;
    if let Some(icon) = icon {
        icons::paint(p, pos2(x + icon_size / 2.0, rect.center().y), icon, icon_size, fg);
        x += icon_w;
    }
    p.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, fg);
    if resp.has_focus() {
        p.rect_stroke(rect.expand(2.0), CornerRadius::same(R_CTRL + 2), Stroke::new(1.5, t.accent), StrokeKind::Outside);
    }
    if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp }
}

/// Bouton carré portant une icône, avec une bulle d'aide.
pub fn icon_button(ui: &mut Ui, icon: Icon, tip: &str, size: f32) -> Response {
    let t = tok(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, tip));
    let hover = ui.ctx().animate_bool(resp.id.with("hover"), resp.hovered());
    ui.painter().rect_filled(rect, CornerRadius::same(R_CTRL), tint(t.ink, 0.08 * hover));
    icons::paint(ui.painter(), rect.center(), icon, size * 0.54, mix(t.muted, t.ink_strong, hover));
    resp.on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand)
}

// ---- choix -------------------------------------------------------------------------------------

/// Interrupteur arrondi.
pub fn switch(ui: &mut Ui, on: &mut bool, a11y_label: &str) -> Response {
    let t = tok(ui);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(38.0, 21.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let state = *on;
    resp.widget_info(|| WidgetInfo::selected(Role::Switch, true, state, a11y_label));
    let k = ui.ctx().animate_bool(resp.id.with("on"), *on);
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(11), mix(t.input, t.accent, k), Stroke::new(1.0, mix(t.line_strong, t.accent, k)), StrokeKind::Inside);
    let x = egui::lerp((rect.left() + 10.5)..=(rect.right() - 10.5), k);
    p.circle_filled(pos2(x, rect.center().y), 7.0, mix(t.muted, t.on_accent, k));
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Ligne de réglage : titre et description à gauche, interrupteur à droite.
pub fn toggle_row(ui: &mut Ui, on: &mut bool, title: &str, description: Option<&str>) -> Response {
    let t = tok(ui);
    ui.horizontal(|ui| {
        let width = ui.available_width();
        ui.vertical(|ui| {
            ui.set_width(width - 56.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(egui::RichText::new(title).font(FontId::new(13.5, theme::semibold())).color(t.ink_strong));
            if let Some(d) = description {
                ui.label(egui::RichText::new(d).size(12.5).color(t.muted));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| switch(ui, on, title)).inner
    })
    .inner
}

/// Champ texte d'une ligne.
pub fn text_input(ui: &mut Ui, value: &mut String, hint: &str, width: f32, mono: bool) -> Response {
    let mut edit = egui::TextEdit::singleline(value).hint_text(hint).desired_width(width).min_size(vec2(width, theme::CONTROL_H)).align(Align2::LEFT_CENTER).margin(Margin::symmetric(10, 7));
    if mono {
        edit = edit.font(egui::TextStyle::Monospace);
    }
    ui.add(edit)
}

/// Donne un nom accessible à un widget standard qui n'en a pas (curseur, compteur…).
pub fn named(resp: Response, role: Role, name: &str) -> Response {
    resp.widget_info(|| WidgetInfo::labeled(role, true, name));
    resp
}

/// Liste déroulante. `name` sert d'identifiant et de nom accessible.
pub fn combo<T: Copy + PartialEq>(ui: &mut Ui, name: &str, value: &mut T, options: &[(T, String)], width: f32) -> bool {
    let current = options.iter().find(|(v, _)| *v == *value).map(|(_, l)| l.as_str()).unwrap_or("");
    let mut changed = false;
    let inner = egui::ComboBox::from_id_salt(name).selected_text(current).width(width).show_ui(ui, |ui| {
        for (v, label) in options {
            if ui.selectable_value(value, *v, label.as_str()).changed() {
                changed = true;
            }
        }
    });
    inner.response.widget_info(|| WidgetInfo::labeled(Role::ComboBox, true, name));
    changed
}

// ---- conteneurs et petits éléments -------------------------------------------------------------

/// Le cadre d'une carte.
pub fn card(t: &Tokens) -> Frame {
    Frame::new().fill(t.surface).stroke(Stroke::new(1.0, t.line)).corner_radius(R_CARD).inner_margin(Margin::same(14))
}

/// Le cadre d'une fenêtre modale.
pub fn modal_frame(t: &Tokens) -> Frame {
    Frame::new().fill(t.surface).stroke(Stroke::new(1.0, t.line_strong)).corner_radius(14).shadow(theme::shadow()).inner_margin(Margin::same(22))
}

/// Carré arrondi teinté portant une icône.
pub fn icon_tile(ui: &mut Ui, icon: Icon, color: Color32, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same((size * 0.24) as u8), tint(color, 0.16));
    icons::paint(ui.painter(), rect.center(), icon, size * 0.54, color);
    resp
}

/// Barre de progression arrondie.
pub fn progress(ui: &mut Ui, fraction: f32, color: Color32, width: f32, height: f32) -> Response {
    let t = tok(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let r = CornerRadius::same((height / 2.0) as u8);
    ui.painter().rect_filled(rect, r, t.neutral_bg);
    let done = Rect::from_min_size(rect.min, vec2(rect.width() * fraction.clamp(0.0, 1.0), rect.height()));
    if done.width() > 0.5 {
        ui.painter().rect_filled(done, r, color);
    }
    resp
}

/// Pastille de couleur sélectionnable.
pub fn swatch(ui: &mut Ui, color: Color32, selected: bool, label: &str) -> Response {
    let t = tok(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, selected, label));
    let p = ui.painter();
    if selected {
        p.circle_stroke(rect.center(), 13.0, Stroke::new(2.0, t.ink_strong));
    } else if resp.hovered() {
        p.circle_stroke(rect.center(), 13.0, Stroke::new(1.0, t.line_strong));
    }
    p.circle_filled(rect.center(), 9.5, color);
    resp.on_hover_text(label).on_hover_cursor(CursorIcon::PointingHand)
}

/// Filet horizontal sur toute la largeur.
pub fn separator(ui: &mut Ui) {
    let t = tok(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, t.hairline());
}

/// Filet vertical.
pub fn vsep(ui: &mut Ui, height: f32) {
    let t = tok(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, height), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), t.hairline());
}

// ---- fenêtre sans bordure ----------------------------------------------------------------------

/// Glisser la zone déplace la fenêtre, double-cliquer la maximise. À appeler avant les boutons.
pub fn title_bar_drag(ui: &mut Ui, rect: Rect) {
    let resp = ui.interact(rect, Id::unique("title-bar-drag"), Sense::click_and_drag());
    if resp.double_clicked() {
        let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if resp.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

/// Réduire / agrandir / fermer. `ui` doit être en disposition droite → gauche.
pub fn window_buttons(ui: &mut Ui, height: f32) {
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    let t = tok(ui);
    let button = |ui: &mut Ui, icon: Icon, label: &str, danger: bool| {
        let (rect, resp) = ui.allocate_exact_size(vec2(44.0, height), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
        let (fill, fg) = match (resp.hovered(), danger) {
            (true, true) => (theme::rgb(0xe81123), Color32::WHITE),
            (true, false) => (tint(t.ink, 0.08), t.ink_strong),
            _ => (Color32::TRANSPARENT, t.muted),
        };
        ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
        icons::paint(ui.painter(), rect.center(), icon, 15.0, fg);
        resp.clicked()
    };
    if button(ui, Icon::X, tr("Fermer la fenêtre"), true) {
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
    }
    if button(ui, if maximized { Icon::Copy } else { Icon::Square }, if maximized { tr("Restaurer") } else { tr("Agrandir") }, false) {
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }
    if button(ui, Icon::Minus, tr("Réduire"), false) {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

/// Le bord de fenêtre sous le pointeur (fenêtre sans bordure → redimensionnement maison).
pub fn edge_under_pointer(ctx: &egui::Context) -> Option<(ResizeDirection, CursorIcon)> {
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return None;
    }
    let pos = ctx.pointer_hover_pos()?;
    let r = ctx.viewport_rect();
    const BAND: f32 = 5.0;
    let (n, s) = (pos.y - r.top() < BAND, r.bottom() - pos.y < BAND);
    let (w, e) = (pos.x - r.left() < BAND, r.right() - pos.x < BAND);
    use ResizeDirection as D;
    Some(match (n, s, w, e) {
        (true, _, true, _) => (D::NorthWest, CursorIcon::ResizeNorthWest),
        (true, _, _, true) => (D::NorthEast, CursorIcon::ResizeNorthEast),
        (_, true, true, _) => (D::SouthWest, CursorIcon::ResizeSouthWest),
        (_, true, _, true) => (D::SouthEast, CursorIcon::ResizeSouthEast),
        (true, ..) => (D::North, CursorIcon::ResizeNorth),
        (_, true, ..) => (D::South, CursorIcon::ResizeSouth),
        (_, _, true, _) => (D::West, CursorIcon::ResizeWest),
        (_, _, _, true) => (D::East, CursorIcon::ResizeEast),
        _ => return None,
    })
}

/// À appeler en fin d'image : curseur de redimensionnement et passage de relais au système.
pub fn resize_from_edge(ctx: &egui::Context, edge: Option<(ResizeDirection, CursorIcon)>) {
    let Some((direction, cursor)) = edge else { return };
    ctx.set_cursor_icon(cursor);
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

/// Une police mono (chemins, commandes).
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

// ---- contrôles de la nouvelle interface --------------------------------------------------------

/// Des choix jointifs sur une ligne : un seul contour, des filets entre les cases, la case choisie
/// teintée d'accent. `mono` écrit les libellés en police à chasse fixe (formats, nombres).
pub fn seg<T: Copy + PartialEq>(ui: &mut Ui, name: &str, value: &mut T, options: &[(T, &str)], width: f32, mono: bool) -> bool {
    let t = tok(ui);
    let font = if mono { FontId::new(11.0, FontFamily::Monospace) } else { FontId::new(12.0, theme::medium()) };
    let (rect, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
    let p = ui.painter().clone();
    p.rect(rect, CornerRadius::same(7), t.bg, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
    let n = options.len().max(1);
    let w = (rect.width() - 2.0) / n as f32;
    let mut changed = false;
    for (i, (option, label)) in options.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(rect.left() + 1.0 + w * i as f32, rect.top() + 1.0), vec2(w, rect.height() - 2.0));
        let resp = ui.interact(cell, Id::unique((name, i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        let selected = *value == *option;
        resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, selected, *label));
        if resp.clicked() && !selected {
            *value = *option;
            changed = true;
        }
        let previous_selected = i > 0 && *value == options[i - 1].0;
        if i > 0 && !selected && !previous_selected {
            p.line_segment([pos2(cell.left().round() + 0.5, cell.top()), pos2(cell.left().round() + 0.5, cell.bottom())], Stroke::new(1.0, t.rule));
        }
        if selected {
            let radius = CornerRadius { nw: if i == 0 { 6 } else { 0 }, sw: if i == 0 { 6 } else { 0 }, ne: if i + 1 == n { 6 } else { 0 }, se: if i + 1 == n { 6 } else { 0 } };
            p.rect(cell, radius, tint(t.accent, 0.13), Stroke::new(1.0, tint(t.accent, 0.55)), StrokeKind::Inside);
        }
        let fg = if selected { t.accent_ink } else if resp.hovered() { t.ink_strong } else { t.muted };
        let galley = p.layout_no_wrap(label.to_string(), font.clone(), fg);
        p.galley(cell.center() - galley.size() / 2.0, galley, fg);
    }
    changed
}

/// Le curseur de compression : une ligne graduée tous les dix pour cent, pleine d'accent jusqu'au
/// bouton. Renvoie `true` quand la valeur change.
pub fn graduated_slider(ui: &mut Ui, name: &str, value: &mut u8, width: f32) -> bool {
    let t = tok(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click_and_drag());
    let current = *value;
    resp.widget_info(|| WidgetInfo::slider(true, f64::from(current), name));
    let track = Rect::from_min_max(pos2(rect.left() + 7.0, rect.center().y - 1.0), pos2(rect.right() - 7.0, rect.center().y + 1.0));
    let mut changed = false;
    if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
        let new = (((pos.x - track.left()) / track.width()).clamp(0.0, 1.0) * 100.0).round() as u8;
        changed = new != *value;
        *value = new;
    }
    let p = ui.painter();
    for i in 0..=10 {
        let x = (track.left() + track.width() * i as f32 / 10.0).round() + 0.5;
        p.line_segment([pos2(x, track.top() - 2.0), pos2(x, track.bottom() + 2.0)], Stroke::new(1.0, t.line_strong));
    }
    p.rect_filled(track, CornerRadius::same(1), theme::rgb(0x2a2a2a));
    let knob = pos2(track.left() + track.width() * f32::from(*value) / 100.0, rect.center().y);
    p.rect_filled(Rect::from_min_max(track.min, pos2(knob.x, track.bottom())), CornerRadius::same(1), t.accent);
    p.circle_filled(knob, 10.0, tint(t.accent, if resp.hovered() || resp.dragged() { 0.36 } else { 0.24 }));
    p.circle_filled(knob, 7.0, t.ink);
    changed
}

/// Le bouton qui lance le travail : haut, plein d'accent, avec son reflet et sa lueur. `danger`
/// en fait le bouton d'arrêt, à contour.
pub fn launch_button(ui: &mut Ui, icon: Icon, text: &str, width: f32, danger: bool) -> Response {
    let t = tok(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 48.0), Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, text));
    let hover = ui.ctx().animate_bool(resp.id.with("hover"), resp.hovered() && enabled);
    let rect = if resp.is_pointer_button_down_on() && enabled { rect.translate(vec2(0.0, 1.0)) } else { rect };
    let p = ui.painter();
    let fg = match (enabled, danger) {
        (false, _) => {
            p.rect(rect, CornerRadius::same(9), theme::rgb(0x1c1c1c), Stroke::new(1.0, t.line), StrokeKind::Inside);
            t.faint
        }
        (true, true) => {
            p.rect(rect, CornerRadius::same(9), tint(t.danger, 0.08 + 0.08 * hover), Stroke::new(1.0, tint(t.danger, 0.5)), StrokeKind::Inside);
            t.danger
        }
        (true, false) => {
            crate::draw::glow(p, rect, t.accent);
            p.rect_filled(rect, CornerRadius::same(9), mix(t.accent, t.accent_hover, hover));
            p.line_segment([pos2(rect.left() + 8.0, rect.top() + 1.0), pos2(rect.right() - 8.0, rect.top() + 1.0)], Stroke::new(1.0, Color32::from_white_alpha(110)));
            t.on_accent
        }
    };
    let galley = p.layout_no_wrap(text.to_owned(), FontId::new(14.5, theme::bold()), fg);
    let x = rect.center().x - (galley.size().x + 24.0) / 2.0;
    icons::paint(p, pos2(x + 8.0, rect.center().y), icon, 16.0, fg);
    p.galley(pos2(x + 24.0, rect.center().y - galley.size().y / 2.0), galley, fg);
    if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp }
}
