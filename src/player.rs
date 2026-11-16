//! Le lecteur de l'aperçu : un fil décode la vidéo choisie et pose ses images, à leur heure, là où
//! la fenêtre vient les prendre. Il lit, se met en pause et saute à un instant. Il ne joue pas le son.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use av::format::Pixel;
use av::software::scaling;
use av::{decoder, ffi, format, frame};
use ffmpeg_next as av;

use crate::engine::{self, MediaInfo};
use crate::webp;

/// La plus grande image montrée : au-delà, l'aperçu n'y gagne rien.
const MAX: (u32, u32) = (1280, 720);

enum Cmd {
    Play,
    Pause,
    Seek(f64),
}

/// Ce que le fil de lecture laisse à la fenêtre.
#[derive(Default)]
struct Shared {
    /// La dernière image, en RGBA, tant que la fenêtre ne l'a pas prise.
    frame: Option<(u32, u32, Vec<u8>)>,
    /// La taille de la dernière image, et combien ont été montrées.
    size: Option<(u32, u32)>,
    frames: u64,
    position: f64,
    playing: bool,
    ended: bool,
    error: Option<String>,
}

pub struct Player {
    /// Le fichier de la liste que ce lecteur joue.
    pub item: u64,
    pub duration: f64,
    /// Un WebP animé se lit d'un bout à l'autre, sans saut.
    pub can_seek: bool,
    tx: Sender<Cmd>,
    shared: Arc<Mutex<Shared>>,
    texture: Option<egui::TextureHandle>,
}

impl Player {
    /// Ouvre le fichier et commence à le lire. Fermer le lecteur (le lâcher) arrête son fil.
    pub fn open(item: u64, path: PathBuf, info: &MediaInfo, ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        let shared = Arc::new(Mutex::new(Shared { playing: true, ..Default::default() }));
        let (theirs, info) = (shared.clone(), info.clone());
        let player = Self { item, duration: info.duration, can_seek: info.webp.is_none(), tx, shared, texture: None };
        std::thread::spawn(move || {
            let result = if info.webp.is_some() { play_webp(&path, &info, &rx, &theirs, &ctx) } else { play_video(&path, &info, &rx, &theirs, &ctx) };
            if let Err(why) = result {
                let mut s = theirs.lock().unwrap();
                (s.error, s.playing) = (Some(why), false);
                drop(s);
                ctx.request_repaint();
            }
        });
        player
    }

    pub fn playing(&self) -> bool {
        self.shared.lock().unwrap().playing
    }

    #[cfg(test)]
    pub fn ended(&self) -> bool {
        self.shared.lock().unwrap().ended
    }

    /// L'instant montré, en secondes depuis le début.
    pub fn position(&self) -> f64 {
        self.shared.lock().unwrap().position
    }

    pub fn error(&self) -> Option<String> {
        self.shared.lock().unwrap().error.clone()
    }

    /// Lecture ou pause. Arrivé au bout, la lecture reprend du début.
    pub fn toggle(&self) {
        let mut s = self.shared.lock().unwrap();
        s.playing = !s.playing;
        let _ = self.tx.send(if s.playing { Cmd::Play } else { Cmd::Pause });
    }

    /// Saute à `seconds` et montre l'image qui s'y trouve.
    pub fn seek(&self, seconds: f64) {
        if self.can_seek {
            let _ = self.tx.send(Cmd::Seek(seconds.clamp(0.0, (self.duration - 0.05).max(0.0))));
        }
    }

    /// L'image du moment, prête à être peinte. `None` tant qu'aucune n'est arrivée.
    pub fn texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        if let Some((w, h, rgba)) = self.shared.lock().unwrap().frame.take() {
            let size = [w as usize, h as usize];
            // Une vidéo est opaque : ses octets sont déjà ceux que la carte graphique attend.
            let image = if self.can_seek { egui::ColorImage::from_rgba_premultiplied(size, &rgba) } else { egui::ColorImage::from_rgba_unmultiplied(size, &rgba) };
            match &mut self.texture {
                Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture(format!("player-{}", self.item), image, egui::TextureOptions::LINEAR)),
            }
        }
        self.texture.clone()
    }

    /// La taille de la dernière image, et combien ont été montrées (pour les tests).
    #[cfg(test)]
    pub fn shown(&self) -> (Option<(u32, u32)>, u64) {
        let s = self.shared.lock().unwrap();
        (s.size, s.frames)
    }
}

/// Pose une image pour la fenêtre, et la réveille.
fn publish(shared: &Mutex<Shared>, ctx: &egui::Context, image: (u32, u32, Vec<u8>), position: f64) {
    let mut s = shared.lock().unwrap();
    s.size = Some((image.0, image.1));
    s.frame = Some(image);
    s.frames += 1;
    s.position = position;
    drop(s);
    ctx.request_repaint();
}

