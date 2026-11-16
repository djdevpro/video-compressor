//! Les motifs de l'interface, peints à la main : la trame de points, le halo en diagonale, les
//! hachures, la pellicule, les croix de repère, et le petit dessin de chaque choix.
//!
//! Ils sont posés à des endroits précis (voir `app.rs`), jamais partout : un motif répété sur
//! chaque bloc redevient un décor.

use egui::epaint::{Mesh, Shadow};
use egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};

use crate::theme::{self, Tokens, mix, tint};

/// Une trame de petits points, pleine en haut de `rect` puis fondue vers le bas.
pub fn dot_grid(p: &Painter, rect: Rect, color: Color32) {
    const STEP: f32 = 14.0;
    let mut y = rect.top() + 6.0;
    while y < rect.bottom() {
        let fade = ((rect.bottom() - y) / (rect.height() * 0.55)).clamp(0.0, 1.0);
        let c = color.gamma_multiply(fade);
        let mut x = rect.left() + 6.0;
        while x < rect.right() {
            p.rect_filled(Rect::from_center_size(pos2(x, y), Vec2::splat(1.5)), CornerRadius::ZERO, c);
            x += STEP;
        }
        y += STEP;
    }
}

/// Une trame de points serrée et uniforme (l'intérieur d'une petite illustration).
fn small_dots(p: &Painter, rect: Rect, color: Color32, step: f32) {
    let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
    let mut y = rect.top() + step / 2.0;
    while y < rect.bottom() {
        let mut x = rect.left() + step / 2.0;
        while x < rect.right() {
            p.rect_filled(Rect::from_center_size(pos2(x, y), Vec2::splat(1.2)), CornerRadius::ZERO, color);
            x += step;
        }
        y += step;
    }
}

