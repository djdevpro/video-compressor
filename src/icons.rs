//! Les icônes Lucide, dessinées comme des glyphes de police : nettes à toutes les tailles et
//! recolorées gratuitement.

use egui::{Align2, Color32, FontId, Painter, Pos2, Response, Sense, Ui, vec2};
pub use lucide_icons::Icon;

use crate::theme;

/// Le caractère d'une icône, à glisser dans un texte (la police Lucide est en secours partout).
pub fn ch(icon: Icon) -> char {
    char::from(icon)
}

pub fn paint(p: &Painter, center: Pos2, icon: Icon, size: f32, color: Color32) {
    p.text(center, Align2::CENTER_CENTER, ch(icon), FontId::new(size, theme::icons()), color);
}

/// Une icône seule, dans un carré de `size` points.
pub fn show(ui: &mut Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    paint(ui.painter(), rect.center(), icon, size, color);
    resp
}
