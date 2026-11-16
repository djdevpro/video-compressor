//! Les WebP animés. FFmpeg ne sait pas les décoder : on lit les images ici (`image-webp`) et on
//! les donne brutes au moteur, à cadence fixe.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use image_webp::WebPDecoder;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WebpInfo {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    /// La durée d'une boucle, en millisecondes.
    pub duration_ms: u64,
    pub has_alpha: bool,
}

/// Le navigateur joue une image sans durée pendant 100 ms : on fait pareil.
const DEFAULT_DELAY_MS: u64 = 100;

fn open(path: &Path) -> Option<WebPDecoder<BufReader<File>>> {
    WebPDecoder::new(BufReader::new(File::open(path).ok()?)).ok()
}

/// Les mesures d'un WebP animé. `None` pour un autre fichier, ou un WebP fixe (FFmpeg le lit).
pub fn inspect(path: &Path) -> Option<WebpInfo> {
    let d = open(path)?;
    if !d.is_animated() || d.num_frames() == 0 {
        return None;
    }
    let (width, height) = d.dimensions();
    let frames = d.num_frames();
    let duration_ms = match d.loop_duration() {
        0 => u64::from(frames) * DEFAULT_DELAY_MS,
        ms => ms,
    };
    Some(WebpInfo { width, height, frames, duration_ms, has_alpha: d.has_alpha() })
}

impl WebpInfo {
    /// La cadence moyenne de l'animation, entre 1 et 60 images par seconde.
    pub fn fps(&self) -> f64 {
        (f64::from(self.frames) * 1000.0 / self.duration_ms.max(1) as f64).round().clamp(1.0, 60.0)
    }

    pub fn duration(&self) -> f64 {
        self.duration_ms as f64 / 1000.0
    }
}

/// La première image, en RGBA.
pub fn first_frame(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let mut d = open(path)?;
    let (w, h) = d.dimensions();
    let mut buf = vec![0u8; d.output_buffer_size()?];
    if d.is_animated() {
        d.read_frame(&mut buf).ok()?;
    } else {
        d.read_image(&mut buf).ok()?;
    }
    let rgba = if d.has_alpha() { buf } else { buf.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect() };
    Some((w, h, rgba))
}

/// Le format des images passées à FFmpeg : `rgba` si on garde la transparence, sinon `rgb24`.
pub fn pipe_format(info: &WebpInfo, keep_alpha: bool) -> &'static str {
    if info.has_alpha && keep_alpha { "rgba" } else { "rgb24" }
}

/// Pose une image RGBA sur un fond blanc (une vidéo n'a pas de transparence).
fn flatten(rgba: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.extend(rgba.as_chunks::<4>().0.iter().flat_map(|p| {
        let a = u32::from(p[3]);
        let over = |c: u8| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8;
        [over(p[0]), over(p[1]), over(p[2])]
    }));
}

/// Rejoue l'animation à `fps` images par seconde : chaque image est donnée à `emit` autant de fois
/// que sa durée le demande (ou sautée si elle est plus courte qu'un pas). Les pixels sont en RGBA
/// ou en RGB selon `pipe_format`. S'arrête dès que `cancel` passe à vrai. Renvoie le nombre
/// d'images données.
pub fn frames(path: &Path, fps: f64, keep_alpha: bool, cancel: &AtomicBool, emit: &mut dyn FnMut(&[u8]) -> Result<(), String>) -> Result<u64, String> {
    let bad = |e: &dyn std::fmt::Display| e.to_string();
    let mut d = open(path).ok_or_else(|| bad(&"WebP illisible"))?;
    let info = WebpInfo { width: d.dimensions().0, height: d.dimensions().1, frames: d.num_frames(), duration_ms: d.loop_duration(), has_alpha: d.has_alpha() };
    let mut buf = vec![0u8; d.output_buffer_size().ok_or_else(|| bad(&"image trop grande"))?];
    let raw_rgba = pipe_format(&info, keep_alpha) == "rgba";
    let mut flat = Vec::new();
    let tick_ms = 1000.0 / fps.max(1.0);
    let (mut end_ms, mut written) = (0.0f64, 0u64);
    for i in 0..info.frames {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let delay = d.read_frame(&mut buf).map_err(|e| bad(&e))?;
        end_ms += if delay == 0 { DEFAULT_DELAY_MS as f64 } else { f64::from(delay) };
        let frame: &[u8] = if raw_rgba || !info.has_alpha {
            &buf
        } else {
            flatten(&buf, &mut flat);
            &flat
        };
        // Les instants de sortie qui tombent pendant cette image ; la dernière image est écrite au
        // moins une fois, pour ne jamais produire une vidéo vide.
        let last = i + 1 == info.frames;
        while (written as f64) * tick_ms < end_ms - 0.001 || (last && written == 0) {
            emit(frame)?;
            written += 1;
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparence_posee_sur_du_blanc() {
        let mut out = Vec::new();
        flatten(&[255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 128], &mut out);
        assert_eq!(out[..6], [255, 0, 0, 255, 255, 255], "opaque inchangé, transparent devenu blanc");
        assert!(out[6] > 120 && out[6] < 135, "à moitié transparent : gris moyen ({})", out[6]);
    }

    #[test]
    fn cadence_moyenne() {
        let info = WebpInfo { width: 10, height: 10, frames: 50, duration_ms: 2000, has_alpha: false };
        assert_eq!(info.fps(), 25.0);
        assert_eq!(WebpInfo { frames: 3, duration_ms: 30_000, ..info }.fps(), 1.0, "jamais sous une image par seconde");
        assert_eq!(pipe_format(&WebpInfo { has_alpha: true, ..info }, false), "rgb24");
        assert_eq!(pipe_format(&WebpInfo { has_alpha: true, ..info }, true), "rgba");
    }

    #[test]
    fn un_autre_fichier_n_est_pas_un_webp_anime() {
        assert_eq!(inspect(Path::new("Cargo.toml")), None);
        assert_eq!(inspect(Path::new("un-fichier-absent.webp")), None);
    }
}
