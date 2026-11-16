//! Le thème, repris de Rust Project : des design tokens (couleurs nommées par rôle) sur des gris
//! neutres, une couleur d'accent au choix, des polices, et les `Visuals` d'egui réécrits à partir
//! de tout ça.
//!
//! Aucun widget ne code une couleur en dur : tous lisent `Tokens::get(ctx)`.

use egui::epaint::Shadow;
use egui::epaint::text::VariationCoords;
use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Id, Stroke, TextStyle, Visuals};
use serde::{Deserialize, Serialize};

use crate::i18n::tr;

/// La couleur d'accent : l'ambre par défaut, cinq autres au choix.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Accent {
    Lime,
    #[default]
    Amber,
    Coral,
    Green,
    Blue,
    Violet,
}

/// Les cinq teintes dérivées d'un accent.
struct AccentSet {
    accent: Color32,
    hover: Color32,
    ink: Color32,
    soft: Color32,
    on: Color32,
}

impl Accent {
    pub const ALL: [Self; 6] = [Self::Amber, Self::Lime, Self::Coral, Self::Green, Self::Blue, Self::Violet];

    pub fn label(self) -> &'static str {
        match self {
            Self::Lime => tr("Citron"),
            Self::Amber => tr("Ambre"),
            Self::Coral => tr("Corail"),
            Self::Green => tr("Vert"),
            Self::Blue => tr("Bleu"),
            Self::Violet => tr("Violet"),
        }
    }

    /// La teinte principale, pour les pastilles du sélecteur.
    pub fn swatch(self) -> Color32 {
        self.set().accent
    }

    fn set(self) -> AccentSet {
        let s = |accent, hover, ink, soft, on| AccentSet { accent: hex(accent), hover: hex(hover), ink: hex(ink), soft: hex(soft), on: hex(on) };
        match self {
            Self::Lime => s(0xc4f042, 0xd4f76c, 0xc4f042, 0x232a12, 0x10130a),
            Self::Amber => s(0xf4a945, 0xffbb62, 0xf6b65c, 0x4a3215, 0x1d1407),
            Self::Coral => s(0xff7f66, 0xff9883, 0xff9a85, 0x4a2219, 0x1f0c07),
            Self::Green => s(0x6fcf8e, 0x86dba2, 0x8fdca6, 0x1f3a28, 0x07140c),
            Self::Blue => s(0x5b9dff, 0x7db1ff, 0x7db1ff, 0x1b2c47, 0x06101f),
            Self::Violet => s(0xb48cff, 0xc3a3ff, 0xc6a6ff, 0x33244f, 0x120a20),
        }
    }
}

// ---- tokens ------------------------------------------------------------------------------------

/// Les couleurs de l'interface, nommées par leur rôle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    /// Fond de la fenêtre.
    pub bg: Color32,
    /// Cartes et panneaux.
    pub surface: Color32,
    /// Survol, éléments en relief.
    pub raised: Color32,
    /// Champs de saisie.
    pub input: Color32,
    /// Filets fins.
    pub line: Color32,
    /// Contours des boutons secondaires et des champs.
    pub line_strong: Color32,
    pub ink: Color32,
    pub ink_strong: Color32,
    pub muted: Color32,
    pub faint: Color32,
    /// Accent : boutons principaux.
    pub accent: Color32,
    pub accent_hover: Color32,
    /// Accent pour du texte ou une icône sur le fond.
    pub accent_ink: Color32,
    /// Fond doux d'un élément sélectionné.
    pub accent_soft: Color32,
    /// Texte posé sur l'accent.
    pub on_accent: Color32,
    /// Pastille « Terminé ».
    pub good_bg: Color32,
    pub good_fg: Color32,
    /// Pastilles neutres.
    pub neutral_bg: Color32,
    pub neutral_fg: Color32,
    pub danger: Color32,
    /// Fond de la colonne des choix.
    pub panel: Color32,
    /// Fond des cadres et des listes.
    pub well: Color32,
    /// Filets entre deux cases jointives.
    pub rule: Color32,
    /// Croix de repère.
    pub mark: Color32,
    /// Points de la trame.
    pub dot: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    let [_, r, g, b] = rgb.to_be_bytes();
    Color32::from_rgb(r, g, b)
}

pub fn rgb(rgb: u32) -> Color32 {
    hex(rgb)
}

