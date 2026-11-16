//! Le moteur vidéo : FFmpeg, lié dans l'exécutable.
//!
//! Il n'y a aucun programme externe. Les bibliothèques de FFmpeg (libavformat, libavcodec,
//! libavfilter, libswscale, libswresample, et les encodeurs x264, x265, SVT-AV1, libvpx, Opus,
//! LAME, libwebp) sont compilées en statique par vcpkg puis liées dans le binaire, et pilotées ici
//! par la crate `ffmpeg-next`.
//!
//! Un encodage suit toujours le même chemin : démultiplexer, décoder, passer par un graphe de
//! filtres (rotation, cadence, échelle, format de pixel), encoder, multiplexer. `plan` décide de
//! tout ce qui se décide sans ouvrir le fichier (c'est ce que les tests vérifient d'abord),
//! `encode` l'exécute.

use std::ffi::CStr;
use std::path::Path;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use av::format::Pixel;
use av::{Dictionary, Packet, Rational, codec, decoder, encoder, ffi, filter, format, frame, media};
use ffmpeg_next as av;

use crate::i18n::{tf, tr};
use crate::settings::{Container, Encode, Kind, Mode, Speed, VideoCodec};
use crate::webp::{self, WebpInfo};

/// Les fichiers proposés au clic droit et dans le sélecteur (FFmpeg en lit bien d'autres).
pub const VIDEO_EXTS: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "mts", "m2ts", "3gp", "ogv", "vob", "gif", "webp"];

/// Prépare FFmpeg une fois pour tout le programme. Il ne parle que pour ses erreurs.
pub fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = av::init();
        av::log::set_level(av::log::Level::Error);
    });
}

/// La version du FFmpeg lié dans l'exécutable (« 9.0.2 »).
pub fn version() -> String {
    // SAFETY : `av_version_info` renvoie une chaîne statique terminée par un zéro.
    let text = unsafe { CStr::from_ptr(ffi::av_version_info()) }.to_string_lossy().into_owned();
    let v = text.trim_start_matches('n');
    let end = v.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(v.len());
    if end == 0 { text } else { v[..end].trim_end_matches('.').to_owned() }
}

// ---- mesures d'un fichier ----------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaInfo {
    /// En secondes (0 si inconnue).
    pub duration: f64,
    /// Les dimensions telles qu'on les voit, rotation appliquée.
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub vcodec: String,
    pub has_video: bool,
    pub has_audio: bool,
    /// Le poids du fichier, en octets.
    pub size: u64,
    /// Le quart de tour à appliquer à l'image, dans le sens horaire (0, 90, 180 ou 270) : une
    /// vidéo de téléphone tenue debout est enregistrée couchée, avec cette indication.
    pub rotation: u16,
    /// Un WebP animé : il est décodé par `webp.rs`, FFmpeg ne sait pas le lire.
    pub webp: Option<WebpInfo>,
}

fn rate(r: Rational) -> f64 {
    if r.numerator() > 0 && r.denominator() > 0 { f64::from(r) } else { 0.0 }
}

/// La piste vidéo à traiter : la première qui n'est pas une pochette d'album.
pub(crate) fn video_index(ictx: &format::context::Input) -> Option<usize> {
    ictx.streams().find(|s| s.parameters().medium() == media::Type::Video && !s.disposition().contains(format::stream::Disposition::ATTACHED_PIC)).map(|s| s.index())
}

fn audio_index(ictx: &format::context::Input) -> Option<usize> {
    ictx.streams().best(media::Type::Audio).map(|s| s.index())
}

/// Le quart de tour demandé par la matrice d'affichage de la piste (comme le fait `ffmpeg`).
fn rotation(stream: &format::stream::Stream) -> u16 {
    let par = stream.parameters();
    // SAFETY : `par` garde les paramètres vivants ; la matrice fait 9 entiers de 32 bits.
    let theta = unsafe {
        let p = par.as_ptr();
        let sd = ffi::av_packet_side_data_get((*p).coded_side_data, (*p).nb_coded_side_data, ffi::AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX);
        if sd.is_null() || (*sd).size < 36 {
            return 0;
        }
        -ffi::av_display_rotation_get((*sd).data as *const i32)
    };
    if !theta.is_finite() {
        return 0;
    }
    ((theta / 90.0).round() as i64).rem_euclid(4) as u16 * 90
}

/// Les mesures d'un fichier. Une erreur si ce n'est ni une vidéo ni un son lisible.
pub fn probe(path: &Path) -> Result<MediaInfo, String> {
    let size = std::fs::metadata(path).map(|m| m.len()).map_err(|e| tf("Fichier illisible ({0})", &[("0", &e)]))?;
    if let Some(w) = webp::inspect(path) {
        return Ok(MediaInfo { duration: w.duration(), width: w.width, height: w.height, fps: w.fps(), vcodec: "webp".into(), has_video: true, has_audio: false, size, rotation: 0, webp: Some(w) });
    }
    init();
    let ictx = format::input(path).map_err(|e| tf("Ce fichier n'est pas reconnu ({0})", &[("0", &e)]))?;
    let mut info = MediaInfo { size, has_audio: audio_index(&ictx).is_some(), ..Default::default() };
    if ictx.duration() > 0 {
        info.duration = ictx.duration() as f64 / f64::from(ffi::AV_TIME_BASE);
    }
    if let Some(stream) = video_index(&ictx).and_then(|i| ictx.stream(i)) {
        let par = stream.parameters();
        info.has_video = true;
        info.vcodec = par.id().name().to_owned();
        // SAFETY : lecture de deux entiers des paramètres de la piste, vivants tant que `par`.
        (info.width, info.height) = unsafe { ((*par.as_ptr()).width.max(0) as u32, (*par.as_ptr()).height.max(0) as u32) };
        info.fps = match rate(stream.avg_frame_rate()) {
            f if f > 0.0 => f,
            _ => rate(stream.rate()),
        };
        info.rotation = rotation(&stream);
        if info.rotation % 180 == 90 {
            std::mem::swap(&mut info.width, &mut info.height);
        }
        if info.duration <= 0.0 && stream.duration() > 0 {
            info.duration = stream.duration() as f64 * f64::from(stream.time_base());
        }
    }
    if info.has_video || info.has_audio { Ok(info) } else { Err(tr("Ce fichier ne contient ni vidéo ni son.").to_owned()) }
}

// ---- vignette ----------------------------------------------------------------------------------

/// La plus grande taille qui tient dans `max` en gardant les proportions.
pub(crate) fn fit(w: u32, h: u32, max: (u32, u32)) -> (u32, u32) {
    let k = (f64::from(max.0) / f64::from(w.max(1))).min(f64::from(max.1) / f64::from(h.max(1))).min(1.0);
    (((f64::from(w) * k).round() as u32).max(1), ((f64::from(h) * k).round() as u32).max(1))
}

