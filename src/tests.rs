//! Les tests de l'interface (egui_kittest : l'application tourne hors écran, on clique par le nom
//! des widgets) et de bout en bout : de vrais fichiers, fabriqués puis encodés par le FFmpeg lié
//! dans l'exécutable, et relus pour vérifier ce qui a été produit.
//!
//! En test, rien n'est lancé, lu ni écrit en dehors de la fenêtre (`dry_run`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use crate::app::{App, Batch, Dialog};
use crate::engine::{self, EncodeError, MediaInfo};
use crate::i18n::Lang;
use crate::player::Player;
use crate::queue::{self, Item, Job, Msg, State};
use crate::settings::{Audio, Container, Encode, Fps, Mode, Resolution, Settings, Speed, VideoCodec};

/// Taille par défaut de la fenêtre, et taille minimale (pour traquer les effets de bord).
const NORMAL: (f32, f32) = (1080.0, 740.0);
const MINI: (f32, f32) = (960.0, 680.0);

fn harness_sized(size: (f32, f32)) -> Harness<'static, App> {
    harness_scaled(size, 1.0)
}

/// La même fenêtre, rendue à `scale` pixels par point (2 pour les images du site).
fn harness_scaled((w, h): (f32, f32), scale: f32) -> Harness<'static, App> {
    let mut harness = Harness::builder().with_size(egui::vec2(w, h)).with_pixels_per_point(scale).with_max_steps(40).build_eframe(|cc| App::with_settings(cc, Settings::default(), true));
    harness.run();
    harness
}

fn harness() -> Harness<'static, App> {
    harness_sized(NORMAL)
}

