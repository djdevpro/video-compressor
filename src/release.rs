//! Les versions de Video Compressor, publiées comme releases GitHub : vérifier, télécharger, installer.
//!
//! Même principe que Rust Project :
//! - la release est la source de vérité (`releases/latest`, ou la liste pour le canal bêta),
//!   interrogée avec son ETag pour ne pas consommer le quota de l'API ;
//! - ses notes sont les nouveautés affichées dans la fenêtre de mise à jour ;
//! - chaque fichier a son empreinte SHA-256 (champ `digest` de GitHub, sinon `SHA256SUMS.txt`),
//!   vérifiée avant l'installation ;
//! - l'installation renomme l'exécutable en cours (`video-compressor.exe.old`), met le nouveau à sa
//!   place, puis l'application redémarre ; l'ancien est effacé au lancement suivant.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::Value;

use crate::i18n::{tf, tr};
use crate::net::{self, Api};

/// Le dépôt des releases.
pub const REPO: &str = "djdevpro/video-compressor";

/// La version de cet exécutable : celle du tag de release (posée par la CI au moment de la
/// compilation), sinon celle de Cargo.toml (compilation locale).
pub const VERSION: &str = match option_env!("VIDEO_COMPRESSOR_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// La fin du nom de fichier publié pour cette plateforme.
pub const PLATFORM: &str = if cfg!(windows) {
    "windows-x64.exe"
} else if cfg!(target_os = "macos") {
    // Le binaire seul (Apple Silicon et Intel) : il remplace celui de `Video Compressor.app`.
    "macos-universal"
} else {
    "linux-x64"
};

/// Passé à l'application qui redémarre après une mise à jour.
pub const AFTER_UPDATE: &str = "--apres-mise-a-jour";

/// Le nom du fichier d'une version pour cette plateforme, tel que la CI le publie.
pub fn asset_name(version: &str) -> String {
    format!("video-compressor-{version}-{PLATFORM}")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Empreinte SHA-256 en hexadécimal, si GitHub la donne.
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    /// Les points de version (lignes « - … » des notes de la release).
    pub notes: Vec<String>,
    /// Date de publication, en chiffres (« 04/11/2026 »).
    pub published: String,
    pub page: String,
    pub prerelease: bool,
    pub asset: Option<Asset>,
    /// `SHA256SUMS.txt`, si la release en a un (empreintes de secours).
    pub sums: Option<String>,
}

/// Où en est la mise à jour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Pas encore vérifié.
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Ready,
    Failed,
}

// ---- versions ----------------------------------------------------------------------------------

/// `1.2.3`, `v1.2.3` ou `1.2.3-beta.1` : (numéros, pré-version éventuelle).
fn parse_version(v: &str) -> Option<([u64; 3], Option<String>)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p.to_owned())),
        None => (v, None),
    };
    let mut it = core.split('.').map(|x| x.parse::<u64>().ok());
    let n = [it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)];
    Some((n, pre))
}

/// `remote` est plus récente que `local` (une pré-version passe avant la version finale).
pub fn newer(remote: &str, local: &str) -> bool {
    let (Some((a, pa)), Some((b, pb))) = (parse_version(remote), parse_version(local)) else { return false };
    match a.cmp(&b) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (pa, pb) {
            (None, Some(_)) => true,
            (Some(x), Some(y)) => x > y,
            _ => false,
        },
    }
}

// ---- lecture d'une release ---------------------------------------------------------------------