// ---- une vidéo, par FFmpeg ---------------------------------------------------------------------

/// Le fichier ouvert et où en est sa lecture.
struct Reader {
    ictx: format::context::Input,
    decoder: decoder::Video,
    index: usize,
    /// Le début du fichier, dans l'unité des sauts.
    origin: i64,
    playing: bool,
    /// En pause, une image reste à montrer (après un saut).
    step: bool,
    eof: bool,
    ended: bool,
    /// L'heure de l'ordinateur et l'instant de la vidéo qui se correspondent.
    clock: Option<(Instant, f64)>,
    /// Après un saut : les images d'avant cet instant ne sont pas montrées.
    skip_until: Option<f64>,
}

impl Reader {
    fn apply(&mut self, cmd: Cmd, shared: &Mutex<Shared>) {
        match cmd {
            Cmd::Pause => self.playing = false,
            Cmd::Play => {
                if self.ended {
                    self.seek(0.0);
                }
                self.playing = true;
                self.clock = None;
            }
            Cmd::Seek(seconds) => self.seek(seconds),
        }
        let mut s = shared.lock().unwrap();
        (s.playing, s.ended) = (self.playing, self.ended);
    }

    /// Va à l'image clé qui précède `seconds` ; les images jusqu'à cet instant seront décodées sans
    /// être montrées.
    fn seek(&mut self, seconds: f64) {
        let ts = self.origin + (seconds.max(0.0) * f64::from(ffi::AV_TIME_BASE)) as i64;
        let _ = self.ictx.seek(ts, ..ts);
        self.decoder.flush();
        (self.eof, self.ended, self.step, self.clock, self.skip_until) = (false, false, true, None, Some(seconds));
    }

    /// Décode l'image suivante. Faux à la fin du fichier.
    fn next(&mut self, decoded: &mut frame::Video) -> bool {
        loop {
            if self.decoder.receive_frame(decoded).is_ok() {
                return true;
            }
            if self.eof {
                return false;
            }
            match self.ictx.packets().next() {
                Some((stream, packet)) => {
                    if stream.index() == self.index {
                        let _ = self.decoder.send_packet(&packet);
                    }
                }
                None => {
                    let _ = self.decoder.send_eof();
                    self.eof = true;
                }
            }
        }
    }
}