/// Réduit une image RGBA en moyennant les pixels de chaque case.
fn downscale(w: u32, h: u32, rgba: &[u8], max: (u32, u32)) -> (u32, u32, Vec<u8>) {
    let (tw, th) = fit(w, h, max);
    if (tw, th) == (w, h) {
        return (w, h, rgba.to_vec());
    }
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    for y in 0..th {
        let (y0, y1) = (y * h / th, ((y + 1) * h / th).max(y * h / th + 1).min(h));
        for x in 0..tw {
            let (x0, x1) = (x * w / tw, ((x + 1) * w / tw).max(x * w / tw + 1).min(w));
            let mut sum = [0u32; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = ((sy * w + sx) * 4) as usize;
                    for c in 0..4 {
                        sum[c] += u32::from(rgba[i + c]);
                    }
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            out.extend(sum.map(|s| (s / n.max(1)) as u8));
        }
    }
    (tw, th, out)
}

/// Tourne une image RGBA d'un quart de tour dans le sens horaire.
pub(crate) fn quarter_turn(w: u32, h: u32, rgba: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut out = vec![0u8; rgba.len()];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = (h - 1 - y, x);
            let (from, to) = (((y * w + x) * 4) as usize, ((ny * h + nx) * 4) as usize);
            out[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
        }
    }
    (h, w, out)
}

/// Les octets d'une image décodée, ligne par ligne (FFmpeg garde une marge au bout de chacune).
pub(crate) fn packed_rows(frame: &frame::Video, bytes_per_pixel: usize) -> Vec<u8> {
    let (w, h, stride) = (frame.width() as usize * bytes_per_pixel, frame.height() as usize, frame.stride(0));
    let data = frame.data(0);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        out.extend_from_slice(&data[y * stride..y * stride + w]);
    }
    out
}

/// Une image du fichier, en RGBA, qui tient dans `max`.
pub fn thumbnail(path: &Path, info: &MediaInfo, max: (u32, u32)) -> Option<(u32, u32, Vec<u8>)> {
    if info.webp.is_some() {
        let (w, h, rgba) = webp::first_frame(path)?;
        return Some(downscale(w, h, &rgba, max));
    }
    if !info.has_video || info.width == 0 || info.height == 0 {
        return None;
    }
    init();
    let mut ictx = format::input(path).ok()?;
    let index = video_index(&ictx)?;
    let mut decoder = open_decoder(&ictx.stream(index)?).ok()?.video().ok()?;
    // Un peu après le début : la première image est souvent noire.
    let at = ((info.duration * 0.1).min(1.0) * f64::from(ffi::AV_TIME_BASE)) as i64;
    if at > 0 {
        let _ = ictx.seek(at, ..at);
    }
    let mut decoded = frame::Video::empty();
    let mut found = false;
    for (stream, packet) in ictx.packets().take(600) {
        if stream.index() == index && decoder.send_packet(&packet).is_ok() && decoder.receive_frame(&mut decoded).is_ok() {
            found = true;
            break;
        }
    }
    if !found {
        let _ = decoder.send_eof();
        found = decoder.receive_frame(&mut decoded).is_ok();
    }
    if !found || decoded.width() == 0 {
        return None;
    }
    // L'image est encore couchée si la vidéo est tournée : on la réduit, puis on la redresse.
    let turned = info.rotation % 180 == 90;
    let (tw, th) = if turned {
        let (a, b) = fit(decoded.height(), decoded.width(), max);
        (b, a)
    } else {
        fit(decoded.width(), decoded.height(), max)
    };
    let mut scaler = av::software::scaling::Context::get(decoded.format(), decoded.width(), decoded.height(), Pixel::RGBA, tw, th, av::software::scaling::Flags::BILINEAR).ok()?;
    let mut small = frame::Video::empty();
    scaler.run(&decoded, &mut small).ok()?;
    let mut image = (tw, th, packed_rows(&small, 4));
    for _ in 0..info.rotation / 90 {
        image = quarter_turn(image.0, image.1, &image.2);
    }
    Some(image)
}

// ---- ce qu'il faut faire, décidé sans ouvrir le fichier ----------------------------------------