/// Les points de version : les lignes en liste des notes, sans liens ni mentions d'auteur.
pub fn notes_points(body: &str) -> Vec<String> {
    let clean = |line: &str| {
        let mut s = line.to_owned();
        // [texte](url) → texte
        while let (Some(a), Some(b)) = (s.find('['), s.find("](")) {
            let Some(c) = s[b..].find(')').map(|c| b + c) else { break };
            if a >= b {
                break;
            }
            s = format!("{}{}{}", &s[..a], &s[a + 1..b], &s[c + 1..]);
        }
        // Notes générées par GitHub : « … by @nom in https://… »
        if let Some(i) = s.find(" by @") {
            s.truncate(i);
        }
        s.trim().trim_end_matches('.').trim().to_owned()
    };
    let bullets: Vec<String> = body
        .lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")))
        .map(clean)
        .filter(|l| !l.is_empty() && !l.contains("Full Changelog"))
        .collect();
    if !bullets.is_empty() {
        return bullets;
    }
    body.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#') && !l.contains("Full Changelog")).map(clean).filter(|l| !l.is_empty()).collect()
}

/// `2026-11-04T10:00:00Z` → `04/11/2026`.
fn short_date(iso: &str) -> String {
    let date = iso.split('T').next().unwrap_or("");
    let mut it = date.split('-');
    match (it.next(), it.next(), it.next()) {
        (Some(y), Some(m), Some(d)) if y.len() == 4 && m.len() == 2 && d.len() == 2 => format!("{d}/{m}/{y}"),
        _ => String::new(),
    }
}

/// Une release de l'API GitHub. `None` pour un brouillon ou une réponse sans tag.
pub fn parse_release(v: &Value) -> Option<Release> {
    if v["draft"].as_bool().unwrap_or(false) {
        return None;
    }
    let tag = v["tag_name"].as_str()?;
    let version = tag.trim_start_matches('v').to_owned();
    parse_version(&version)?;
    let wanted = asset_name(&version);
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let asset = assets.iter().find(|a| a["name"].as_str() == Some(wanted.as_str())).or_else(|| assets.iter().find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(&format!("-{PLATFORM}"))))).map(|a| Asset {
        name: a["name"].as_str().unwrap_or_default().to_owned(),
        url: a["browser_download_url"].as_str().unwrap_or_default().to_owned(),
        size: a["size"].as_u64().unwrap_or(0),
        sha256: a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_lowercase),
    });
    let sums = assets.iter().find(|a| a["name"].as_str() == Some("SHA256SUMS.txt")).and_then(|a| a["browser_download_url"].as_str()).map(str::to_owned);
    Some(Release {
        notes: notes_points(v["body"].as_str().unwrap_or("")),
        published: short_date(v["published_at"].as_str().unwrap_or("")),
        page: v["html_url"].as_str().unwrap_or_default().to_owned(),
        prerelease: v["prerelease"].as_bool().unwrap_or(false),
        version,
        asset,
        sums,
    })
}

// ---- réseau ------------------------------------------------------------------------------------

enum Fetch {
    NotModified,
    Latest(Option<Box<Release>>, Option<String>),
}

fn fetch(api: &str, beta: bool, etag: Option<&str>) -> Result<Fetch, String> {
    let url = if beta { format!("{api}/repos/{REPO}/releases?per_page=10") } else { format!("{api}/repos/{REPO}/releases/latest") };
    Ok(match net::github(&url, etag)? {
        Api::NotModified => Fetch::NotModified,
        // Aucune release publiée pour l'instant.
        Api::Missing => Fetch::Latest(None, None),
        Api::Json(body, etag) => {
            let release = if beta { body.as_array().and_then(|list| list.iter().find_map(parse_release)) } else { parse_release(&body) };
            Fetch::Latest(release.map(Box::new), etag)
        }
    })
}

/// Télécharge le fichier de la release, vérifie son empreinte, et le range dans `dir`.
fn download(release: &Release, dir: &Path, progress: impl FnMut(u64)) -> Result<PathBuf, String> {
    let asset = release.asset.as_ref().ok_or_else(|| tr("Cette version n'a pas de fichier pour cette plateforme.").to_owned())?;
    let expected = match (&asset.sha256, &release.sums) {
        (Some(h), _) => Some(h.clone()),
        (None, Some(url)) => net::sum_for(&net::text(url)?, &asset.name),
        _ => None,
    };
    let expected = expected.ok_or_else(|| tr("Aucune empreinte SHA-256 n'est publiée pour ce fichier, installation refusée.").to_owned())?;
    net::download(&asset.url, dir, &asset.name, &expected, &AtomicBool::new(false), progress)
}