// La palette neutre de Rust Project et de sa landing, sans dominante brune.
const DARK: Tokens = Tokens {
    bg: hex(0x121212),
    surface: hex(0x181818),
    raised: hex(0x242424),
    input: hex(0x121212),
    line: hex(0x262626),
    line_strong: hex(0x333333),
    ink: hex(0xededed),
    ink_strong: hex(0xf5f5f5),
    muted: hex(0xa8a8a8),
    faint: hex(0x8a8a8a),
    accent: hex(0xf4a945),
    accent_hover: hex(0xffbb62),
    accent_ink: hex(0xf6b65c),
    accent_soft: hex(0x4a3215),
    on_accent: hex(0x1d1407),
    good_bg: hex(0x2c4526),
    good_fg: hex(0xb4d89a),
    neutral_bg: hex(0x333333),
    neutral_fg: hex(0xd3d3d3),
    danger: hex(0xff8a80),
    panel: hex(0x141414),
    well: hex(0x161616),
    rule: hex(0x2c2c2c),
    mark: hex(0x585858),
    dot: hex(0x2a2a2a),
};

fn accent_id() -> Id {
    Id::unique("video-compressor-accent")
}

impl Tokens {
    /// Les tokens affichés, avec l'accent choisi par l'utilisateur.
    pub fn get(ctx: &egui::Context) -> Self {
        let accent = ctx.data(|d| d.get_temp::<Accent>(accent_id())).unwrap_or_default();
        Self::of(accent)
    }

    fn of(accent: Accent) -> Self {
        let a = accent.set();
        Self { accent: a.accent, accent_hover: a.hover, accent_ink: a.ink, accent_soft: a.soft, on_accent: a.on, ..DARK }
    }

    pub fn hairline(&self) -> Stroke {
        Stroke::new(1.0, self.line)
    }
}

/// `c` avec une opacité `alpha` (0–1).
pub fn tint(c: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Mélange de `a` vers `b` (`t` de 0 à 1), pour les transitions.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn shadow() -> Shadow {
    Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(160) }
}

pub fn scrim() -> Color32 {
    Color32::from_black_alpha(150)
}

// ---- mesures -----------------------------------------------------------------------------------

pub const TOPBAR_H: f32 = 44.0;
pub const STATUS_H: f32 = 28.0;
pub const CONTROL_H: f32 = 32.0;
pub const PANEL_W: f32 = 500.0;
pub const R_CARD: u8 = 10;
pub const R_CTRL: u8 = 7;
pub const R_CHIP: u8 = 5;

// ---- Visuals -----------------------------------------------------------------------------------

fn visuals(t: &Tokens) -> Visuals {
    let mut v = Visuals::dark();
    v.panel_fill = t.bg;
    v.window_fill = t.surface;
    v.extreme_bg_color = t.input;
    v.text_edit_bg_color = Some(t.input);
    v.faint_bg_color = t.raised;
    v.code_bg_color = t.raised;
    v.window_stroke = Stroke::new(1.0, t.line_strong);
    v.window_corner_radius = CornerRadius::same(R_CARD);
    v.menu_corner_radius = CornerRadius::same(R_CTRL);
    v.window_shadow = shadow();
    v.popup_shadow = shadow();
    v.override_text_color = Some(t.ink);
    v.hyperlink_color = t.accent_ink;
    v.selection.bg_fill = tint(t.accent, 0.32);
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.text_cursor.stroke = Stroke::new(2.0, t.accent);
    v.warn_fg_color = t.accent_ink;
    v.error_fg_color = t.danger;
    v.handle_shape = egui::style::HandleShape::Circle;
    for (w, fill, stroke, fg) in [
        (&mut v.widgets.noninteractive, t.surface, t.line, t.muted),
        (&mut v.widgets.inactive, t.input, t.line_strong, t.ink),
        (&mut v.widgets.hovered, t.raised, t.faint, t.ink_strong),
        (&mut v.widgets.active, t.raised, t.accent, t.ink_strong),
        (&mut v.widgets.open, t.raised, t.line_strong, t.ink_strong),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0, stroke);
        w.corner_radius = CornerRadius::same(R_CTRL);
        w.fg_stroke = Stroke::new(1.0, fg);
        w.expansion = 0.0;
    }
    v
}

/// Installe l'apparence, l'accent, les espacements et la typographie.
pub fn apply(ctx: &egui::Context, accent: Accent) {
    ctx.data_mut(|d| d.insert_temp(accent_id(), accent));
    let v = visuals(&Tokens::of(accent));
    ctx.set_visuals_of(egui::Theme::Dark, v.clone());
    ctx.set_visuals_of(egui::Theme::Light, v);
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style.spacing.interact_size.y = CONTROL_H;
        style.spacing.slider_width = 200.0;
        style.spacing.icon_spacing = 8.0;
        // Libellés non sélectionnables : glisser sur un texte fait défiler au lieu de le surligner.
        style.interaction.selectable_labels = false;
        style.spacing.scroll = egui::style::ScrollStyle { dormant_handle_opacity: 0.35, ..egui::style::ScrollStyle::floating() };
        style.text_styles = [
            (TextStyle::Heading, FontId::new(22.0, bold())),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.0, semibold())),
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
        ]
        .into();
    });
}