/// Un WebP animé en entrée : la cadence à laquelle ses images sont rejouées.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feed {
    pub fps: f64,
    pub keep_alpha: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VideoPlan {
    /// Le nom de l'encodeur de FFmpeg.
    pub encoder: &'static str,
    /// La chaîne de filtres, jusqu'au format de pixel que l'encodeur attend.
    pub filters: String,
    pub pixel: Pixel,
    /// Les options de l'encodeur (qualité, vitesse, débit).
    pub options: Vec<(&'static str, String)>,
    /// L'étiquette du codec dans le conteneur, quand elle doit être forcée.
    pub tag: Option<&'static [u8; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioPlan {
    pub encoder: &'static str,
    pub kbps: u32,
}

/// Tout ce qu'un encodage va faire.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub video: Option<VideoPlan>,
    pub audio: Option<AudioPlan>,
    /// Les options du conteneur.
    pub muxer: Vec<(&'static str, String)>,
    pub feed: Option<Feed>,
}

/// Le facteur de qualité (CRF) du codec pour ce degré de compression.
pub fn crf(enc: &Encode) -> u32 {
    let c = f32::from(enc.compression.min(100));
    let v = match enc.codec {
        VideoCodec::H264 => 18.0 + 0.14 * c,
        VideoCodec::H265 => 20.0 + 0.14 * c,
        VideoCodec::Av1 => 24.0 + 0.26 * c,
        VideoCodec::Vp9 => 24.0 + 0.24 * c,
    };
    v.round() as u32
}

/// Le son est-il gardé (et y en a-t-il) ?
fn audio_kbps(enc: &Encode, info: &MediaInfo) -> Option<u32> {
    enc.audio.kbps().filter(|_| info.has_audio)
}

/// Le débit vidéo, en kb/s, qui fait tenir le fichier dans la taille cible. `None` hors de ce mode
/// ou si la durée est inconnue.
pub fn video_kbps(enc: &Encode, info: &MediaInfo) -> Option<u32> {
    if enc.mode != Mode::TargetSize || enc.container.kind() != Kind::Video || info.duration < 0.5 {
        return None;
    }
    // 1 Mo = 1 000 000 octets, moins 3 % pour le conteneur : le fichier reste sous la limite.
    let total = f64::from(enc.target_mb) * 8000.0 * 0.97 / info.duration;
    Some((total - f64::from(audio_kbps(enc, info).unwrap_or(0))).max(40.0).round() as u32)
}

/// La mise à l'échelle : le petit côté ramené à la résolution choisie, jamais agrandi. Les codecs
/// vidéo veulent des dimensions paires (`even`).
fn scale_filter(enc: &Encode, info: &MediaInfo, even: bool) -> Option<String> {
    let (w, h) = (info.width, info.height);
    let portrait = h > w;
    let short = if portrait { w } else { h };
    match enc.resolution.short_side() {
        Some(target) if short == 0 || target < short => Some(if portrait { format!("scale={target}:-2:flags=lanczos") } else { format!("scale=-2:{target}:flags=lanczos") }),
        _ if even && (w == 0 || w % 2 == 1 || h % 2 == 1) => Some("scale=trunc(iw/2)*2:trunc(ih/2)*2".to_owned()),
        _ => None,
    }
}

/// La cadence : réduite si on en demande moins que la source, jamais augmentée.
fn fps_filter(enc: &Encode, source_fps: f64) -> Option<String> {
    let n = enc.fps.value()?;
    (source_fps <= 0.0 || f64::from(n) < source_fps - 0.01).then(|| format!("fps={n}"))
}

/// Décide de l'encodage de ce fichier avec ces réglages. `bitrate` remplace le débit calculé
/// (deuxième essai, quand le premier a dépassé la taille cible).
pub fn plan(enc: &Encode, info: &MediaInfo, bitrate: Option<u32>) -> Result<Plan, String> {
    let kind = enc.container.kind();
    if kind != Kind::Audio && !info.has_video {
        return Err(tr("Ce fichier n'a pas d'image. Choisis le format MP3 pour garder son son.").to_owned());
    }
    if kind == Kind::Audio && !info.has_audio {
        return Err(tr("Ce fichier n'a pas de son à extraire.").to_owned());
    }
    if kind == Kind::Audio {
        return Ok(Plan { video: None, audio: Some(AudioPlan { encoder: "libmp3lame", kbps: enc.audio.kbps().unwrap_or(128) }), muxer: Vec::new(), feed: None });
    }

    // Un WebP animé est rejoué à sa cadence, ou à celle demandée si elle est plus basse.
    let feed = info.webp.map(|w| Feed {
        fps: match enc.fps.value() {
            Some(n) if f64::from(n) < w.fps() => f64::from(n),
            _ => w.fps(),
        },
        keep_alpha: kind == Kind::Animation,
    });
    let mut filters: Vec<String> = Vec::new();
    match info.rotation {
        90 => filters.push("transpose=clock".into()),
        180 => filters.push("hflip,vflip".into()),
        270 => filters.push("transpose=cclock".into()),
        _ => {}
    }
    if feed.is_none() {
        filters.extend(fps_filter(enc, info.fps));
    }
    let threads = ("threads", "auto".to_owned());

    if enc.container == Container::Gif {
        filters.extend(scale_filter(enc, info, false));
        let colors = match enc.compression {
            0..=24 => 256,
            25..=49 => 128,
            50..=74 => 64,
            _ => 32,
        };
        filters.push(format!("split[a][b];[a]palettegen=max_colors={colors}:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle"));
        let video = VideoPlan { encoder: "gif", filters: filters.join(","), pixel: Pixel::PAL8, options: Vec::new(), tag: None };
        return Ok(Plan { video: Some(video), audio: None, muxer: Vec::new(), feed });
    }
    if enc.container == Container::Webp {
        filters.extend(scale_filter(enc, info, false));
        // La transparence d'un WebP animé est gardée de bout en bout.
        let pixel = if info.webp.is_some_and(|w| w.has_alpha) { Pixel::YUVA420P } else { Pixel::YUV420P };
        filters.push(format!("format={}", if pixel == Pixel::YUVA420P { "yuva420p" } else { "yuv420p" }));
        let quality = (90.0 - 0.7 * f32::from(enc.compression)).round() as u32;
        let effort = match enc.speed {
            Speed::Fast => "2",
            Speed::Balanced => "4",
            Speed::Slow => "6",
        };
        let options = vec![("lossless", "0".to_owned()), ("quality", quality.to_string()), ("compression_level", effort.to_owned())];
        let video = VideoPlan { encoder: "libwebp_anim", filters: filters.join(","), pixel, options, tag: None };
        return Ok(Plan { video: Some(video), audio: None, muxer: vec![("loop", "0".to_owned())], feed });
    }

    let codec = enc.codec;
    filters.extend(scale_filter(enc, info, true));
    filters.push("format=yuv420p".into());
    let preset = match (codec, enc.speed) {
        (VideoCodec::H264 | VideoCodec::H265, Speed::Fast) => "veryfast",
        (VideoCodec::H264 | VideoCodec::H265, Speed::Balanced) => "medium",
        (VideoCodec::H264 | VideoCodec::H265, Speed::Slow) => "slow",
        (VideoCodec::Av1, Speed::Fast) => "10",
        (VideoCodec::Av1, Speed::Balanced) => "7",
        (VideoCodec::Av1, Speed::Slow) => "4",
        (VideoCodec::Vp9, Speed::Fast) => "5",
        (VideoCodec::Vp9, Speed::Balanced) => "3",
        (VideoCodec::Vp9, Speed::Slow) => "1",
    };
    let mut options = vec![threads];
    match codec {
        VideoCodec::Vp9 => options.extend([("deadline", "good".to_owned()), ("cpu-used", preset.to_owned()), ("row-mt", "1".to_owned())]),
        _ => options.push(("preset", preset.to_owned())),
    }
    // x265 écrit ses propres messages si on ne le fait pas taire.
    let mut x265 = vec!["log-level=error".to_owned()];
    match bitrate.or_else(|| video_kbps(enc, info)) {
        // Une taille à tenir : un débit moyen, borné pour que les passages difficiles ne débordent pas.
        Some(k) => {
            options.push(("b", format!("{k}k")));
            match codec {
                VideoCodec::H264 => options.extend([("maxrate", format!("{}k", k * 3 / 2)), ("bufsize", format!("{}k", k * 3))]),
                VideoCodec::H265 => x265.extend([format!("vbv-maxrate={}", k * 3 / 2), format!("vbv-bufsize={}", k * 3)]),
                _ => {}
            }
        }
        None => {
            options.push(("crf", crf(enc).to_string()));
            if codec == VideoCodec::Vp9 {
                options.push(("b", "0".to_owned()));
            }
        }
    }
    if codec == VideoCodec::H265 {
        options.push(("x265-params", x265.join(":")));
    }
    // Sans cette étiquette, QuickTime et l'iPhone refusent le H.265.
    let tag = (codec == VideoCodec::H265 && matches!(enc.container, Container::Mp4 | Container::Mov)).then_some(b"hvc1");
    let audio = audio_kbps(enc, info).map(|kbps| AudioPlan { encoder: if enc.container == Container::Webm { "libopus" } else { "aac" }, kbps });
    let muxer = if matches!(enc.container, Container::Mp4 | Container::Mov) { vec![("movflags", "+faststart".to_owned())] } else { Vec::new() };
    Ok(Plan { video: Some(VideoPlan { encoder: codec.encoder(), filters: filters.join(","), pixel: Pixel::YUV420P, options, tag }), audio, muxer, feed })
}

// ---- l'encodage --------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum EncodeError {
    Cancelled,
    Failed(String),
}

impl From<av::Error> for EncodeError {
    fn from(e: av::Error) -> Self {
        Self::Failed(e.to_string())
    }
}