// ---- installation ------------------------------------------------------------------------------

/// Un exécutable compilé sur place (`target/debug`, `target/release`) : on ne le remplace pas.
pub fn is_dev_build() -> bool {
    std::env::current_exe().map(|p| p.to_string_lossy().replace('\\', "/").to_lowercase()).is_ok_and(|p| p.contains("/target/debug/") || p.contains("/target/release/"))
}

fn old_exe(exe: &Path) -> PathBuf {
    PathBuf::from(format!("{}.old", exe.display()))
}

/// Les anciens exécutables laissés par les mises à jour, à côté de celui qui tourne.
fn old_exes(exe: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name().and_then(|n| n.to_str())) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.len() > name.len() && n.starts_with(name) && n.ends_with(".old")))
        .collect()
}

/// Met `new_exe` à la place de `exe`. Un exécutable qui tourne ne s'efface pas sous Windows, mais
/// il se renomme : l'ancien devient `<exe>.old`, effacé au lancement suivant.
pub fn replace_exe(exe: &Path, new_exe: &Path) -> Result<(), String> {
    let mut old = old_exe(exe);
    // L'ancien peut servir encore (une autre fenêtre ouverte avant la mise à jour précédente).
    if old.exists() && std::fs::remove_file(&old).is_err() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        old = PathBuf::from(format!("{}.{stamp}.old", exe.display()));
    }
    if exe.exists() {
        std::fs::rename(exe, &old).map_err(|e| tf("Impossible de remplacer {0} ({1})", &[("0", &exe.display()), ("1", &e)]))?;
    }
    if let Err(e) = std::fs::copy(new_exe, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(tf("Installation impossible ({0})", &[("0", &e)]));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(exe, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

/// Met le nouvel exécutable à la place de celui qui tourne (il reste utilisable jusqu'à la fin).
pub fn install(new_exe: &Path) -> Result<(), String> {
    if is_dev_build() {
        return Err(tr("Version de développement. Les mises à jour s'installent sur l'exécutable d'une release.").to_owned());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    replace_exe(&exe, new_exe)
}

/// Relance l'application (le même exécutable), pour appliquer une mise à jour.
pub fn restart() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe).arg(AFTER_UPDATE).spawn().map(|_| ()).map_err(|e| tf("Redémarrage impossible ({0})", &[("0", &e)]))
}

/// Au démarrage : efface les anciens exécutables, dès que les processus qui tournaient depuis
/// eux les ont lâchés.
pub fn cleanup_old_exes() {
    if is_dev_build() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    std::thread::spawn(move || {
        for _ in 0..20 {
            if old_exes(&exe).into_iter().filter(|p| std::fs::remove_file(p).is_err()).count() == 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

// ---- l'état vu par l'interface -----------------------------------------------------------------

enum Msg {
    Checked(Result<Fetch, String>),
    Progress(u64),
    Downloaded(Result<PathBuf, String>),
}

/// La mise à jour : phase, release trouvée, progression. Le travail réseau tourne dans un fil.
pub struct Updater {
    pub phase: Phase,
    pub release: Option<Release>,
    /// (reçu, total) en octets.
    pub progress: (u64, u64),
    pub error: Option<String>,
    pub downloaded: Option<PathBuf>,
    /// L'adresse de l'API (un faux serveur dans les tests).
    pub api: String,
    etag: Option<String>,
    cached: Option<Release>,
    rx: Option<Receiver<Msg>>,
}

impl Default for Updater {
    fn default() -> Self {
        Self { phase: Phase::Idle, release: None, progress: (0, 0), error: None, downloaded: None, api: "https://api.github.com".into(), etag: None, cached: None, rx: None }
    }
}

impl Updater {
    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Checking | Phase::Downloading)
    }

    pub fn check(&mut self, ctx: &egui::Context, beta: bool) {
        if self.busy() {
            return;
        }
        self.phase = Phase::Checking;
        self.error = None;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let (api, etag, ctx) = (self.api.clone(), self.etag.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Checked(fetch(&api, beta, etag.as_deref())));
            ctx.request_repaint();
        });
    }

    pub fn download(&mut self, ctx: &egui::Context, dir: PathBuf) {
        let Some(release) = self.release.clone().filter(|_| self.phase == Phase::Available || self.phase == Phase::Failed) else { return };
        self.phase = Phase::Downloading;
        self.error = None;
        let total = release.asset.as_ref().map(|a| a.size).unwrap_or(0);
        self.progress = (0, total);
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut last = 0u64;
            let result = download(&release, &dir, |done| {
                // Pas plus d'un message par 256 Ko : l'interface suit sans être inondée.
                if done - last >= 256 * 1024 || done == total {
                    last = done;
                    let _ = tx.send(Msg::Progress(done));
                    ctx.request_repaint();
                }
            });
            let _ = tx.send(Msg::Downloaded(result));
            ctx.request_repaint();
        });
    }

    /// Une release trouvée : disponible si plus récente que cette version.
    pub fn apply(&mut self, release: Option<Release>) {
        self.phase = match &release {
            Some(r) if newer(&r.version, VERSION) => Phase::Available,
            _ => Phase::UpToDate,
        };
        self.release = release;
    }

    /// Relève les messages du fil de travail. Renvoie `true` si quelque chose a changé.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let msgs: Vec<Msg> = rx.try_iter().collect();
        let changed = !msgs.is_empty();
        for msg in msgs {
            match msg {
                Msg::Checked(Ok(Fetch::Latest(release, etag))) => {
                    self.etag = etag;
                    self.cached = release.as_deref().cloned();
                    self.apply(release.map(|r| *r));
                }
                Msg::Checked(Ok(Fetch::NotModified)) => {
                    let cached = self.cached.clone();
                    self.apply(cached);
                }
                Msg::Checked(Err(e)) | Msg::Downloaded(Err(e)) => {
                    self.phase = Phase::Failed;
                    self.error = Some(e);
                }
                Msg::Progress(done) => self.progress.0 = done,
                Msg::Downloaded(Ok(path)) => {
                    self.downloaded = Some(path);
                    self.phase = Phase::Ready;
                }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    fn release_json(tag: &str) -> Value {
        json!({
            "tag_name": tag, "name": format!("Video Compressor {}", tag.trim_start_matches('v')), "draft": false, "prerelease": false,
            "published_at": "2026-11-04T10:00:00Z", "html_url": format!("https://github.com/{REPO}/releases/tag/{tag}"),
            "body": "- Conversion des WebP animés\n- Clic droit sous Linux\n\n**Full Changelog**: https://github.com/x/y/compare/v0.1.0...v0.2.0",
            "assets": [
                { "name": asset_name(tag.trim_start_matches('v')), "size": 19_300_000, "browser_download_url": "https://example.invalid/video-compressor.exe", "digest": "sha256:ABCDEF" },
                { "name": "SHA256SUMS.txt", "size": 120, "browser_download_url": "https://example.invalid/SHA256SUMS.txt" }
            ]
        })
    }

    #[test]
    fn comparer_les_versions() {
        assert!(newer("0.2.0", "0.1.9") && newer("v1.0.0", "0.9.9") && newer("0.1.10", "0.1.9"));
        assert!(!newer("0.1.0", "0.1.0") && !newer("0.1.0", "0.2.0"));
        assert!(newer("0.2.0", "0.2.0-beta.1"), "la version finale passe après la bêta");
        assert!(!newer("0.2.0-beta.1", "0.2.0"));
        assert!(!newer("pas-une-version", "0.1.0"));
    }

    #[test]
    fn lire_une_release_github() {
        let r = parse_release(&release_json("v0.2.0")).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.notes, ["Conversion des WebP animés", "Clic droit sous Linux"]);
        assert_eq!(r.published, "04/11/2026");
        let a = r.asset.unwrap();
        assert_eq!((a.name.clone(), a.size, a.sha256.as_deref()), (asset_name("0.2.0"), 19_300_000, Some("abcdef")));
        assert!(r.sums.is_some());
        assert!(parse_release(&json!({ "tag_name": "v0.3.0", "draft": true })).is_none(), "un brouillon n'est pas une version");
    }

    #[test]
    fn notes_generees_par_github() {
        let body = "## What's Changed\n* Real updater by @djdevpro in https://github.com/x/y/pull/3\n* [Docs](https://x.y) refreshed\n\n**Full Changelog**: https://x";
        assert_eq!(notes_points(body), ["Real updater", "Docs refreshed"]);
        assert_eq!(notes_points("Une seule phrase sans liste."), ["Une seule phrase sans liste"]);
    }

    #[test]
    fn disponible_seulement_si_plus_recente() {
        let mut u = Updater::default();
        let mut r = parse_release(&release_json("v0.2.0")).unwrap();
        r.version = "999.0.0".into();
        u.apply(Some(r.clone()));
        assert_eq!(u.phase, Phase::Available);
        r.version = VERSION.into();
        u.apply(Some(r));
        assert_eq!(u.phase, Phase::UpToDate);
        u.apply(None);
        assert_eq!(u.phase, Phase::UpToDate, "aucune release publiée");
    }

    #[test]
    fn remplacer_un_executable() {
        let dir = std::env::temp_dir().join(format!("vc-maj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (exe, new) = (dir.join("app.exe"), dir.join("telecharge.exe"));
        std::fs::write(&exe, b"ancien").unwrap();
        std::fs::write(&new, b"nouveau").unwrap();
        replace_exe(&exe, &new).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"nouveau");
        assert_eq!(std::fs::read(old_exe(&exe)).unwrap(), b"ancien", "l'ancien est gardé jusqu'au prochain lancement");
        assert_eq!(old_exes(&exe), [old_exe(&exe)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Un faux serveur GitHub : la release, puis le fichier ; l'empreinte est vérifiée.
    #[test]
    fn verifier_puis_telecharger_depuis_un_faux_github() {
        let payload = b"MZ faux executable".to_vec();
        let digest = format!("{:x}", Sha256::digest(&payload));
        let served = payload.clone();
        let base = std::sync::Arc::new(std::sync::OnceLock::<String>::new());
        let known = base.clone();
        let addr = crate::net::tests::serve(2, move |first| {
            if first.contains("/fichier") {
                return ("application/octet-stream", served.clone());
            }
            let release = json!({
                "tag_name": "v999.0.0", "draft": false, "prerelease": false, "published_at": "2026-11-04T10:00:00Z",
                "html_url": "https://example.invalid", "body": "- Une nouveauté",
                "assets": [{ "name": asset_name("999.0.0"), "size": served.len(), "browser_download_url": format!("{}/fichier", known.get().unwrap()), "digest": format!("sha256:{digest}") }]
            });
            ("application/json", release.to_string().into_bytes())
        });
        base.set(addr.clone()).unwrap();
        let ctx = egui::Context::default();
        let mut u = Updater { api: addr, ..Default::default() };
        u.check(&ctx, false);
        let wait = |u: &mut Updater, until: Phase| {
            for _ in 0..300 {
                u.poll();
                if u.phase == until || u.phase == Phase::Failed {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        wait(&mut u, Phase::Available);
        assert_eq!(u.phase, Phase::Available, "{:?}", u.error);
        assert_eq!(u.release.as_ref().unwrap().notes, ["Une nouveauté"]);
        let dir = std::env::temp_dir().join(format!("vc-release-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        u.download(&ctx, dir.clone());
        wait(&mut u, Phase::Ready);
        assert_eq!(u.phase, Phase::Ready, "{:?}", u.error);
        assert_eq!(std::fs::read(u.downloaded.as_ref().unwrap()).unwrap(), payload, "fichier complet et vérifié");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