/// Un widget porte-t-il ce nom ? (Un réglage en porte souvent deux : son libellé et son contrôle.)
fn has(h: &Harness<'static, App>, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

/// Clique le premier widget portant ce nom.
fn click(h: &mut Harness<'static, App>, label: &str) {
    h.query_all_by_label(label).next().unwrap_or_else(|| panic!("aucun widget « {label} »")).click_accesskit();
    h.run();
}

fn info(width: u32, height: u32, fps: f64, duration: f64, size: u64, vcodec: &str) -> MediaInfo {
    MediaInfo { duration, width, height, fps, vcodec: vcodec.into(), has_video: true, has_audio: true, size, rotation: 0, webp: None }
}

/// Une liste qui montre chaque état d'un fichier.
fn fill(app: &mut App) {
    let rows = [
        ("Réunion de lancement.mov", info(3840, 2160, 29.97, 1284.0, 2_840_000_000, "hevc"), State::Done { output: PathBuf::from(r"C:\Vidéos\Réunion de lancement-compresse.mp4"), size: 412_000_000 }),
        ("Démo produit, prise 3.mp4", info(1920, 1080, 60.0, 187.0, 486_200_000, "h264"), State::Running { fraction: 0.42, speed: 3.1, started: Instant::now() - Duration::from_secs(21) }),
        ("capture-ecran-2026-11-16.mkv", info(2560, 1440, 30.0, 95.0, 152_000_000, "h264"), State::Queued),
        ("autocollant-anime.webp", info(512, 512, 25.0, 4.2, 1_840_000, "webp"), State::Ready),
        ("vieux-camescope.avi", info(720, 576, 25.0, 3605.0, 1_250_000_000, "mpeg4"), State::Failed("Invalid data found when processing input".into())),
    ];
    for (i, (name, info, state)) in rows.into_iter().enumerate() {
        let mut item = Item::new(100 + i as u64, PathBuf::from(format!(r"C:\Vidéos\{name}")));
        item.info = Some(info);
        item.state = state;
        app.items.push(item);
    }
    app.menu_on = true;
}

/// Une image de synthèse en 16/9, pour voir l'aperçu en grand dans les captures.
fn picture(h: &Harness<'static, App>) -> egui::TextureHandle {
    let (w, hgt) = (640usize, 360usize);
    let mut rgba = Vec::with_capacity(w * hgt * 4);
    for y in 0..hgt {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / hgt as f32);
            // Un ciel, une colline, un soleil : de quoi juger le cadre.
            let hill = 0.62 + 0.10 * (fx * 6.3).sin() + 0.05 * (fx * 17.0).cos();
            let sun = ((fx - 0.72).powi(2) * 3.2 + (fy - 0.30).powi(2)).sqrt();
            let color = if fy > hill {
                [(38.0 + 30.0 * fy) as u8, (52.0 + 40.0 * (1.0 - fy)) as u8, (60.0 + 20.0 * fx) as u8]
            } else if sun < 0.09 {
                [246, 226, 170]
            } else {
                [(70.0 + 150.0 * fy) as u8, (96.0 + 80.0 * fy) as u8, (150.0 - 40.0 * fy) as u8]
            };
            rgba.extend_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
    h.ctx.load_texture("mire", egui::ColorImage::from_rgba_unmultiplied([w, hgt], &rgba), egui::TextureOptions::LINEAR)
}

#[test]
fn fenetre_vide() {
    let h = harness();
    h.get_by_label("Que veux-tu en faire");
    h.get_by_label("Choisir des vidéos");
    h.get_by_label("Compresser");
    for choice in ["Web 1080p", "Léger 720p", "Sous 10 Mo", "GIF animé", "Garder seulement le son", "Réglages fins"] {
        h.get_by_label(choice);
    }
    assert!(!has(&h, "Format"), "les réglages fins sont repliés au départ");
    h.get_by_label(format!("FFmpeg {} intégré", engine::version()).as_str());
    assert!(h.state().items.is_empty() && !h.state().running());
}

#[test]
fn un_choix_remplit_les_reglages() {
    let mut h = harness();
    click(&mut h, "GIF animé");
    let enc = h.state().settings.encode;
    assert_eq!((enc.container, enc.resolution, enc.fps), (Container::Gif, Resolution::P480, Fps::F15));
    h.get_by_label("Convertir");
    click(&mut h, "Réglages fins");
    assert!(h.state().fine_open);
    assert!(has(&h, "Format") && has(&h, "Résolution"));
    assert!(!has(&h, "Codec") && !has(&h, "Son"), "un GIF n'a ni codec à choisir ni son");
    // Dépliés, les choix tiennent sur une ligne, sous leur nom court.
    click(&mut h, "Sous 10 Mo");
    let enc = h.state().settings.encode;
    assert_eq!((enc.container, enc.mode, enc.target_mb), (Container::Mp4, Mode::TargetSize, 10.0));
    h.get_by_label("Taille à ne pas dépasser");
    assert!(has(&h, "Codec") && has(&h, "Son"));
    click(&mut h, "Par la qualité");
    assert_eq!(h.state().settings.encode.mode, Mode::Quality);
    h.get_by_label("Degré de compression");
    click(&mut h, "H.265");
    assert_eq!(h.state().settings.encode.codec, VideoCodec::H265);
    click(&mut h, "Son seul");
    assert_eq!(h.state().settings.encode.container, Container::Mp3);
    assert!(!has(&h, "Codec") && !has(&h, "Résolution") && has(&h, "Son"));
    click(&mut h, "Réglages fins");
    assert!(!h.state().fine_open && !has(&h, "Format"));
}

#[test]
fn preferences_et_clic_droit() {
    let mut h = harness();
    click(&mut h, "Préférences");
    assert_eq!(h.state().dialog, Some(Dialog::Prefs));
    let label = format!("Proposer « {} » sur les vidéos", crate::shell::menu_label());
    for expected in [true, false] {
        h.get_by_role_and_label(Role::Switch, &label).click_accesskit();
        h.run();
        assert_eq!(h.state().menu_on, expected, "en test, le registre n'est pas touché");
    }
    click(&mut h, "Fermer");
    assert_eq!(h.state().dialog, None);
    click(&mut h, "Mises à jour");
    assert_eq!(h.state().dialog, Some(Dialog::Update));
}

/// La vidéo choisie passe en grand et dit où elle en est ; les autres attendent dessous.
#[test]
fn la_selection_montre_chaque_etat() {
    let mut h = harness();
    fill(h.state_mut());
    h.run_steps(3);
    assert_eq!(h.state().selected, Some(100), "la première vidéo est choisie d'office");
    for label in ["Réunion de lancement.mov", "→  412,0 Mo  ·  85 % de moins", "Lire la vidéo", "Lire", "Montrer", "Compresser 2 fichiers", "Clic droit actif"] {
        h.get_by_label(label);
    }
    click(&mut h, "vieux-camescope.avi");
    assert_eq!(h.state().selected, Some(104));
    h.get_by_label("Échec");
    h.get_by_label("Réessayer");
    click(&mut h, "capture-ecran-2026-11-16.mkv");
    h.get_by_label("En attente");
    assert!(!has(&h, "Retirer de la liste"), "une vidéo en attente ne se retire pas");
    click(&mut h, "autocollant-anime.webp");
    h.get_by_label("Prêt");
    click(&mut h, "Retirer de la liste");
    assert_eq!(h.state().items.len(), 4);
    assert_eq!(h.state().selected, Some(104), "la suivante prend la place de celle qu'on retire");
}

/// Aucun widget ne dépasse de la fenêtre, à la taille normale comme à la taille minimale,
/// réglages fins repliés ou dépliés.
#[test]
fn rien_ne_deborde() {
    use egui_kittest::kittest::NodeT as _;
    for size in [NORMAL, MINI] {
        for fine in [false, true] {
            let mut h = harness_sized(size);
            fill(h.state_mut());
            h.state_mut().fine_open = fine;
            h.state_mut().menu_on = false;
            h.run_steps(3);
            let out: Vec<String> = h
                .root()
                .children_recursive()
                .filter(|n| n.accesskit_node().bounding_box().is_some_and(|b| b.x1 > f64::from(size.0) + 1.0 || b.y1 > f64::from(size.1) + 1.0 || b.x0 < -1.0))
                .filter_map(|n| n.accesskit_node().label())
                .collect();
            assert!(out.is_empty(), "dépassent à {size:?} (réglages fins {fine}) : {out:?}");
        }
    }
}

/// Les captures de l'interface, à regarder : `target/captures/*.png`.
#[test]
fn captures() {
    std::fs::create_dir_all("target/captures").unwrap();
    for (name, size) in [("normal", NORMAL), ("mini", MINI)] {
        let mut h = harness_sized(size);
        h.render().unwrap().save(format!("target/captures/{name}-vide.png")).unwrap();
        fill(h.state_mut());
        let texture = picture(&h);
        h.state_mut().items[0].thumb = Some(texture.clone());
        h.state_mut().items[1].thumb = Some(texture);
        h.run_steps(3);
        h.render().unwrap().save(format!("target/captures/{name}-choix.png")).unwrap();
        // Le lecteur de l'aperçu sur un vrai fichier : en lecture, puis en pause.
        let dir = workspace(&format!("captures-{name}"));
        let clip = sample(&dir, 3, 640, 360);
        let player = Player::open(100, clip.clone(), &engine::probe(&clip).unwrap(), h.ctx.clone());
        wait_for(5, "quelques images du lecteur", || player.shown().1 >= 20);
        h.state_mut().player = Some(player);
        h.run_steps(3);
        h.render().unwrap().save(format!("target/captures/{name}-lecture.png")).unwrap();
        h.state().player.as_ref().unwrap().toggle();
        std::thread::sleep(Duration::from_millis(150));
        h.run_steps(3);
        h.get_by_label("Position dans la vidéo");
        h.render().unwrap().save(format!("target/captures/{name}-pause.png")).unwrap();
        h.state_mut().player = None;
        let _ = std::fs::remove_dir_all(&dir);
        h.state_mut().batch = Some(Batch { cancel: Arc::new(AtomicBool::new(false)), total: 4, finished: 1, ok: 1, failed: 0, saved: 0 });
        h.state_mut().selected = Some(101);
        h.run_steps(3);
        h.render().unwrap().save(format!("target/captures/{name}-en-cours.png")).unwrap();
        h.state_mut().batch = None;
        h.state_mut().menu_on = false;
        h.state_mut().selected = Some(103);
        h.state_mut().fine_open = true;
        h.run_steps(3);
        h.render().unwrap().save(format!("target/captures/{name}-reglages-fins.png")).unwrap();
        h.state_mut().settings.encode = Encode { mode: Mode::TargetSize, target_mb: 10.0, resolution: Resolution::P720, ..Default::default() };
        h.state_mut().dialog = Some(Dialog::Prefs);
        h.run_steps(3);
        h.render().unwrap().save(format!("target/captures/{name}-preferences.png")).unwrap();
        h.state_mut().dialog = Some(Dialog::Update);
        h.run_steps(3);
        h.render().unwrap().save(format!("target/captures/{name}-mise-a-jour.png")).unwrap();
    }
}

/// Les images du site (en anglais) et du README (en français), en double densité.
/// `scripts/landing-shots.py` les met en place. À lancer seul, parce qu'il change la langue,
/// qui est commune à tous les tests : `cargo test captures_du_site -- --ignored`.
#[test]
#[ignore = "change la langue de tout le processus"]
fn captures_du_site() {
    std::fs::create_dir_all("target/captures").unwrap();
    for (lang, code) in [(Lang::En, "en"), (Lang::Fr, "fr")] {
        crate::i18n::set(lang);
        let mut h = harness_scaled(NORMAL, 2.0);
        fill(h.state_mut());
        // Le site est dans les teintes ambre : ses captures aussi.
        h.state_mut().settings.accent = crate::theme::Accent::Amber;
        if lang == Lang::En {
            let names = ["Kickoff meeting.mov", "Product demo, take 3.mp4", "screen-capture-2026-11-16.mkv", "animated-sticker.webp", "old-camcorder.avi"];
            for (item, name) in h.state_mut().items.iter_mut().zip(names) {
                item.path = PathBuf::from(format!(r"C:\Videos\{name}"));
                item.name = name.to_owned();
            }
            h.state_mut().settings.suffix = "-compressed".into();
        }
        let texture = picture(&h);
        h.state_mut().items[0].thumb = Some(texture.clone());
        h.state_mut().items[1].thumb = Some(texture);
        h.run_steps(3);
        h.render().unwrap().save(format!("target/captures/site-{code}-choix.png")).unwrap();
        h.state_mut().fine_open = true;
        h.run_steps(3);
        h.render().unwrap().save(format!("target/captures/site-{code}-reglages-fins.png")).unwrap();
        h.state_mut().fine_open = false;
        h.state_mut().batch = Some(Batch { cancel: Arc::new(AtomicBool::new(false)), total: 4, finished: 1, ok: 1, failed: 0, saved: 0 });
        h.state_mut().selected = Some(101);
        h.run_steps(3);
        h.render().unwrap().save(format!("target/captures/site-{code}-en-cours.png")).unwrap();
    }
    crate::i18n::set(Lang::Fr);
}

// ---- de bout en bout, avec le FFmpeg lié dans l'exécutable -------------------------------------

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vc-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Une mire de `seconds` secondes avec un son.
fn sample(dir: &Path, seconds: u32, width: u32, height: u32) -> PathBuf {
    let path = dir.join("mire.mp4");
    engine::testing::sample(&path, seconds, width, height);
    path
}

/// Encode `input` avec ces réglages, puis relit le fichier produit.
fn convert(input: &Path, enc: &Encode, name: &str) -> MediaInfo {
    let info = engine::probe(input).unwrap();
    let output = input.with_file_name(format!("{name}.{}", enc.container.ext()));
    let mut last = 0.0f32;
    engine::encode(enc, &info, input, &output, &AtomicBool::new(false), &mut |fraction, _| last = fraction).unwrap_or_else(|e| panic!("{name} : {e:?}"));
    assert!(last > 0.5, "{name} : la progression n'a pas avancé ({last})");
    engine::probe(&output).unwrap_or_else(|e| panic!("{name} : fichier produit illisible ({e})"))
}

#[test]
fn e2e_formats_et_reglages() {
    let dir = workspace("formats");
    let input = sample(&dir, 3, 640, 360);
    let source = engine::probe(&input).unwrap();
    assert_eq!((source.width, source.height, source.fps.round(), source.has_audio, source.vcodec.as_str()), (640, 360, 30.0, true, "h264"));
    assert!((source.duration - 3.0).abs() < 0.2, "durée {}", source.duration);
    let thumb = engine::thumbnail(&input, &source, queue::THUMB).unwrap();
    assert_eq!((thumb.0, thumb.1, thumb.2.len()), (640, 360, 640 * 360 * 4));
    assert!(thumb.2.iter().any(|b| *b > 40), "la vignette n'est pas noire");

    let fast = Encode { speed: Speed::Fast, ..Default::default() };
    let mp4 = convert(&input, &fast, "defaut");
    assert_eq!((mp4.width, mp4.height, mp4.vcodec.as_str(), mp4.has_audio), (640, 360, "h264", true));
    assert!((mp4.duration - 3.0).abs() < 0.3, "durée {}", mp4.duration);
    assert_eq!(mp4.fps.round(), 30.0);

    let small = convert(&input, &Encode { codec: VideoCodec::H265, resolution: Resolution::P240, fps: Fps::F15, audio: Audio::None, ..fast }, "h265-240p");
    assert_eq!((small.height, small.fps.round(), small.vcodec.as_str(), small.has_audio), (240, 15.0, "hevc", false));

    let webm = convert(&input, &Encode { container: Container::Webm, codec: VideoCodec::Vp9, ..fast }, "vp9");
    assert_eq!((webm.vcodec.as_str(), webm.has_audio, webm.width), ("vp9", true, 640));

    let av1 = convert(&input, &Encode { codec: VideoCodec::Av1, resolution: Resolution::P240, ..fast }, "av1");
    assert_eq!((av1.vcodec.as_str(), av1.height), ("av1", 240));

    let mkv = convert(&input, &Encode { container: Container::Mkv, resolution: Resolution::P240, ..fast }, "mkv");
    assert_eq!((mkv.vcodec.as_str(), mkv.height, mkv.has_audio), ("h264", 240, true));

    let gif = convert(&input, &Encode { container: Container::Gif, resolution: Resolution::P240, fps: Fps::F10, ..fast }, "gif");
    assert_eq!((gif.vcodec.as_str(), gif.height, gif.has_audio), ("gif", 240, false));

    let mp3 = convert(&input, &Encode { container: Container::Mp3, ..fast }, "son");
    assert!(!mp3.has_video && mp3.has_audio && (mp3.duration - 3.0).abs() < 0.3);

    // Une taille cible : le fichier reste sous la limite, et utilise le débit qu'on lui donne.
    let target = Encode { mode: Mode::TargetSize, target_mb: 1.0, ..fast };
    let limited = convert(&input, &target, "taille-cible");
    assert!(limited.size <= 1_000_000, "{} octets pour 1 Mo demandé", limited.size);
    assert!(limited.size > 400_000, "le débit visé est bien utilisé ({} octets)", limited.size);
    let h265 = convert(&input, &Encode { codec: VideoCodec::H265, ..target }, "taille-cible-h265");
    assert!(h265.size <= 1_000_000, "{} octets pour 1 Mo demandé", h265.size);

    // Dans l'autre sens : chaque fichier produit est relu et reconverti en MP4, ce qui vérifie la
    // lecture du H.265, du VP9 avec Opus, de l'AV1, du Matroska, du GIF et du MP3.
    let back = Encode { resolution: Resolution::P240, speed: Speed::Fast, ..Default::default() };
    for (name, audio) in [("h265-240p.mp4", false), ("vp9.webm", true), ("av1.mp4", true), ("mkv.mkv", true), ("gif.gif", false)] {
        let again = convert(&dir.join(name), &back, &format!("retour-{}", name.replace('.', "-")));
        assert_eq!((again.vcodec.as_str(), again.height, again.has_audio), ("h264", 240, audio), "{name}");
        assert!((again.duration - 3.0).abs() < 0.35, "{name} : durée {}", again.duration);
    }
    let from_mp3 = convert(&dir.join("son.mp3"), &Encode { container: Container::Mp3, audio: Audio::K64, ..Default::default() }, "retour-son");
    assert!(from_mp3.has_audio && from_mp3.size < mp3.size, "un MP3 à 64 kb/s pèse moins qu'à 128 ({} contre {})", from_mp3.size, mp3.size);
    assert!(engine::plan(&back, &mp3, None).is_err(), "pas de vidéo à tirer d'un MP3");
    let _ = std::fs::remove_dir_all(&dir);
}

/// L'exemple de départ : un WebP animé devient un MP4.
#[test]
fn e2e_webp_anime_vers_mp4() {
    let dir = workspace("webp");
    let input = sample(&dir, 2, 160, 120);
    // Le WebP animé est fabriqué par le moteur lui-même, à partir de la mire.
    let made = convert(&input, &Encode { container: Container::Webp, fps: Fps::F10, ..Default::default() }, "animation");
    let webp = dir.join("animation.webp");
    let anim = made.webp.expect("le WebP produit est animé, et relu par notre décodeur");
    assert_eq!((anim.width, anim.height, anim.frames, made.fps), (160, 120, 20, 10.0));
    assert!(engine::thumbnail(&webp, &made, queue::THUMB).is_some());

    let mp4 = convert(&webp, &Encode { speed: Speed::Fast, ..Default::default() }, "en-mp4");
    assert_eq!((mp4.width, mp4.height, mp4.fps.round(), mp4.vcodec.as_str(), mp4.has_audio), (160, 120, 10.0, "h264", false));
    assert!((mp4.duration - 2.0).abs() < 0.25, "durée {}", mp4.duration);

    let webm = convert(&webp, &Encode { resolution: Resolution::P240, container: Container::Webm, codec: VideoCodec::Vp9, speed: Speed::Fast, ..Default::default() }, "en-webm");
    assert_eq!((webm.vcodec.as_str(), webm.height), ("vp9", 120), "jamais agrandi");
    let gif = convert(&webp, &Encode { container: Container::Gif, ..Default::default() }, "en-gif");
    assert_eq!((gif.vcodec.as_str(), gif.width), ("gif", 160));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Attend qu'une condition soit vraie, au plus `seconds` secondes.
fn wait_for(seconds: u64, what: &str, mut done: impl FnMut() -> bool) {
    let limit = Instant::now() + Duration::from_secs(seconds);
    while !done() {
        assert!(Instant::now() < limit, "toujours pas : {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Le lecteur de l'aperçu : il lit à la cadence de la vidéo, se met en pause, saute à un instant,
/// s'arrête au bout, redresse une vidéo de téléphone et rejoue un WebP animé.
#[test]
fn e2e_lecteur_de_l_apercu() {
    let dir = workspace("lecteur");
    let input = sample(&dir, 3, 640, 360);
    let info = engine::probe(&input).unwrap();
    let ctx = egui::Context::default();

    let started = Instant::now();
    let player = Player::open(1, input.clone(), &info, ctx.clone());
    assert!(player.playing() && player.can_seek);
    wait_for(5, "trente images lues", || player.shown().1 >= 30);
    assert_eq!(player.shown().0, Some((640, 360)));
    // Trente images à 30 par seconde : une seconde de vidéo, en une seconde environ.
    let (elapsed, position) = (started.elapsed().as_secs_f64(), player.position());
    assert!((0.8..1.3).contains(&position), "position {position}");
    assert!(elapsed > 0.8, "la lecture suit l'horloge, pas la vitesse du décodeur ({elapsed} s)");

    player.toggle();
    std::thread::sleep(Duration::from_millis(150));
    let paused = player.shown().1;
    std::thread::sleep(Duration::from_millis(300));
    assert!(!player.playing() && player.shown().1 == paused, "en pause, plus aucune image n'arrive");

    player.seek(2.0);
    wait_for(5, "l'image du saut", || (player.position() - 2.0).abs() < 0.05);
    assert!(!player.playing(), "un saut en pause reste en pause");
    assert_eq!(player.shown().1, paused + 1, "une seule image montrée, celle de l'instant demandé");

    player.toggle();
    wait_for(5, "la fin de la vidéo", || player.ended());
    assert!(!player.playing() && (player.position() - info.duration).abs() < 0.1, "position {}", player.position());
    // Au bout, relancer repart du début.
    player.toggle();
    wait_for(5, "la reprise au début", || player.playing() && player.position() < 1.0 && !player.ended());
    assert!(player.error().is_none());
    drop(player);

    // Dans la fenêtre : un clic sur l'image lit, un autre met en pause, choisir une autre vidéo ferme.
    let mut h = harness();
    for id in [7, 8] {
        let mut item = Item::new(id, input.clone());
        (item.info, item.state) = (Some(info.clone()), State::Ready);
        h.state_mut().items.push(item);
    }
    h.state_mut().dry_run = false;
    h.run_steps(2);
    h.get_by_label("Lire la vidéo").click_accesskit();
    h.run_steps(3);
    assert!(h.state().player.as_ref().is_some_and(|p| p.item == 7 && p.playing()));
    wait_for(5, "une image dans la fenêtre", || h.state().player.as_ref().unwrap().shown().1 >= 3);
    h.run_steps(2);
    h.get_by_label("Mettre en pause").click_accesskit();
    h.run_steps(3);
    assert!(h.state().player.as_ref().is_some_and(|p| !p.playing()));
    h.state_mut().selected = Some(8);
    h.run_steps(2);
    assert!(h.state().player.is_none(), "le lecteur suit la vidéo choisie");
    drop(h);

    // Une vidéo de téléphone : l'image arrive debout.
    let phone = dir.join("telephone.mp4");
    engine::testing::sample_turned(&phone, 2, 320, 240, Some(90.0));
    let turned = Player::open(2, phone.clone(), &engine::probe(&phone).unwrap(), ctx.clone());
    wait_for(5, "une image de la vidéo de téléphone", || turned.shown().1 >= 1);
    assert_eq!(turned.shown().0, Some((240, 320)));
    drop(turned);

    // Un WebP animé : rejoué par son propre décodeur, d'un bout à l'autre, sans saut.
    convert(&input, &Encode { container: Container::Webp, resolution: Resolution::P240, fps: Fps::F10, ..Default::default() }, "animation");
    let webp = dir.join("animation.webp");
    let info = engine::probe(&webp).unwrap();
    assert!(info.webp.is_some());
    let animation = Player::open(3, webp, &info, ctx);
    assert!(!animation.can_seek);
    wait_for(10, "la fin de l'animation", || animation.ended());
    let (size, frames) = animation.shown();
    assert_eq!(size.map(|s| s.1), Some(240));
    assert!((25..=35).contains(&frames), "{frames} images pour 3 s à 10 images par seconde");
    assert!(animation.error().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Une vidéo de téléphone tenue debout : enregistrée couchée avec l'ordre de la tourner. Elle est
/// mesurée debout, sa vignette aussi, et le fichier produit est debout sans plus rien à tourner.
#[test]
fn e2e_video_de_telephone_redressee() {
    let dir = workspace("rotation");
    let input = dir.join("telephone.mp4");
    // Un quart de tour dans le sens horaire, ce qu'écrit un téléphone tenu debout.
    engine::testing::sample_turned(&input, 2, 320, 240, Some(90.0));
    let source = engine::probe(&input).unwrap();
    assert_eq!((source.rotation, source.width, source.height), (90, 240, 320));
    let thumb = engine::thumbnail(&input, &source, queue::THUMB).unwrap();
    assert_eq!((thumb.0, thumb.1), (240, 320), "la vignette est debout, et jamais agrandie");

    let made = convert(&input, &Encode { speed: Speed::Fast, ..Default::default() }, "debout");
    assert_eq!((made.rotation, made.width, made.height), (0, 240, 320));
    let small = convert(&input, &Encode { resolution: Resolution::P240, container: Container::Gif, ..Default::default() }, "debout-gif");
    assert_eq!((small.width, small.height), (240, 320), "240p vise le petit côté, déjà à 240");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Le lot complet, comme l'interface le lance : messages, fichier renommé à la fin, annulation.
#[test]
fn e2e_lot_et_annulation() {
    let dir = workspace("lot");
    let input = sample(&dir, 20, 1280, 720);
    let info = engine::probe(&input).unwrap();
    let ctx = egui::Context::default();
    let run = |enc: Encode, cancel_after: Option<Duration>| {
        let output = queue::output_path(&input, &enc, "", "-compresse", &[]);
        let (tx, rx) = channel();
        let cancel = queue::spawn_batch(enc, vec![Job { id: 7, input: input.clone(), info: info.clone(), output: output.clone() }], tx, ctx.clone());
        let (mut started, mut progressed, mut result) = (false, false, None);
        let begin = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Msg::Started { id: 7 }) => started = true,
                Ok(Msg::Progress { .. }) => progressed = true,
                Ok(Msg::Finished { result: r, .. }) => result = Some(r),
                Ok(Msg::BatchDone) => break,
                _ => {}
            }
            if cancel_after.is_some_and(|d| begin.elapsed() > d) {
                cancel.store(true, Ordering::Relaxed);
            }
            assert!(begin.elapsed() < Duration::from_secs(300), "lot trop long");
        }
        (output, started, progressed, result.expect("un résultat par fichier"))
    };

    let (output, started, progressed, result) = run(Encode { resolution: Resolution::P360, speed: Speed::Fast, ..Default::default() }, None);
    let (path, size) = result.unwrap();
    assert!(started && progressed && path == output && output.is_file());
    assert_eq!(size, std::fs::metadata(&output).unwrap().len());
    assert!(!dir.join("mire-compresse.partial.mp4").exists(), "le fichier en cours a été renommé");
    let made = engine::probe(&output).unwrap();
    assert_eq!((made.height, made.has_audio), (360, true));
    assert!((made.duration - 20.0).abs() < 0.5, "durée {}", made.duration);

    // Un encodage lent, arrêté au bout d'une demi-seconde : rien ne reste sur le disque.
    let (output, _, _, result) = run(Encode { codec: VideoCodec::H265, speed: Speed::Slow, ..Default::default() }, Some(Duration::from_millis(500)));
    assert_eq!(result, Err(EncodeError::Cancelled));
    assert!(!output.exists() && !dir.join("mire-compresse-2.partial.mp4").exists(), "ni fichier final ni fichier en cours");
    let _ = std::fs::remove_dir_all(&dir);
}