fn play_video(path: &Path, info: &MediaInfo, rx: &Receiver<Cmd>, shared: &Mutex<Shared>, ctx: &egui::Context) -> Result<(), String> {
    engine::init();
    let bad = |e: av::Error| e.to_string();
    let ictx = format::input(path).map_err(bad)?;
    let index = engine::video_index(&ictx).ok_or_else(|| av::Error::StreamNotFound.to_string())?;
    let stream = ictx.stream(index).ok_or_else(|| av::Error::StreamNotFound.to_string())?;
    let time_base = f64::from(stream.time_base());
    let start = if stream.start_time() == ffi::AV_NOPTS_VALUE { 0 } else { stream.start_time() };
    let decoder = engine::open_decoder(&stream).map_err(bad)?.video().map_err(bad)?;
    // SAFETY : lecture d'un entier du contexte, vivant tant que `ictx`.
    let origin = match unsafe { (*ictx.as_ptr()).start_time } {
        ffi::AV_NOPTS_VALUE => 0,
        origin => origin,
    };
    let fps = if info.fps > 0.0 { info.fps } else { 25.0 };
    let mut r = Reader { ictx, decoder, index, origin, playing: true, step: false, eof: false, ended: false, clock: None, skip_until: None };
    let mut scaler: Option<((Pixel, u32, u32), scaling::Context)> = None;
    let (mut decoded, mut small) = (frame::Video::empty(), frame::Video::empty());
    let (mut count, mut last_shown) = (0u64, Instant::now());

    loop {
        // Les ordres de la fenêtre. Sans rien à montrer, on les attend ; fenêtre fermée, on s'arrête.
        let cmd = if !r.playing && !r.step {
            match rx.recv() {
                Ok(cmd) => Some(cmd),
                Err(_) => return Ok(()),
            }
        } else {
            match rx.try_recv() {
                Ok(cmd) => Some(cmd),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        };
        if let Some(cmd) = cmd {
            r.apply(cmd, shared);
            continue;
        }

        if !r.next(&mut decoded) {
            // La fin : on reste sur la dernière image.
            (r.playing, r.step, r.ended) = (false, false, true);
            let mut s = shared.lock().unwrap();
            (s.playing, s.ended) = (false, true);
            if r.skip_until.take().is_none() && info.duration > 0.0 {
                s.position = info.duration;
            }
            drop(s);
            ctx.request_repaint();
            continue;
        }
        let at = match decoded.timestamp() {
            Some(pts) => ((pts - start) as f64 * time_base).max(0.0),
            None => count as f64 / fps,
        };
        count += 1;
        if let Some(target) = r.skip_until {
            if at + 0.5 / fps < target {
                continue;
            }
            r.skip_until = None;
        }

        if r.playing {
            let (wall, media) = *r.clock.get_or_insert((Instant::now(), at));
            let due = wall + Duration::from_secs_f64((at - media).max(0.0));
            // On attend l'heure de l'image, en restant à l'écoute des ordres.
            let mut jumped = false;
            while let Some(wait) = due.checked_duration_since(Instant::now()).filter(|w| !w.is_zero()) {
                match rx.recv_timeout(wait) {
                    Ok(cmd) => {
                        jumped = matches!(cmd, Cmd::Seek(_));
                        r.apply(cmd, shared);
                        if jumped || !r.playing {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return Ok(()),
                }
            }
            if jumped {
                continue;
            }
            // Le décodage ne suit pas : on saute des images plutôt que de ralentir la vidéo, en
            // en montrant quand même quelques-unes par seconde.
            if r.playing && Instant::now() > due + Duration::from_millis(60) && last_shown.elapsed() < Duration::from_millis(250) {
                continue;
            }
        }
        r.step = false;

        // L'image, réduite à la taille de l'aperçu puis redressée si la vidéo est tournée.
        let (w, h) = (decoded.width(), decoded.height());
        let key = (decoded.format(), w, h);
        if scaler.as_ref().is_none_or(|(k, _)| *k != key) {
            let (tw, th) = engine::fit(w, h, if info.rotation % 180 == 90 { (MAX.1, MAX.0) } else { MAX });
            scaler = Some((key, scaling::Context::get(key.0, w, h, Pixel::RGBA, tw, th, scaling::Flags::BILINEAR).map_err(bad)?));
            small = frame::Video::empty();
        }
        let Some((_, context)) = &mut scaler else { continue };
        context.run(&decoded, &mut small).map_err(bad)?;
        let mut image = (small.width(), small.height(), engine::packed_rows(&small, 4));
        for _ in 0..info.rotation / 90 {
            image = engine::quarter_turn(image.0, image.1, &image.2);
        }
        publish(shared, ctx, image, at);
        last_shown = Instant::now();
    }
}

// ---- un WebP animé -----------------------------------------------------------------------------

/// Rejoue l'animation à sa cadence, d'un bout à l'autre. La pause la retient ; il n'y a pas de saut.
fn play_webp(path: &Path, info: &MediaInfo, rx: &Receiver<Cmd>, shared: &Mutex<Shared>, ctx: &egui::Context) -> Result<(), String> {
    const CLOSED: &str = "fermé";
    let Some(webp) = info.webp else { return Ok(()) };
    let fps = webp.fps();
    let opaque = webp::pipe_format(&webp, true) != "rgba";
    loop {
        let (mut n, mut clock) = (0u64, None::<(Instant, u64)>);
        let played = webp::frames(path, fps, true, &AtomicBool::new(false), &mut |pixels| {
            // La pause : on attend ici l'ordre de reprendre.
            let mut paused = false;
            loop {
                let cmd = if paused { rx.recv().map_err(|_| CLOSED.to_owned())? } else {
                    match rx.try_recv() {
                        Ok(cmd) => cmd,
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return Err(CLOSED.to_owned()),
                    }
                };
                paused = matches!(cmd, Cmd::Pause) || (paused && matches!(cmd, Cmd::Seek(_)));
                shared.lock().unwrap().playing = !paused;
                clock = None;
            }
            let (wall, first) = *clock.get_or_insert((Instant::now(), n));
            if let Some(wait) = (wall + Duration::from_secs_f64((n - first) as f64 / fps)).checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            let rgba = if opaque { pixels.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect() } else { pixels.to_vec() };
            publish(shared, ctx, (webp.width, webp.height, rgba), n as f64 / fps);
            n += 1;
            Ok(())
        });
        match played {
            Err(why) if why == CLOSED => return Ok(()),
            Err(why) => return Err(why),
            Ok(_) => {}
        }
        // La fin : on attend qu'on demande de relire.
        let mut s = shared.lock().unwrap();
        (s.playing, s.ended, s.position) = (false, true, info.duration);
        drop(s);
        ctx.request_repaint();
        loop {
            match rx.recv() {
                Ok(Cmd::Play) => break,
                Ok(_) => {}
                Err(_) => return Ok(()),
            }
        }
        let mut s = shared.lock().unwrap();
        (s.playing, s.ended) = (true, false);
    }
}