fn dictionary(options: &[(&'static str, String)]) -> Dictionary<'static> {
    let mut dict = Dictionary::new();
    for (key, value) in options {
        dict.set(key, value);
    }
    dict
}

fn find_encoder(name: &str) -> Result<av::Codec, EncodeError> {
    encoder::find_by_name(name).ok_or_else(|| EncodeError::Failed(tf("L'encodeur {0} n'est pas dans ce FFmpeg.", &[("0", &name)])))
}

/// Ouvre le décodeur d'une piste, sur tous les coeurs.
pub(crate) fn open_decoder(stream: &format::stream::Stream) -> Result<decoder::Opened, av::Error> {
    let context = codec::context::Context::from_parameters(stream.parameters())?;
    let codec = decoder::find(context.id()).ok_or(av::Error::DecoderNotFound)?;
    let mut decoder = context.decoder();
    decoder.set_packet_time_base(stream.time_base());
    decoder.open_as_with(codec, dictionary(&[("threads", "auto".to_owned())]))
}

/// La sortie vidéo : le graphe de filtres, l'encodeur, et la piste du fichier produit.
struct VideoOut {
    graph: filter::Graph,
    encoder: encoder::Video,
    index: usize,
    encoder_tb: Rational,
    stream_tb: Rational,
    /// La durée d'une image, en unités de l'encodeur (0 si la cadence est inconnue).
    frame_ticks: i64,
}

impl VideoOut {
    /// Monte le graphe pour des images de cette forme, puis l'encodeur sur ce qui en sort.
    #[allow(clippy::too_many_arguments)]
    fn new(octx: &mut format::context::Output, plan: &VideoPlan, width: u32, height: u32, pixel: Pixel, tb: Rational, aspect: Rational, frame_rate: Option<Rational>, global_header: bool) -> Result<Self, EncodeError> {
        let aspect = if aspect.numerator() > 0 && aspect.denominator() > 0 { aspect } else { Rational(1, 1) };
        let mut args = format!("video_size={width}x{height}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}", ffi::AVPixelFormat::from(pixel) as i32, tb.numerator(), tb.denominator(), aspect.numerator(), aspect.denominator());
        if let Some(r) = frame_rate.filter(|r| r.numerator() > 0 && r.denominator() > 0) {
            args.push_str(&format!(":frame_rate={}/{}", r.numerator(), r.denominator()));
        }
        let mut graph = filter::Graph::new();
        graph.add(&filter::find("buffer").ok_or(av::Error::FilterNotFound)?, "in", &args)?;
        graph.add(&filter::find("buffersink").ok_or(av::Error::FilterNotFound)?, "out", "")?;
        graph.output("in", 0)?.input("out", 0)?.parse(&plan.filters)?;
        graph.validate()?;
        // Ce qui sort du graphe : la taille, la base de temps et la cadence de l'encodeur.
        let (w, h, out_tb, out_rate, out_aspect) = {
            let sink = graph.get("out").ok_or(av::Error::FilterNotFound)?;
            // SAFETY : le graphe est configuré, ces fonctions ne font que lire sa sortie.
            unsafe {
                let p = sink.as_ptr();
                (ffi::av_buffersink_get_w(p), ffi::av_buffersink_get_h(p), Rational::from(ffi::av_buffersink_get_time_base(p)), Rational::from(ffi::av_buffersink_get_frame_rate(p)), Rational::from(ffi::av_buffersink_get_sample_aspect_ratio(p)))
            }
        };
        let codec = find_encoder(plan.encoder)?;
        let mut stream = octx.add_stream(codec)?;
        let index = stream.index();
        let mut video = codec::context::Context::new_with_codec(codec).encoder().video()?;
        video.set_width(w.max(1) as u32);
        video.set_height(h.max(1) as u32);
        video.set_format(plan.pixel);
        video.set_time_base(out_tb);
        if out_rate.numerator() > 0 && out_rate.denominator() > 0 {
            video.set_frame_rate(Some(out_rate));
        }
        if out_aspect.numerator() > 0 && out_aspect.denominator() > 0 {
            video.set_aspect_ratio(out_aspect);
        }
        if global_header {
            video.set_flags(codec::Flags::GLOBAL_HEADER);
        }
        // L'encodeur reporte la durée de chaque image sur ses paquets : la dernière image du
        // fichier dure alors ce qu'elle doit (`ffmpeg-next` ne connaît pas ce drapeau).
        // SAFETY : le contexte de l'encodeur est à nous, pas encore ouvert.
        unsafe { (*video.as_mut_ptr()).flags |= ffi::AV_CODEC_FLAG_FRAME_DURATION as i32 };
        let encoder = video.open_with(dictionary(&plan.options))?;
        stream.set_parameters(&encoder);
        stream.set_time_base(out_tb);
        if out_rate.numerator() > 0 && out_rate.denominator() > 0 {
            stream.set_avg_frame_rate(out_rate);
        }
        if let Some(tag) = plan.tag {
            // SAFETY : la piste vient d'être créée, ses paramètres sont à elle.
            unsafe { (*stream.parameters().as_mut_ptr()).codec_tag = u32::from_le_bytes(*tag) };
        }
        let frame_ticks = if rate(out_rate) > 0.0 && rate(out_tb) > 0.0 { (1.0 / rate(out_rate) / rate(out_tb)).round() as i64 } else { 0 };
        Ok(Self { graph, encoder, index, encoder_tb: out_tb, stream_tb: out_tb, frame_ticks })
    }

    /// Une image entre dans le graphe ; ce qui en sort est encodé et écrit.
    fn push(&mut self, image: &frame::Video, octx: &mut format::context::Output) -> Result<(), EncodeError> {
        self.graph.get("in").ok_or(av::Error::FilterNotFound)?.source().add(image)?;
        self.pull(octx)
    }

    fn pull(&mut self, octx: &mut format::context::Output) -> Result<(), EncodeError> {
        let mut filtered = frame::Video::empty();
        while self.graph.get("out").ok_or(av::Error::FilterNotFound)?.sink().frame(&mut filtered).is_ok() {
            // L'encodeur choisit lui-même ses images clés.
            filtered.set_kind(av::picture::Type::None);
            // Une image qui sort du graphe sans durée prend celle de la cadence : sans cela la
            // dernière image du fichier ne durerait rien.
            // SAFETY : l'image vient d'être remplie par le graphe, elle est à nous.
            unsafe {
                if (*filtered.as_ptr()).duration <= 0 && self.frame_ticks > 0 {
                    (*filtered.as_mut_ptr()).duration = self.frame_ticks;
                }
            }
            self.encoder.send_frame(&filtered)?;
            self.write(octx)?;
        }
        Ok(())
    }

    fn write(&mut self, octx: &mut format::context::Output) -> Result<(), EncodeError> {
        let mut packet = Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(self.index);
            packet.rescale_ts(self.encoder_tb, self.stream_tb);
            packet.write_interleaved(octx)?;
        }
        Ok(())
    }

    /// Plus d'images : on vide le graphe, puis l'encodeur.
    fn finish(&mut self, octx: &mut format::context::Output) -> Result<(), EncodeError> {
        self.graph.get("in").ok_or(av::Error::FilterNotFound)?.source().flush()?;
        self.pull(octx)?;
        self.encoder.send_eof()?;
        self.write(octx)
    }
}

/// Le son : décodé, ramené au format de l'encodeur par un graphe de filtres, puis encodé.
struct AudioOut {
    decoder: decoder::Audio,
    graph: filter::Graph,
    encoder: encoder::Audio,
    index: usize,
    encoder_tb: Rational,
    stream_tb: Rational,
}

impl AudioOut {
    fn new(octx: &mut format::context::Output, plan: &AudioPlan, stream: &format::stream::Stream, global_header: bool) -> Result<Self, EncodeError> {
        let decoder = open_decoder(stream)?.audio()?;
        let codec = find_encoder(plan.encoder)?;
        // Opus ne connaît que 48 kHz ; les autres gardent 44,1 kHz quand c'est celui de la source.
        let rate: i32 = if plan.encoder != "libopus" && decoder.rate() == 44_100 { 44_100 } else { 48_000 };
        let layout = if decoder.channels() >= 2 { av::ChannelLayout::STEREO } else { av::ChannelLayout::MONO };
        let sample = format::Sample::F32(if plan.encoder == "libopus" { format::sample::Type::Packed } else { format::sample::Type::Planar });
        let mut out = octx.add_stream(codec)?;
        let index = out.index();
        let mut audio = codec::context::Context::new_with_codec(codec).encoder().audio()?;
        audio.set_rate(rate);
        audio.set_channel_layout(layout);
        audio.set_format(sample);
        audio.set_bit_rate(plan.kbps as usize * 1000);
        audio.set_time_base(Rational(1, rate));
        if global_header {
            audio.set_flags(codec::Flags::GLOBAL_HEADER);
        }
        let encoder = audio.open_as(codec)?;
        out.set_parameters(&encoder);
        out.set_time_base(Rational(1, rate));

        // Certaines pistes ne disent pas la disposition de leurs canaux : on prend celle par défaut.
        let source_layout = if decoder.channel_layout().bits() == 0 { av::ChannelLayout::default(i32::from(decoder.channels())) } else { decoder.channel_layout() };
        let tb = stream.time_base();
        let args = format!("time_base={}/{}:sample_rate={}:sample_fmt={}:channel_layout=0x{:x}", tb.numerator(), tb.denominator(), decoder.rate(), decoder.format().name(), source_layout.bits());
        let mut graph = filter::Graph::new();
        graph.add(&filter::find("abuffer").ok_or(av::Error::FilterNotFound)?, "in", &args)?;
        graph.add(&filter::find("abuffersink").ok_or(av::Error::FilterNotFound)?, "out", "")?;
        // Le format que l'encodeur attend est imposé par un filtre `aformat`, pas par les options de
        // la sortie du graphe : avec FFmpeg 9, les poser depuis `ffmpeg-next` fait planter le programme.
        let spec = format!("aformat=sample_fmts={}:sample_rates={rate}:channel_layouts={}", sample.name(), if layout.channels() >= 2 { "stereo" } else { "mono" });
        graph.output("in", 0)?.input("out", 0)?.parse(&spec)?;
        graph.validate()?;
        // La plupart des encodeurs veulent des blocs d'une taille fixe.
        if !codec.capabilities().contains(codec::capabilities::Capabilities::VARIABLE_FRAME_SIZE) {
            graph.get("out").ok_or(av::Error::FilterNotFound)?.sink().set_frame_size(encoder.frame_size());
        }
        Ok(Self { decoder, graph, encoder, index, encoder_tb: Rational(1, rate), stream_tb: Rational(1, rate) })
    }

    /// Un paquet du fichier d'origine. Renvoie l'instant atteint, en unités de la piste.
    fn decode(&mut self, packet: &Packet, octx: &mut format::context::Output) -> Result<Option<i64>, EncodeError> {
        // Un paquet abîmé ne doit pas arrêter tout le fichier.
        if self.decoder.send_packet(packet).is_err() {
            return Ok(None);
        }
        self.drain(octx)
    }

    fn drain(&mut self, octx: &mut format::context::Output) -> Result<Option<i64>, EncodeError> {
        let mut decoded = frame::Audio::empty();
        let mut reached = None;
        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let ts = decoded.timestamp();
            decoded.set_pts(ts);
            reached = ts.or(reached);
            self.graph.get("in").ok_or(av::Error::FilterNotFound)?.source().add(&decoded)?;
            self.pull(octx)?;
        }
        Ok(reached)
    }

    fn pull(&mut self, octx: &mut format::context::Output) -> Result<(), EncodeError> {
        let mut filtered = frame::Audio::empty();
        while self.graph.get("out").ok_or(av::Error::FilterNotFound)?.sink().frame(&mut filtered).is_ok() {
            self.encoder.send_frame(&filtered)?;
            self.write(octx)?;
        }
        Ok(())
    }

    fn write(&mut self, octx: &mut format::context::Output) -> Result<(), EncodeError> {
        let mut packet = Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(self.index);
            packet.rescale_ts(self.encoder_tb, self.stream_tb);
            packet.write_interleaved(octx)?;
        }
        Ok(())
    }

    fn finish(&mut self, octx: &mut format::context::Output) -> Result<(), EncodeError> {
        let _ = self.decoder.send_eof();
        self.drain(octx)?;
        self.graph.get("in").ok_or(av::Error::FilterNotFound)?.source().flush()?;
        self.pull(octx)?;
        self.encoder.send_eof()?;
        self.write(octx)
    }
}

