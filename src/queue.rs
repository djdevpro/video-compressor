//! La file des fichiers à traiter, et les fils qui travaillent pour elle : l'analyse de chaque
//! fichier ajouté, puis l'encodage, un fichier après l'autre.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Instant;

use crate::engine::{self, EncodeError, MediaInfo, VIDEO_EXTS};
use crate::settings::Encode;

/// La taille des vignettes : assez pour l'aperçu en grand de la vidéo choisie.
pub const THUMB: (u32, u32) = (640, 360);

pub enum State {
    /// Le moteur lit ses mesures.
    Probing,
    Ready,
    /// Dans un lot en cours, pas encore commencé.
    Queued,
    Running { fraction: f32, speed: f32, started: Instant },
    Done { output: PathBuf, size: u64 },
    Failed(String),
}

pub struct Item {
    pub id: u64,
    pub path: PathBuf,
    pub name: String,
    pub info: Option<MediaInfo>,
    pub thumb: Option<egui::TextureHandle>,
    pub state: State,
}

impl Item {
    pub fn new(id: u64, path: PathBuf) -> Self {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        Self { id, path, name, info: None, thumb: None, state: State::Probing }
    }

    /// Peut partir dans le prochain lot.
    pub fn encodable(&self) -> bool {
        self.info.is_some() && matches!(self.state, State::Ready | State::Failed(_))
    }
}

/// Ce que les fils de travail renvoient à l'interface.
pub enum Msg {
    Probed { id: u64, result: Result<MediaInfo, String>, thumb: Option<(u32, u32, Vec<u8>)> },
    Started { id: u64 },
    Progress { id: u64, fraction: f32, speed: f32 },
    Finished { id: u64, result: Result<(PathBuf, u64), EncodeError> },
    BatchDone,
}

/// Les fichiers d'une sélection : chaque fichier tel quel, et les vidéos d'un dossier déposé.
pub fn expand(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut inside: Vec<PathBuf> = std::fs::read_dir(&path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()).is_some_and(|e| VIDEO_EXTS.contains(&e.to_lowercase().as_str())))
                .collect();
            inside.sort();
            out.extend(inside);
        } else if path.is_file() {
            out.push(path);
        }
    }
    out
}

/// Le fichier à produire : à côté de l'original (ou dans `out_dir`), avec le suffixe et
/// l'extension du format. Jamais un fichier qui existe, ni un nom déjà pris dans ce lot.
pub fn output_path(input: &Path, enc: &Encode, out_dir: &str, suffix: &str, taken: &[PathBuf]) -> PathBuf {
    let dir = match out_dir.trim() {
        "" => input.parent().map(Path::to_path_buf).unwrap_or_default(),
        d => PathBuf::from(d),
    };
    let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "video".into());
    // Les caractères qu'un nom de fichier n'accepte pas sont retirés du suffixe.
    let suffix: String = suffix.trim().chars().filter(|c| !r#"\/:*?"<>|"#.contains(*c)).collect();
    let ext = enc.container.ext();
    let free = |p: &PathBuf| !p.exists() && !taken.contains(p) && p != input;
    let first = dir.join(format!("{stem}{suffix}.{ext}"));
    if free(&first) {
        return first;
    }
    (2..).map(|n| dir.join(format!("{stem}{suffix}-{n}.{ext}"))).find(free).unwrap_or(first)
}

/// Analyse un fichier dans un fil : ses mesures, puis sa vignette.
pub fn spawn_probe(id: u64, path: PathBuf, tx: Sender<Msg>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let result = engine::probe(&path);
        let thumb = result.as_ref().ok().and_then(|info| engine::thumbnail(&path, info, THUMB));
        let _ = tx.send(Msg::Probed { id, result, thumb });
        ctx.request_repaint();
    });
}

/// Un fichier d'un lot : d'où, vers où.
pub struct Job {
    pub id: u64,
    pub input: PathBuf,
    pub info: MediaInfo,
    pub output: PathBuf,
}