/// Une bande de lumière douce, centrée sur `center` et couchée le long de `along`. Elle s'éteint
/// vers ses quatre bords : aucun arrêt net ne trahit sa forme.
pub fn halo(p: &Painter, center: Pos2, along: Vec2, half_length: f32, half_width: f32, strength: u8) {
    const N: usize = 12;
    let along = along.normalized();
    let across = vec2(-along.y, along.x);
    let mut mesh = Mesh::default();
    for i in 0..=N {
        let u = i as f32 / N as f32 * 2.0 - 1.0;
        for j in 0..=N {
            let v = j as f32 / N as f32 * 2.0 - 1.0;
            // Une cloche dans les deux sens.
            let bell = |x: f32| (1.0 - x * x).max(0.0).powi(2);
            let alpha = (f32::from(strength) * bell(u) * bell(v)).round() as u8;
            mesh.colored_vertex(center + along * (u * half_length) + across * (v * half_width), Color32::from_white_alpha(alpha));
        }
    }
    let at = |i: usize, j: usize| (i * (N + 1) + j) as u32;
    for i in 0..N {
        for j in 0..N {
            mesh.add_triangle(at(i, j), at(i + 1, j), at(i, j + 1));
            mesh.add_triangle(at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
        }
    }
    p.add(Shape::mesh(mesh));
}

/// Les points du contour d'un rectangle arrondi, assez serrés pour un dégradé le long du trait.
fn outline(rect: Rect, radius: f32) -> Vec<Pos2> {
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [
        (pos2(rect.right() - r, rect.top() + r), -90.0f32),
        (pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (pos2(rect.left() + r, rect.bottom() - r), 90.0),
        (pos2(rect.left() + r, rect.top() + r), 180.0),
    ];
    let mut points: Vec<Pos2> = Vec::new();
    for (center, start) in corners {
        let first = center + r * Vec2::angled(start.to_radians());
        // Le bord droit qui mène à ce coin, découpé en petits pas.
        if let Some(&last) = points.last() {
            let steps = ((first - last).length() / 14.0).ceil().max(1.0) as usize;
            points.extend((1..steps).map(|k| last + (first - last) * (k as f32 / steps as f32)));
        }
        points.extend((0..=8).map(|k| center + r * Vec2::angled((start + 90.0 * k as f32 / 8.0).to_radians())));
    }
    // Le bord du haut, qui referme le contour.
    if let (Some(&last), Some(&first)) = (points.last(), points.first()) {
        let steps = ((first - last).length() / 14.0).ceil().max(1.0) as usize;
        points.extend((1..steps).map(|k| last + (first - last) * (k as f32 / steps as f32)));
    }
    points
}

/// Un contour d'un pixel dont la lumière change le long de la diagonale : clair en haut à gauche,
/// sombre au milieu, un peu plus clair en bas à droite.
pub fn gradient_frame(p: &Painter, rect: Rect, radius: f32, [start, middle, end]: [Color32; 3]) {
    let points = outline(rect.shrink(0.5), radius);
    let color_at = |pos: Pos2| {
        let t = ((pos.x - rect.left()) / rect.width() + (pos.y - rect.top()) / rect.height()) / 2.0;
        if t < 0.42 { mix(start, middle, t / 0.42) } else { mix(middle, end, (t - 0.42) / 0.58) }
    };
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        p.line_segment([a, b], Stroke::new(1.0, color_at(a + (b - a) * 0.5)));
    }
}

/// Des hachures fines en diagonale, à l'intérieur de `rect`.
pub fn hatch(p: &Painter, rect: Rect, color: Color32, spacing: f32) {
    let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
    let step = spacing * std::f32::consts::SQRT_2;
    let mut x = rect.left() - rect.height();
    while x < rect.right() {
        p.line_segment([pos2(x, rect.bottom()), pos2(x + rect.height(), rect.top())], Stroke::new(1.0, color));
        x += step;
    }
}

/// Un repère en croix de 13 points, centré sur un angle ou un croisement de filets.
pub fn cross(p: &Painter, center: Pos2, color: Color32) {
    let c = pos2(center.x.round() + 0.5, center.y.round() + 0.5);
    p.line_segment([c - vec2(6.5, 0.0), c + vec2(6.5, 0.0)], Stroke::new(1.0, color));
    p.line_segment([c - vec2(0.0, 6.5), c + vec2(0.0, 6.5)], Stroke::new(1.0, color));
}

/// Les perforations d'une pellicule, le long d'une bande horizontale.
pub fn perforations(p: &Painter, strip: Rect, color: Color32) {
    let p = p.with_clip_rect(strip.intersect(p.clip_rect()));
    let mut x = strip.left() + 6.0;
    while x < strip.right() {
        p.rect_filled(Rect::from_min_size(pos2(x, strip.center().y - 2.5), vec2(8.0, 5.0)), CornerRadius::same(1), color);
        x += 20.0;
    }
}

/// Un filet gradué, comme le bord d'une règle : un trait, et une graduation tous les `step` points.
pub fn ruler(p: &Painter, rect: Rect, line: Color32, tick: Color32, step: f32) {
    let mut x = rect.left();
    while x < rect.right() {
        p.line_segment([pos2(x.round() + 0.5, rect.top()), pos2(x.round() + 0.5, rect.bottom())], Stroke::new(1.0, tick));
        x += step;
    }
    p.line_segment([pos2(rect.left(), rect.top() + 0.5), pos2(rect.right(), rect.top() + 0.5)], Stroke::new(1.0, line));
}

/// Le damier qui signale la transparence.
pub fn checker(p: &Painter, rect: Rect, radius: u8, dark: Color32, light: Color32, cell: f32) {
    p.rect_filled(rect, CornerRadius::same(radius), dark);
    let p = p.with_clip_rect(rect.shrink(1.0).intersect(p.clip_rect()));
    let (cols, rows) = ((rect.width() / cell).ceil() as usize, (rect.height() / cell).ceil() as usize);
    for row in 0..rows {
        for col in (row % 2..cols).step_by(2) {
            p.rect_filled(Rect::from_min_size(rect.min + vec2(col as f32 * cell, row as f32 * cell), Vec2::splat(cell)), CornerRadius::ZERO, light);
        }
    }
}

/// La lueur sous le bouton principal.
pub fn glow(p: &Painter, rect: Rect, color: Color32) {
    let shadow = Shadow { offset: [0, 9], blur: 26, spread: 0, color: tint(color, 0.32) };
    p.add(shadow.as_shape(rect.shrink2(vec2(22.0, 8.0)), CornerRadius::same(9)));
}

fn mono(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Monospace)
}