/// La piste vidéo du fichier d'origine, et de quoi dater les images qui n'ont pas d'instant.
struct VideoIn {
    decoder: decoder::Video,
    next: i64,
    step: i64,
}

impl VideoIn {
    fn decode(&mut self, packet: &Packet, out: &mut VideoOut, octx: &mut format::context::Output) -> Result<Option<i64>, EncodeError> {
        if self.decoder.send_packet(packet).is_err() {
            return Ok(None);
        }
        self.drain(out, octx)
    }

    fn drain(&mut self, out: &mut VideoOut, octx: &mut format::context::Output) -> Result<Option<i64>, EncodeError> {
        let mut decoded = frame::Video::empty();
        let mut reached = None;
        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let ts = decoded.timestamp().unwrap_or(self.next);
            self.next = ts + self.step;
            decoded.set_pts(Some(ts));
            reached = Some(ts);
            out.push(&decoded, octx)?;
        }
        Ok(reached)
    }
}

fn write_header(octx: &mut format::context::Output, plan: &Plan) -> Result<(), EncodeError> {
    octx.write_header_with(dictionary(&plan.muxer))?;
    Ok(())
}

/// Encode `input` vers `output` selon `plan`.
fn transcode(plan: &Plan, info: &MediaInfo, input: &Path, output: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(f32, f32)) -> Result<(), EncodeError> {
    init();
    let mut octx = format::output(output)?;
    let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);
    let started = Instant::now();
    let mut report = |seconds: f64| {
        if info.duration > 0.0 {
            progress((seconds / info.duration).clamp(0.0, 1.0) as f32, (seconds / started.elapsed().as_secs_f64().max(0.001)) as f32);
        }
    };

    // Un WebP animé : ses images viennent de `webp.rs`, à cadence fixe.
    if let (Some(feed), Some(w), Some(video_plan)) = (plan.feed, info.webp, plan.video.as_ref()) {
        let rgba = webp::pipe_format(&w, feed.keep_alpha) == "rgba";
        let (pixel, bytes) = if rgba { (Pixel::RGBA, 4) } else { (Pixel::RGB24, 3) };
        let fps = feed.fps.round().max(1.0) as i32;
        let mut video = VideoOut::new(&mut octx, video_plan, w.width, w.height, pixel, Rational(1, fps), Rational(1, 1), Some(Rational(fps, 1)), global_header)?;
        write_header(&mut octx, plan)?;
        video.stream_tb = octx.stream(video.index).map(|s| s.time_base()).unwrap_or(video.stream_tb);
        let row = w.width as usize * bytes;
        let mut count = 0i64;
        let fed = webp::frames(input, feed.fps, feed.keep_alpha, cancel, &mut |pixels| {
            let mut image = frame::Video::new(pixel, w.width, w.height);
            let stride = image.stride(0);
            let data = image.data_mut(0);
            for y in 0..w.height as usize {
                data[y * stride..y * stride + row].copy_from_slice(&pixels[y * row..(y + 1) * row]);
            }
            image.set_pts(Some(count));
            // Chaque image dure un pas : sans cela, la dernière n'aurait pas de durée dans le fichier.
            // SAFETY : l'image vient d'être allouée, elle est à nous.
            unsafe { (*image.as_mut_ptr()).duration = 1 };
            count += 1;
            video.push(&image, &mut octx).map_err(|e| match e {
                EncodeError::Failed(why) => why,
                EncodeError::Cancelled => String::new(),
            })?;
            report(count as f64 / f64::from(fps));
            Ok(())
        });
        if cancel.load(Ordering::Relaxed) {
            return Err(EncodeError::Cancelled);
        }
        fed.map_err(|e| EncodeError::Failed(tf("Le WebP animé n'a pas pu être lu en entier ({0})", &[("0", &e)])))?;
        video.finish(&mut octx)?;
        octx.write_trailer()?;
        return Ok(());
    }

    let mut ictx = format::input(input)?;
    let video_at = plan.video.as_ref().and_then(|_| video_index(&ictx));
    let audio_at = plan.audio.as_ref().and_then(|_| audio_index(&ictx));
    let mut video = None;
    let mut start = 0i64;
    let mut video_tb = Rational(1, 1);
    if let (Some(video_plan), Some(stream)) = (plan.video.as_ref(), video_at.and_then(|i| ictx.stream(i))) {
        let decoder = open_decoder(&stream)?.video()?;
        video_tb = stream.time_base();
        let fps = match rate(stream.avg_frame_rate()) {
            f if f > 0.0 => f,
            _ => rate(stream.rate()),
        };
        let frame_rate = if rate(stream.avg_frame_rate()) > 0.0 { Some(stream.avg_frame_rate()) } else { None };
        let step = if fps > 0.0 { (1.0 / fps / f64::from(video_tb)).round().max(1.0) as i64 } else { 1 };
        if stream.start_time() != ffi::AV_NOPTS_VALUE {
            start = stream.start_time();
        }
        let out = VideoOut::new(&mut octx, video_plan, decoder.width(), decoder.height(), decoder.format(), video_tb, decoder.aspect_ratio(), frame_rate, global_header)?;
        video = Some((VideoIn { decoder, next: start, step }, out));
    }
    let mut audio = None;
    let mut audio_tb = Rational(1, 1);
    if let (Some(audio_plan), Some(stream)) = (plan.audio.as_ref(), audio_at.and_then(|i| ictx.stream(i))) {
        audio_tb = stream.time_base();
        if video.is_none() && stream.start_time() != ffi::AV_NOPTS_VALUE {
            start = stream.start_time();
        }
        audio = Some(AudioOut::new(&mut octx, audio_plan, &stream, global_header)?);
    }
    if video.is_none() && audio.is_none() {
        return Err(EncodeError::Failed(tr("Ce fichier ne contient ni vidéo ni son.").to_owned()));
    }
    octx.set_metadata(ictx.metadata().to_owned());
    write_header(&mut octx, plan)?;
    // Le conteneur peut imposer sa propre base de temps : on la relit après l'en-tête.
    if let Some((_, out)) = &mut video {
        out.stream_tb = octx.stream(out.index).map(|s| s.time_base()).unwrap_or(out.stream_tb);
    }
    if let Some(out) = &mut audio {
        out.stream_tb = octx.stream(out.index).map(|s| s.time_base()).unwrap_or(out.stream_tb);
    }

    for (stream, packet) in ictx.packets() {
        if cancel.load(Ordering::Relaxed) {
            return Err(EncodeError::Cancelled);
        }
        let index = stream.index();
        if Some(index) == video_at {
            if let Some((source, out)) = &mut video
                && let Some(ts) = source.decode(&packet, out, &mut octx)?
            {
                report((ts - start) as f64 * f64::from(video_tb));
            }
        } else if Some(index) == audio_at
            && let Some(out) = &mut audio
            && let Some(ts) = out.decode(&packet, &mut octx)?
            && video.is_none()
        {
            report((ts - start) as f64 * f64::from(audio_tb));
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(EncodeError::Cancelled);
    }
    if let Some((source, out)) = &mut video {
        let _ = source.decoder.send_eof();
        source.drain(out, &mut octx)?;
        out.finish(&mut octx)?;
    }
    if let Some(out) = &mut audio {
        out.finish(&mut octx)?;
    }
    octx.write_trailer()?;
    Ok(())
}