/// Le fichier en cours d'écriture : « film.partial.mp4 », renommé une fois complet.
fn partial_path(output: &Path) -> PathBuf {
    let stem = output.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = output.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    output.with_file_name(format!("{stem}.partial.{ext}"))
}

fn run_job(enc: &Encode, job: &Job, cancel: &AtomicBool, tx: &Sender<Msg>, ctx: &egui::Context) -> Result<(PathBuf, u64), EncodeError> {
    let failed = |e: String| EncodeError::Failed(e);
    let partial = partial_path(&job.output);
    if let Some(dir) = job.output.parent() {
        std::fs::create_dir_all(dir).map_err(|e| failed(e.to_string()))?;
    }
    let mut last = Instant::now();
    let result = engine::encode(enc, &job.info, &job.input, &partial, cancel, &mut |fraction, speed| {
        // Dix messages par seconde suffisent à une barre de progression.
        if last.elapsed().as_millis() >= 100 {
            last = Instant::now();
            let _ = tx.send(Msg::Progress { id: job.id, fraction, speed });
            ctx.request_repaint();
        }
    });
    if let Err(e) = result {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    let size = std::fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let _ = std::fs::remove_file(&partial);
        return Err(failed(crate::i18n::tr("Le fichier produit est vide.").to_owned()));
    }
    std::fs::rename(&partial, &job.output).map_err(|e| failed(e.to_string()))?;
    Ok((job.output.clone(), size))
}

/// Encode les fichiers d'un lot, l'un après l'autre, dans un fil. Renvoie de quoi l'arrêter.
pub fn spawn_batch(enc: Encode, jobs: Vec<Job>, tx: Sender<Msg>, ctx: egui::Context) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    std::thread::spawn(move || {
        for job in &jobs {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let _ = tx.send(Msg::Started { id: job.id });
            ctx.request_repaint();
            let result = run_job(&enc, job, &stop, &tx, &ctx);
            let _ = tx.send(Msg::Finished { id: job.id, result });
            ctx.request_repaint();
        }
        let _ = tx.send(Msg::BatchDone);
        ctx.request_repaint();
    });
    cancel
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Container;

    #[test]
    fn nom_du_fichier_produit() {
        let dir = std::env::temp_dir().join(format!("vc-noms-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("vacances.mov");
        std::fs::write(&input, b"x").unwrap();
        let enc = Encode::default();
        let first = output_path(&input, &enc, "", "-compresse", &[]);
        assert_eq!(first, dir.join("vacances-compresse.mp4"));
        assert_eq!(output_path(&input, &enc, "", "-compresse", std::slice::from_ref(&first)), dir.join("vacances-compresse-2.mp4"), "nom déjà pris dans le lot");
        std::fs::write(&first, b"x").unwrap();
        assert_eq!(output_path(&input, &enc, "", "-compresse", &[]), dir.join("vacances-compresse-2.mp4"), "un fichier existant n'est jamais écrasé");
        let same = Encode { container: Container::Mov, ..enc };
        assert_eq!(output_path(&input, &same, "", "", &[]), dir.join("vacances-2.mov"), "jamais l'original lui-même");
        assert_eq!(output_path(&input, &enc, "", " :mini?", &[]), dir.join("vacancesmini.mp4"), "suffixe nettoyé");
        let elsewhere = dir.join("sortie");
        assert_eq!(output_path(&input, &enc, &elsewhere.display().to_string(), "-c", &[]), elsewhere.join("vacances-c.mp4"));
        assert_eq!(partial_path(&first), dir.join("vacances-compresse.partial.mp4"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn un_dossier_depose_donne_ses_videos() {
        let dir = std::env::temp_dir().join(format!("vc-depot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["b.MP4", "a.webm", "notes.txt"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let found = expand(vec![dir.clone(), dir.join("notes.txt"), dir.join("absent.mp4")]);
        assert_eq!(found, [dir.join("a.webm"), dir.join("b.MP4"), dir.join("notes.txt")], "les vidéos du dossier, et le fichier choisi exprès");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