// ---- polices -----------------------------------------------------------------------------------

pub fn medium() -> FontFamily {
    FontFamily::Name("medium".into())
}
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}
/// Les icônes Lucide seules (voir `icons.rs`).
pub fn icons() -> FontFamily {
    FontFamily::Name("icons".into())
}

type Face<'a> = (&'static str, &'static str, &'a [([u8; 4], f32)], FontFamily);

/// Segoe UI Variable (texte, quatre graisses tirées d'un seul fichier), Cascadia Mono (chemins)
/// et Lucide (icônes). Lucide est aussi ajoutée en secours derrière chaque police : une icône peut
/// ainsi se glisser dans n'importe quel texte.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let proportional = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let monospace = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();

    const BODY: &str = "SegUIVar.ttf";
    const MONO: &str = "CascadiaMono.ttf";
    let faces: [Face<'_>; 5] = [
        ("Body", BODY, &[(*b"wght", 400.0), (*b"opsz", 10.5)], FontFamily::Proportional),
        ("Body-Medium", BODY, &[(*b"wght", 500.0), (*b"opsz", 10.5)], medium()),
        ("Body-SemiBold", BODY, &[(*b"wght", 600.0), (*b"opsz", 10.5)], semibold()),
        ("Body-Bold", BODY, &[(*b"wght", 700.0), (*b"opsz", 28.0)], bold()),
        ("Mono", MONO, &[(*b"wght", 400.0)], FontFamily::Monospace),
    ];
    fonts.font_data.insert(
        "Lucide".to_owned(),
        FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES).tweak(FontTweak { y_offset_factor: 0.07, ..Default::default() }).into(),
    );
    for (name, file, coords, family) in faces {
        let fallback = if file == MONO { &monospace } else { &proportional };
        let mut stack = Vec::new();
        // Segoe UI et Cascadia (Windows), sinon San Francisco (Mac) : des polices variables,
        // réglées en graisse.
        let mac = if file == MONO { "/System/Library/Fonts/SFNSMono.ttf" } else { "/System/Library/Fonts/SFNS.ttf" };
        if let Some(bytes) = system_font(file).or_else(|| system_font(mac)) {
            let tweak = FontTweak { coords: VariationCoords::new(coords.iter().copied()), subpixel_binning: Some(false), ..Default::default() };
            fonts.font_data.insert(name.to_owned(), FontData::from_static(bytes).tweak(tweak).into());
            stack.push(name.to_owned());
        } else if let Some(bytes) = linux_font(name) {
            // Sans les polices de Windows (Linux) : DejaVu, qui a aussi les flèches et symboles.
            fonts.font_data.insert(name.to_owned(), FontData::from_static(bytes).into());
            stack.push(name.to_owned());
        }
        stack.extend(fallback.iter().cloned());
        stack.push("Lucide".to_owned());
        fonts.families.insert(family, stack);
    }
    fonts.families.insert(icons(), vec!["Lucide".to_owned()]);
    ctx.set_fonts(fonts);
}

/// La police DejaVu qui remplace une face de Segoe UI ou de Cascadia, quand elles manquent.
fn linux_font(face: &str) -> Option<&'static [u8]> {
    const DIR: &str = "/usr/share/fonts/truetype/dejavu/";
    let file = match face {
        "Body-SemiBold" | "Body-Bold" => "DejaVuSans-Bold.ttf",
        "Mono" => "DejaVuSansMono.ttf",
        _ => "DejaVuSans.ttf",
    };
    system_font(&format!("{DIR}{file}"))
}

/// Lit une police de `C:\Windows\Fonts` (ou un chemin absolu) une seule fois (octets partagés en
/// `&'static [u8]`).
fn system_font(file: &str) -> Option<&'static [u8]> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    type FontCache = HashMap<String, Option<&'static [u8]>>;
    static CACHE: OnceLock<Mutex<FontCache>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    *cache.entry(file.to_owned()).or_insert_with(|| {
        let path = if std::path::Path::new(file).is_absolute() {
            std::path::PathBuf::from(file)
        } else {
            let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
            std::path::Path::new(&dir).join("Fonts").join(file)
        };
        let bytes = std::fs::read(path).ok()?;
        Some(&*Box::leak(bytes.into_boxed_slice()))
    })
}