/// Encode `input` vers `output` avec ces réglages. `progress` reçoit la part faite (0 à 1) et la
/// vitesse (1.0 = temps réel) ; `cancel` arrête le travail dès qu'il passe à vrai.
pub fn encode(enc: &Encode, info: &MediaInfo, input: &Path, output: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(f32, f32)) -> Result<(), EncodeError> {
    let limit = (enc.mode == Mode::TargetSize && enc.container.kind() == Kind::Video).then(|| (f64::from(enc.target_mb) * 1_000_000.0) as u64);
    let mut bitrate = video_kbps(enc, info);
    // Une taille cible se vise en une passe. Si le fichier dépasse quand même (vidéo courte ou
    // très agitée), on recommence avec un débit corrigé : la limite demandée est tenue.
    for attempt in 0..3 {
        let plan = plan(enc, info, bitrate).map_err(EncodeError::Failed)?;
        transcode(&plan, info, input, output, cancel, progress)?;
        let size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
        match (limit, bitrate) {
            (Some(limit), Some(k)) if size > limit && attempt < 2 => {
                bitrate = Some(((f64::from(k) * limit as f64 / size as f64 * 0.94) as u32).max(30));
            }
            _ => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Audio, Fps, Resolution};

    fn hd() -> MediaInfo {
        MediaInfo { duration: 60.0, width: 1920, height: 1080, fps: 30.0, vcodec: "h264".into(), has_video: true, has_audio: true, size: 50_000_000, rotation: 0, webp: None }
    }

    fn video(enc: &Encode, info: &MediaInfo) -> VideoPlan {
        plan(enc, info, None).unwrap().video.unwrap()
    }

    fn option(v: &VideoPlan, key: &str) -> Option<String> {
        v.options.iter().find(|(k, _)| *k == key).map(|(_, value)| value.clone())
    }

    #[test]
    fn reglages_par_defaut() {
        let p = plan(&Encode::default(), &hd(), None).unwrap();
        let v = p.video.clone().unwrap();
        assert_eq!((v.encoder, v.filters.as_str(), v.pixel), ("libx264", "format=yuv420p", Pixel::YUV420P), "ni échelle ni cadence quand on garde l'original");
        assert_eq!((option(&v, "preset").as_deref(), option(&v, "crf").as_deref(), option(&v, "threads").as_deref()), (Some("medium"), Some("24"), Some("auto")));
        assert_eq!(p.audio, Some(AudioPlan { encoder: "aac", kbps: 128 }));
        assert_eq!(p.muxer, [("movflags", "+faststart".to_owned())]);
        assert_eq!((v.tag, p.feed), (None, None));
    }

    #[test]
    fn echelle_cadence_et_rotation() {
        let enc = Encode { resolution: Resolution::P720, fps: Fps::F24, ..Default::default() };
        assert_eq!(video(&enc, &hd()).filters, "fps=24,scale=-2:720:flags=lanczos,format=yuv420p");
        let portrait = MediaInfo { width: 1080, height: 1920, rotation: 90, ..hd() };
        assert_eq!(video(&enc, &portrait).filters, "transpose=clock,fps=24,scale=720:-2:flags=lanczos,format=yuv420p", "tenue debout : redressée, et c'est la largeur qu'on réduit");
        let small = MediaInfo { width: 640, height: 360, fps: 15.0, ..hd() };
        assert_eq!(video(&enc, &small).filters, "format=yuv420p", "jamais d'agrandissement ni d'images ajoutées");
        let odd = MediaInfo { width: 641, height: 360, ..hd() };
        assert_eq!(video(&Encode::default(), &odd).filters, "scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p", "dimensions impaires arrondies");
        assert!(video(&Encode::default(), &MediaInfo { rotation: 180, ..hd() }).filters.starts_with("hflip,vflip,"));
        assert!(video(&Encode::default(), &MediaInfo { rotation: 270, ..hd() }).filters.starts_with("transpose=cclock,"));
    }

    #[test]
    fn codecs_et_son() {
        let h265 = plan(&Encode { codec: VideoCodec::H265, audio: Audio::None, ..Default::default() }, &hd(), None).unwrap();
        let v = h265.video.unwrap();
        assert_eq!((v.encoder, v.tag, option(&v, "x265-params").as_deref(), h265.audio), ("libx265", Some(b"hvc1"), Some("log-level=error"), None));
        let webm = plan(&Encode { container: Container::Webm, codec: VideoCodec::Vp9, ..Default::default() }, &hd(), None).unwrap();
        let v = webm.video.unwrap();
        assert_eq!((v.encoder, option(&v, "crf").as_deref(), option(&v, "b").as_deref(), option(&v, "cpu-used").as_deref()), ("libvpx-vp9", Some("34"), Some("0"), Some("3")));
        assert_eq!((webm.audio, webm.muxer.len()), (Some(AudioPlan { encoder: "libopus", kbps: 128 }), 0));
        let av1 = video(&Encode { codec: VideoCodec::Av1, speed: Speed::Fast, ..Default::default() }, &hd());
        assert_eq!((av1.encoder, option(&av1, "preset").as_deref()), ("libsvtav1", Some("10")));
        assert_eq!(plan(&Encode::default(), &MediaInfo { has_audio: false, ..hd() }, None).unwrap().audio, None, "pas de son dans la source");
    }

    #[test]
    fn taille_cible() {
        let enc = Encode { mode: Mode::TargetSize, target_mb: 10.0, ..Default::default() };
        // 10 Mo sur 60 s : 1293 kb/s en tout, moins 128 de son.
        assert_eq!(video_kbps(&enc, &hd()), Some(1165));
        let v = video(&enc, &hd());
        assert_eq!((option(&v, "b").as_deref(), option(&v, "maxrate").as_deref(), option(&v, "bufsize").as_deref(), option(&v, "crf")), (Some("1165k"), Some("1747k"), Some("3495k"), None));
        let h265 = video(&Encode { codec: VideoCodec::H265, ..enc }, &hd());
        assert_eq!(option(&h265, "x265-params").as_deref(), Some("log-level=error:vbv-maxrate=1747:vbv-bufsize=3495"));
        let again = plan(&enc, &hd(), Some(900)).unwrap().video.unwrap();
        assert_eq!(option(&again, "b").as_deref(), Some("900k"), "deuxième essai : le débit corrigé l'emporte");
        assert_eq!(video_kbps(&enc, &MediaInfo { duration: 0.0, ..hd() }), None, "durée inconnue : retour à la qualité");
    }

    #[test]
    fn gif_webp_et_mp3() {
        let gif = plan(&Encode { container: Container::Gif, resolution: Resolution::P480, fps: Fps::F15, ..Default::default() }, &hd(), None).unwrap();
        let v = gif.video.unwrap();
        assert_eq!((v.encoder, v.pixel, gif.audio), ("gif", Pixel::PAL8, None));
        assert!(v.filters.starts_with("fps=15,scale=-2:480:flags=lanczos,split[a][b];[a]palettegen=max_colors=128"), "{}", v.filters);
        let webp = plan(&Encode { container: Container::Webp, compression: 0, ..Default::default() }, &hd(), None).unwrap();
        let v = webp.video.unwrap();
        assert_eq!((v.encoder, option(&v, "quality").as_deref(), v.filters.as_str()), ("libwebp_anim", Some("90"), "format=yuv420p"));
        assert_eq!(webp.muxer, [("loop", "0".to_owned())]);
        let mp3 = plan(&Encode { container: Container::Mp3, audio: Audio::K192, ..Default::default() }, &hd(), None).unwrap();
        assert_eq!((mp3.video, mp3.audio), (None, Some(AudioPlan { encoder: "libmp3lame", kbps: 192 })));
        assert!(plan(&Encode { container: Container::Mp3, ..Default::default() }, &MediaInfo { has_audio: false, ..hd() }, None).is_err());
        assert!(plan(&Encode::default(), &MediaInfo { has_video: false, ..hd() }, None).is_err());
    }

    #[test]
    fn webp_anime_en_entree() {
        let w = WebpInfo { width: 400, height: 300, frames: 50, duration_ms: 2000, has_alpha: true };
        let info = MediaInfo { duration: 2.0, width: 400, height: 300, fps: 25.0, vcodec: "webp".into(), has_video: true, has_audio: false, size: 1, rotation: 0, webp: Some(w) };
        let p = plan(&Encode { fps: Fps::F12, ..Default::default() }, &info, None).unwrap();
        assert_eq!(p.feed, Some(Feed { fps: 12.0, keep_alpha: false }), "la cadence choisie est appliquée au décodage");
        assert_eq!(p.video.unwrap().filters, "format=yuv420p", "pas de filtre de cadence : les images arrivent déjà au bon rythme");
        let gif = plan(&Encode { container: Container::Gif, ..Default::default() }, &info, None).unwrap();
        assert_eq!(gif.feed, Some(Feed { fps: 25.0, keep_alpha: true }), "un GIF garde la transparence");
        let again = plan(&Encode { container: Container::Webp, ..Default::default() }, &info, None).unwrap().video.unwrap();
        assert_eq!(again.pixel, Pixel::YUVA420P);
    }

    #[test]
    fn vignette_reduite_et_redressee() {
        assert_eq!(fit(1920, 1080, (192, 108)), (192, 108));
        assert_eq!(fit(1080, 1920, (192, 108)), (61, 108));
        assert_eq!(fit(50, 40, (192, 108)), (50, 40), "jamais agrandie");
        let (w, h, px) = downscale(4, 2, &[255u8; 32], (2, 1));
        assert_eq!((w, h, px), (2, 1, vec![255u8; 8]));
        // Deux pixels côte à côte, rouge puis bleu : après un quart de tour, rouge au-dessus du bleu.
        let (w, h, px) = quarter_turn(2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]);
        assert_eq!((w, h, px), (1, 2, vec![255, 0, 0, 255, 0, 0, 255, 255]));
    }
}