// ---- le dessin de chaque choix -----------------------------------------------------------------

/// Web 1080p : une fenêtre de navigateur, sa page tramée, la vidéo qui joue.
pub fn web(p: &Painter, origin: Pos2, t: &Tokens, selected: bool) {
    let rect = Rect::from_min_size(origin, vec2(92.0, 54.0));
    let lead = if selected { t.accent } else { t.muted };
    p.rect(rect, CornerRadius::same(5), t.bg, Stroke::new(1.0, if selected { mix(theme::rgb(0x3a3a3a), t.accent, 0.6) } else { theme::rgb(0x4a4a4a) }), StrokeKind::Inside);
    let bar = Rect::from_min_size(rect.min + vec2(1.0, 1.0), vec2(rect.width() - 2.0, 9.0));
    p.rect_filled(bar, CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 }, theme::rgb(0x1a1a1a));
    p.line_segment([pos2(rect.left() + 1.0, rect.top() + 10.5), pos2(rect.right() - 1.0, rect.top() + 10.5)], Stroke::new(1.0, theme::rgb(0x3a3a3a)));
    for x in [7.0, 14.0] {
        p.circle_filled(rect.min + vec2(x, 5.5), 2.0, theme::rgb(0x5a5a5a));
    }
    p.rect_filled(Rect::from_min_max(rect.min + vec2(22.0, 3.5), pos2(rect.right() - 6.0, rect.top() + 7.5)), CornerRadius::same(2), theme::rgb(0x2e2e2e));
    small_dots(p, Rect::from_min_max(pos2(rect.left() + 1.0, rect.top() + 11.0), rect.max - vec2(1.0, 1.0)), t.dot, 7.0);
    let c = rect.min + vec2(47.0, 32.0);
    if selected {
        p.circle_filled(c, 11.0, tint(t.accent, 0.10));
        p.circle_filled(c, 7.0, tint(t.accent, 0.12));
    }
    p.add(Shape::convex_polygon(vec![c + vec2(-6.0, -8.0), c + vec2(7.0, 0.0), c + vec2(-6.0, 8.0)], lead, Stroke::NONE));
}

/// Léger 720p : deux poids sur une règle graduée, l'ancien en hachures, le nouveau bien plus petit.
pub fn light(p: &Painter, origin: Pos2, t: &Tokens, selected: bool) {
    let base = origin.y + 48.0;
    ruler(p, Rect::from_min_size(pos2(origin.x, base + 1.0), vec2(150.0, 5.0)), theme::rgb(0x404040), theme::rgb(0x383838), 8.0);
    let old = Rect::from_min_max(pos2(origin.x + 10.0, base - 46.0), pos2(origin.x + 34.0, base));
    hatch(p, old, theme::rgb(0x2c2c2c), 6.0);
    p.rect_stroke(old, CornerRadius::ZERO, Stroke::new(1.0, theme::rgb(0x3c3c3c)), StrokeKind::Inside);
    p.rect_filled(Rect::from_min_max(pos2(origin.x + 46.0, base - 15.0), pos2(origin.x + 70.0, base)), CornerRadius::ZERO, if selected { t.accent } else { t.muted });
    p.extend(Shape::dashed_line(&[pos2(origin.x, base - 15.5), pos2(origin.x + 150.0, base - 15.5)], Stroke::new(1.0, theme::rgb(0x3c3c3c)), 3.0, 3.0));
    p.text(pos2(origin.x + 82.0, base - 20.0), Align2::LEFT_BOTTOM, "720p", mono(10.5), t.muted);
}

