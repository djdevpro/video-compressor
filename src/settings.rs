//! Les réglages d'encodage et les préférences, enregistrés dans `settings.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::i18n::{Lang, tr};
use crate::icons::Icon;
use crate::theme::Accent;

/// Le dossier des données : réglages et mises à jour téléchargées.
/// `VIDEO_COMPRESSOR_DATA` le déplace (tests, essais).
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("VIDEO_COMPRESSOR_DATA")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    if cfg!(windows)
        && let Ok(local) = std::env::var("LOCALAPPDATA")
    {
        return PathBuf::from(local).join("video-compressor");
    }
    let home = PathBuf::from(std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into()));
    if cfg!(target_os = "macos") {
        return home.join("Library").join("Application Support").join("video-compressor");
    }
    match std::env::var("XDG_DATA_HOME") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir).join("video-compressor"),
        _ => home.join(".local").join("share").join("video-compressor"),
    }
}

// ---- format de sortie --------------------------------------------------------------------------

/// Ce que produit un format : une vidéo, une image animée, ou le son seul.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Video,
    Animation,
    Audio,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Container {
    #[default]
    Mp4,
    Mkv,
    Mov,
    Webm,
    Gif,
    Webp,
    Mp3,
}

impl Container {
    pub const ALL: [Self; 7] = [Self::Mp4, Self::Mkv, Self::Mov, Self::Webm, Self::Gif, Self::Webp, Self::Mp3];