/// De quoi fabriquer un fichier d'essai sans rien d'autre que FFmpeg lui-même : une mire et un son
/// sortis de ses filtres, encodés en H.264 et AAC.
#[cfg(test)]
pub mod testing {
    use super::*;

    /// Un graphe qui n'a pas d'entrée : sa source est un filtre (`testsrc2`, `sine`).
    fn source_graph(sink: &str, spec: &str) -> filter::Graph {
        let mut graph = filter::Graph::new();
        graph.add(&filter::find(sink).unwrap(), "out", "").unwrap();
        graph.input("out", 0).unwrap().parse(spec).unwrap();
        graph.validate().unwrap();
        graph
    }

    fn write(encoder: &mut encoder::Encoder, index: usize, from: Rational, to: Rational, octx: &mut format::context::Output) {
        let mut packet = Packet::empty();
        while encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(index);
            packet.rescale_ts(from, to);
            packet.write_interleaved(octx).unwrap();
        }
    }

    /// Écrit dans `path` une mire de `seconds` secondes à 30 images par seconde, avec un la à 440 Hz.
    pub fn sample(path: &Path, seconds: u32, width: u32, height: u32) {
        sample_turned(path, seconds, width, height, None);
    }

    /// La même mire, marquée comme une vidéo de téléphone : enregistrée couchée, avec l'indication
    /// de la tourner de `degrees` dans le sens horaire pour l'afficher (la matrice d'affichage).
    pub fn sample_turned(path: &Path, seconds: u32, width: u32, height: u32, degrees: Option<f64>) {
        init();
        let mut octx = format::output(path).unwrap();
        let global = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);

        let mut video_graph = source_graph("buffersink", &format!("testsrc2=duration={seconds}:size={width}x{height}:rate=30,format=yuv420p"));
        let video_codec = find_encoder("libx264").unwrap();
        let (video_index, mut video_encoder) = {
            let mut stream = octx.add_stream(video_codec).unwrap();
            let mut video = codec::context::Context::new_with_codec(video_codec).encoder().video().unwrap();
            video.set_width(width);
            video.set_height(height);
            video.set_format(Pixel::YUV420P);
            video.set_time_base(Rational(1, 30));
            video.set_frame_rate(Some(Rational(30, 1)));
            if global {
                video.set_flags(codec::Flags::GLOBAL_HEADER);
            }
            // SAFETY : le contexte de l'encodeur est à nous, pas encore ouvert.
            unsafe { (*video.as_mut_ptr()).flags |= ffi::AV_CODEC_FLAG_FRAME_DURATION as i32 };
            let encoder = video.open_with(dictionary(&[("preset", "ultrafast".to_owned()), ("crf", "23".to_owned())])).unwrap();
            stream.set_parameters(&encoder);
            if let Some(degrees) = degrees {
                // SAFETY : la piste vient d'être créée ; la matrice fait 9 entiers de 32 bits.
                unsafe {
                    let par = stream.parameters().as_mut_ptr();
                    let sd = ffi::av_packet_side_data_new(&mut (*par).coded_side_data, &mut (*par).nb_coded_side_data, ffi::AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX, 36, 0);
                    assert!(!sd.is_null());
                    ffi::av_display_rotation_set((*sd).data as *mut i32, degrees);
                }
            }
            (stream.index(), encoder)
        };

        let mut audio_graph = source_graph("abuffersink", &format!("sine=frequency=440:duration={seconds}:sample_rate=48000,aformat=sample_fmts=fltp:channel_layouts=stereo"));
        let audio_codec = find_encoder("aac").unwrap();
        let (audio_index, mut audio_encoder) = {
            let mut stream = octx.add_stream(audio_codec).unwrap();
            let mut audio = codec::context::Context::new_with_codec(audio_codec).encoder().audio().unwrap();
            audio.set_rate(48_000);
            audio.set_channel_layout(av::ChannelLayout::STEREO);
            audio.set_format(format::Sample::F32(format::sample::Type::Planar));
            audio.set_bit_rate(128_000);
            audio.set_time_base(Rational(1, 48_000));
            if global {
                audio.set_flags(codec::Flags::GLOBAL_HEADER);
            }
            let encoder = audio.open_as(audio_codec).unwrap();
            stream.set_parameters(&encoder);
            (stream.index(), encoder)
        };
        audio_graph.get("out").unwrap().sink().set_frame_size(audio_encoder.frame_size());

        octx.write_header().unwrap();
        let video_tb = octx.stream(video_index).unwrap().time_base();
        let audio_tb = octx.stream(audio_index).unwrap().time_base();

        let mut image = frame::Video::empty();
        while video_graph.get("out").unwrap().sink().frame(&mut image).is_ok() {
            image.set_kind(av::picture::Type::None);
            video_encoder.send_frame(&image).unwrap();
            write(&mut video_encoder, video_index, Rational(1, 30), video_tb, &mut octx);
        }
        video_encoder.send_eof().unwrap();
        write(&mut video_encoder, video_index, Rational(1, 30), video_tb, &mut octx);

        let mut samples = frame::Audio::empty();
        while audio_graph.get("out").unwrap().sink().frame(&mut samples).is_ok() {
            audio_encoder.send_frame(&samples).unwrap();
            write(&mut audio_encoder, audio_index, Rational(1, 48_000), audio_tb, &mut octx);
        }
        audio_encoder.send_eof().unwrap();
        write(&mut audio_encoder, audio_index, Rational(1, 48_000), audio_tb, &mut octx);
        octx.write_trailer().unwrap();
    }
}