/// Sous 10 Mo : une jauge graduée, sa limite, et la zone interdite hachurée au-delà.
pub fn limit(p: &Painter, origin: Pos2, t: &Tokens, selected: bool) {
    let mut x = origin.x;
    while x < origin.x + 170.0 {
        p.line_segment([pos2(x.round() + 0.5, origin.y + 19.0), pos2(x.round() + 0.5, origin.y + 24.0)], Stroke::new(1.0, theme::rgb(0x3a3a3a)));
        x += 10.0;
    }
    let gauge = Rect::from_min_size(origin + vec2(0.0, 30.0), vec2(170.0, 10.0));
    p.rect(gauge, CornerRadius::ZERO, t.bg, Stroke::new(1.0, theme::rgb(0x4a4a4a)), StrokeKind::Inside);
    p.rect_filled(Rect::from_min_size(gauge.min + vec2(1.0, 1.0), vec2(92.0, 8.0)), CornerRadius::ZERO, if selected { t.accent } else { t.muted });
    hatch(p, Rect::from_min_max(pos2(origin.x + 111.0, gauge.top() + 1.0), gauge.max - vec2(1.0, 1.0)), theme::rgb(0x3c3c3c), 6.0);
    p.line_segment([pos2(origin.x + 110.5, origin.y + 13.0), pos2(origin.x + 110.5, origin.y + 46.0)], Stroke::new(1.0, t.ink));
    p.text(pos2(origin.x + 110.0, origin.y), Align2::CENTER_TOP, format!("10 {}", crate::i18n::tr("Mo")), mono(10.5), t.ink);
}

/// GIF animé : des images empilées, la première sur un damier de transparence.
pub fn gif(p: &Painter, origin: Pos2, t: &Tokens, selected: bool) {
    let size = vec2(62.0, 38.0);
    p.rect(Rect::from_min_size(origin + vec2(30.0, 0.0), size), CornerRadius::same(4), theme::rgb(0x161616), Stroke::new(1.0, theme::rgb(0x3a3a3a)), StrokeKind::Inside);
    p.rect(Rect::from_min_size(origin + vec2(15.0, 8.0), size), CornerRadius::same(4), theme::rgb(0x1a1a1a), Stroke::new(1.0, theme::rgb(0x4c4c4c)), StrokeKind::Inside);
    let front = Rect::from_min_size(origin + vec2(0.0, 16.0), size);
    checker(p, front, 4, theme::rgb(0x1b1b1b), theme::rgb(0x272727), 8.0);
    p.rect_stroke(front, CornerRadius::same(4), Stroke::new(1.0, if selected { t.accent } else { theme::rgb(0x707070) }), StrokeKind::Inside);
    let label = Rect::from_center_size(front.center(), vec2(30.0, 15.0));
    p.rect(label, CornerRadius::same(3), t.bg, Stroke::new(1.0, theme::rgb(0x4c4c4c)), StrokeKind::Inside);
    p.text(label.center(), Align2::CENTER_CENTER, "GIF", mono(10.5), t.ink);
}

/// Le son seul : une petite onde en bâtons, centrée verticalement sur `left_center`.
pub fn sound(p: &Painter, left_center: Pos2, t: &Tokens, selected: bool) {
    for (i, h) in [6.0f32, 12.0, 18.0, 9.0, 14.0, 5.0, 10.0].into_iter().enumerate() {
        let strong = i == 2 || i == 4;
        let color = if strong && selected { t.accent } else if strong { t.ink } else { t.faint };
        p.rect_filled(Rect::from_center_size(left_center + vec2(1.0 + i as f32 * 4.0, 0.0), vec2(2.0, h)), CornerRadius::ZERO, color);
    }
}