    pub fn label(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4",
            Self::Mkv => "MKV",
            Self::Mov => "MOV",
            Self::Webm => "WebM",
            Self::Gif => tr("GIF animé"),
            Self::Webp => tr("WebP animé"),
            Self::Mp3 => tr("MP3, le son seul"),
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
            Self::Mov => "mov",
            Self::Webm => "webm",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::Mp3 => "mp3",
        }
    }

    pub fn kind(self) -> Kind {
        match self {
            Self::Gif | Self::Webp => Kind::Animation,
            Self::Mp3 => Kind::Audio,
            _ => Kind::Video,
        }
    }

    /// Les codecs vidéo que ce conteneur accepte, le plus compatible d'abord.
    pub fn codecs(self) -> &'static [VideoCodec] {
        match self {
            Self::Mp4 | Self::Mkv | Self::Mov => &[VideoCodec::H264, VideoCodec::H265, VideoCodec::Av1],
            Self::Webm => &[VideoCodec::Vp9, VideoCodec::Av1],
            _ => &[],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VideoCodec {
    #[default]
    H264,
    H265,
    Av1,
    Vp9,
}

impl VideoCodec {
    pub fn label(self) -> &'static str {
        match self {
            Self::H264 => tr("H.264, lu partout"),
            Self::H265 => tr("H.265, plus léger"),
            Self::Av1 => tr("AV1, le plus léger"),
            Self::Vp9 => "VP9",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::H265 => "H.265",
            Self::Av1 => "AV1",
            Self::Vp9 => "VP9",
        }
    }

    /// Le nom de l'encodeur dans FFmpeg.
    pub fn encoder(self) -> &'static str {
        match self {
            Self::H264 => "libx264",
            Self::H265 => "libx265",
            Self::Av1 => "libsvtav1",
            Self::Vp9 => "libvpx-vp9",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Resolution {
    #[default]
    Source,
    P2160,
    P1440,
    P1080,
    P720,
    P480,
    P360,
    P240,
}

impl Resolution {
    pub const ALL: [Self; 8] = [Self::Source, Self::P2160, Self::P1440, Self::P1080, Self::P720, Self::P480, Self::P360, Self::P240];

    /// Le petit côté visé, en pixels (`None` : celui de la source).
    pub fn short_side(self) -> Option<u32> {
        match self {
            Self::Source => None,
            Self::P2160 => Some(2160),
            Self::P1440 => Some(1440),
            Self::P1080 => Some(1080),
            Self::P720 => Some(720),
            Self::P480 => Some(480),
            Self::P360 => Some(360),
            Self::P240 => Some(240),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Source => tr("Celle de l'original"),
            Self::P2160 => "2160p (4K)",
            Self::P1440 => "1440p",
            Self::P1080 => "1080p (Full HD)",
            Self::P720 => "720p (HD)",
            Self::P480 => "480p",
            Self::P360 => "360p",
            Self::P240 => "240p",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Fps {
    #[default]
    Source,
    F60,
    F50,
    F30,
    F25,
    F24,
    F15,
    F12,
    F10,
}

impl Fps {
    pub const ALL: [Self; 9] = [Self::Source, Self::F60, Self::F50, Self::F30, Self::F25, Self::F24, Self::F15, Self::F12, Self::F10];

    pub fn value(self) -> Option<u32> {
        match self {
            Self::Source => None,
            Self::F60 => Some(60),
            Self::F50 => Some(50),
            Self::F30 => Some(30),
            Self::F25 => Some(25),
            Self::F24 => Some(24),
            Self::F15 => Some(15),
            Self::F12 => Some(12),
            Self::F10 => Some(10),
        }
    }

    pub fn label(self) -> String {
        match self.value() {
            None => tr("Celles de l'original").to_owned(),
            Some(n) => crate::i18n::tf("{n} images par seconde", &[("n", &n)]),
        }
    }
}

/// Comment on fixe le poids du fichier : par la qualité, ou par une taille à ne pas dépasser.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Quality,
    TargetSize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Speed {
    Fast,
    #[default]
    Balanced,
    Slow,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Audio {
    K192,
    #[default]
    K128,
    K96,
    K64,
    None,
}

impl Audio {
    pub const ALL: [Self; 5] = [Self::K192, Self::K128, Self::K96, Self::K64, Self::None];

    pub fn kbps(self) -> Option<u32> {
        match self {
            Self::K192 => Some(192),
            Self::K128 => Some(128),
            Self::K96 => Some(96),
            Self::K64 => Some(64),
            Self::None => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::K192 => tr("Très bonne qualité (192 kb/s)"),
            Self::K128 => tr("Bonne qualité (128 kb/s)"),
            Self::K96 => tr("Légère (96 kb/s)"),
            Self::K64 => tr("Très légère (64 kb/s)"),
            Self::None => tr("Sans le son"),
        }
    }
}

/// Les réglages d'un encodage.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Encode {
    pub container: Container,
    pub codec: VideoCodec,
    pub resolution: Resolution,
    pub fps: Fps,
    pub mode: Mode,
    /// Le degré de compression, de 0 (légère, fichier plus lourd) à 100 (forte, fichier plus petit).
    pub compression: u8,
    /// La taille à ne pas dépasser, en mégaoctets (mode « taille cible »).
    pub target_mb: f32,
    pub speed: Speed,
    pub audio: Audio,
}

impl Default for Encode {
    fn default() -> Self {
        Self {
            container: Container::Mp4,
            codec: VideoCodec::H264,
            resolution: Resolution::Source,
            fps: Fps::Source,
            mode: Mode::Quality,
            compression: 40,
            target_mb: 25.0,
            speed: Speed::Balanced,
            audio: Audio::K128,
        }
    }
}

impl Encode {
    /// Remet les réglages d'accord entre eux après un changement de format.
    pub fn normalize(&mut self) {
        let codecs = self.container.codecs();
        if !codecs.is_empty() && !codecs.contains(&self.codec) {
            self.codec = codecs[0];
        }
        if self.container.kind() != Kind::Video {
            self.mode = Mode::Quality;
        }
        if self.container == Container::Mp3 && self.audio == Audio::None {
            self.audio = Audio::K128;
        }
        self.compression = self.compression.min(100);
        self.target_mb = self.target_mb.clamp(1.0, 100_000.0);
    }

    /// Le mot qui dit le degré de compression.
    pub fn compression_label(&self) -> &'static str {
        match self.compression {
            0..=19 => tr("Légère"),
            20..=49 => tr("Équilibrée"),
            50..=74 => tr("Forte"),
            _ => tr("Très forte"),
        }
    }

    /// Les réglages en une ligne : « MP4 · H.264 · 720p · 30 i/s ».
    pub fn summary(&self) -> String {
        let mut parts = vec![self.container.ext().to_uppercase()];
        if self.container.kind() == Kind::Video {
            parts.push(self.codec.short().to_owned());
        }
        if self.container.kind() != Kind::Audio {
            if let Some(side) = self.resolution.short_side() {
                parts.push(format!("{side}p"));
            }
            if let Some(fps) = self.fps.value() {
                parts.push(crate::i18n::tf("{n} i/s", &[("n", &fps)]));
            }
        }
        if self.mode == Mode::TargetSize {
            parts.push(crate::i18n::tf("{n} Mo au plus", &[("n", &crate::i18n::decimal(f64::from(self.target_mb), 0))]));
        }
        parts.join(" · ")
    }
}

/// Un préréglage : un nom, une icône, des réglages complets.
pub struct Preset {
    pub name: &'static str,
    pub hint: &'static str,
    pub icon: Icon,
    pub encode: Encode,
}

pub fn presets() -> Vec<Preset> {
    let base = Encode::default();
    vec![
        Preset { name: tr("Web 1080p"), hint: tr("MP4 H.264 en 1080p, lu partout"), icon: Icon::Globe, encode: Encode { resolution: Resolution::P1080, ..base } },
        Preset {
            name: tr("Léger 720p"),
            hint: tr("MP4 H.264 en 720p à 30 images par seconde"),
            icon: Icon::Feather,
            encode: Encode { resolution: Resolution::P720, fps: Fps::F30, compression: 55, audio: Audio::K96, ..base },
        },
        Preset {
            name: tr("Sous 10 Mo"),
            hint: tr("Pour les messageries qui limitent la taille des envois"),
            icon: Icon::Send,
            encode: Encode { resolution: Resolution::P720, fps: Fps::F30, mode: Mode::TargetSize, target_mb: 10.0, audio: Audio::K96, ..base },
        },
        Preset {
            name: tr("GIF"),
            hint: tr("GIF animé en 480p à 15 images par seconde"),
            icon: Icon::Images,
            encode: Encode { container: Container::Gif, resolution: Resolution::P480, fps: Fps::F15, audio: Audio::None, ..base },
        },
        Preset { name: tr("Son MP3"), hint: tr("Garde uniquement la piste audio"), icon: Icon::Music, encode: Encode { container: Container::Mp3, audio: Audio::K192, ..base } },
    ]
}

// ---- préférences -------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub encode: Encode,
    /// Le dossier des fichiers produits (vide : à côté de l'original).
    pub out_dir: String,
    /// Ajouté au nom de l'original (« vacances » devient « vacances-compresse »).
    pub suffix: String,
    pub update_auto: bool,
    pub update_beta: bool,
    /// La langue choisie (`None` : celle du système).
    pub language: Option<Lang>,
    pub accent: Accent,
    /// La proposition d'ajouter le clic droit a été écartée.
    pub menu_prompt_dismissed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            encode: Encode::default(),
            out_dir: String::new(),
            suffix: "-compresse".into(),
            update_auto: true,
            update_beta: false,
            language: None,
            accent: Accent::default(),
            menu_prompt_dismissed: false,
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        data_dir().join("settings.json")
    }

    /// Les réglages enregistrés, ou ceux par défaut (premier lancement, fichier illisible).
    pub fn load() -> Self {
        let mut s: Self = std::fs::read_to_string(Self::path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        s.encode.normalize();
        s
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changer_de_format_garde_des_reglages_coherents() {
        let mut e = Encode { container: Container::Webm, codec: VideoCodec::H264, ..Default::default() };
        e.normalize();
        assert_eq!(e.codec, VideoCodec::Vp9, "le WebM n'accepte pas le H.264");
        let mut e = Encode { container: Container::Gif, mode: Mode::TargetSize, ..Default::default() };
        e.normalize();
        assert_eq!(e.mode, Mode::Quality, "pas de taille cible pour un GIF");
        let mut e = Encode { container: Container::Mp3, audio: Audio::None, ..Default::default() };
        e.normalize();
        assert_eq!(e.audio, Audio::K128, "un MP3 sans son n'a pas de sens");
    }

    #[test]
    fn reglages_relus_meme_incomplets() {
        let s: Settings = serde_json::from_str(r#"{"encode":{"container":"Webm"},"suffix":"-mini"}"#).unwrap();
        assert_eq!((s.encode.container, s.suffix.as_str(), s.update_auto), (Container::Webm, "-mini", true));
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn prereglages_coherents() {
        for p in presets() {
            let mut e = p.encode;
            e.normalize();
            assert_eq!(e, p.encode, "le préréglage « {} » se contredit", p.name);
        }
        assert_eq!(Encode { resolution: Resolution::P720, fps: Fps::F30, ..Default::default() }.summary(), "MP4 · H.264 · 720p · 30 i/s");
    }
}
