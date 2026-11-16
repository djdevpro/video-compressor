//! Le réseau : interroger l'API de GitHub et télécharger un fichier en vérifiant son empreinte.
//! Sert aux mises à jour (`release.rs`) et à l'installation de FFmpeg (`ffmpeg_setup.rs`).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::i18n::{tf, tr};

pub const USER_AGENT: &str = concat!("video-compressor/", env!("CARGO_PKG_VERSION"));

pub fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).build().into()
}

/// La réponse d'une requête à l'API de GitHub.
pub enum Api {
    /// Rien de neuf depuis l'ETag donné.
    NotModified,
    /// Pas de ressource à cette adresse (aucune release publiée).
    Missing,
    Json(Value, Option<String>),
}

/// Interroge l'API de GitHub. Avec un ETag, une réponse inchangée ne consomme pas de quota.
pub fn github(url: &str, etag: Option<&str>) -> Result<Api, String> {
    let mut req = agent(Duration::from_secs(15)).get(url).header("User-Agent", USER_AGENT).header("Accept", "application/vnd.github+json");
    if let Some(e) = etag {
        req = req.header("If-None-Match", e);
    }
    let mut resp = req.call().map_err(|e| tf("GitHub est injoignable ({0})", &[("0", &e)]))?;
    let etag = resp.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_owned);
    match resp.status().as_u16() {
        304 => Ok(Api::NotModified),
        404 => Ok(Api::Missing),
        200 => {
            let text = resp.body_mut().with_config().limit(16 * 1024 * 1024).read_to_string().map_err(|e| e.to_string())?;
            serde_json::from_str(&text).map(|v| Api::Json(v, etag)).map_err(|e| tf("Réponse illisible ({0})", &[("0", &e)]))
        }
        403 | 429 => Err(tr("GitHub limite les vérifications pour l'instant, réessaie plus tard.").to_owned()),
        s => Err(tf("GitHub a répondu {0}", &[("0", &s)])),
    }
}

/// Un petit fichier texte (`SHA256SUMS.txt`).
pub fn text(url: &str) -> Result<String, String> {
    let mut resp = agent(Duration::from_secs(30)).get(url).header("User-Agent", USER_AGENT).call().map_err(|e| e.to_string())?;
    resp.body_mut().with_config().limit(4 * 1024 * 1024).read_to_string().map_err(|e| e.to_string())
}

/// L'empreinte d'un fichier dans un `SHA256SUMS.txt` (« empreinte  nom »).
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let (hash, file) = (it.next()?, it.next()?);
        (file.trim_start_matches('*') == name).then(|| hash.to_lowercase())
    })
}

/// Télécharge `url` dans `dir/name`, en vérifiant son empreinte SHA-256. Un fichier dont l'empreinte
/// ne correspond pas est effacé. `progress` reçoit les octets reçus.
pub fn download(url: &str, dir: &Path, name: &str, sha256: &str, cancel: &AtomicBool, mut progress: impl FnMut(u64)) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let dest = dir.join(name);
    let part = dir.join(format!("{name}.part"));
    let mut resp = agent(Duration::from_secs(3600)).get(url).header("User-Agent", USER_AGENT).call().map_err(|e| tf("Téléchargement impossible ({0})", &[("0", &e)]))?;
    if resp.status().as_u16() != 200 {
        return Err(tf("Téléchargement impossible, le serveur a répondu {0}", &[("0", &resp.status().as_u16())]));
    }
    let mut reader = resp.body_mut().with_config().limit(4 * 1024 * 1024 * 1024).reader();
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&part);
            return Err(tr("Téléchargement annulé").to_owned());
        }
        let n = reader.read(&mut buf).map_err(|e| tf("Téléchargement interrompu ({0})", &[("0", &e)]))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        hasher.update(&buf[..n]);
        done += n as u64;
        progress(done);
    }
    drop(file);
    if format!("{:x}", hasher.finalize()) != sha256.to_lowercase() {
        let _ = std::fs::remove_file(&part);
        return Err(tr("L'empreinte du fichier ne correspond pas, installation refusée.").to_owned());
    }
    let _ = std::fs::remove_file(&dest);
    std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Un faux serveur : répond à `count` requêtes avec ce que `route` renvoie pour la première
    /// ligne de la requête (type de contenu, corps). Renvoie son adresse.
    pub fn serve(count: usize, route: impl Fn(&str) -> (&'static str, Vec<u8>) + Send + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().take(count).flatten() {
                let mut stream = stream;
                let mut req = [0u8; 4096];
                let n = stream.read(&mut req).unwrap_or(0);
                let first = String::from_utf8_lossy(&req[..n]).lines().next().unwrap_or_default().to_owned();
                let (ctype, body) = route(&first);
                let head = format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nETag: \"e1\"\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        base
    }

    #[test]
    fn empreintes_de_secours() {
        let sums = "abc123  video-compressor-0.2.0-windows-x64.exe\nfff  *autre.zip\n";
        assert_eq!(sum_for(sums, "video-compressor-0.2.0-windows-x64.exe").as_deref(), Some("abc123"));
        assert_eq!(sum_for(sums, "autre.zip").as_deref(), Some("fff"));
        assert_eq!(sum_for(sums, "absent.exe"), None);
    }

    #[test]
    fn telecharger_et_verifier() {
        let payload = b"un fichier a verifier".to_vec();
        let digest = format!("{:x}", Sha256::digest(&payload));
        let served = payload.clone();
        let base = serve(2, move |_| ("application/octet-stream", served.clone()));
        let dir = std::env::temp_dir().join(format!("vc-net-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cancel = AtomicBool::new(false);
        let mut seen = 0;
        let path = download(&format!("{base}/a"), &dir, "a.bin", &digest, &cancel, |n| seen = n).unwrap();
        assert_eq!((std::fs::read(&path).unwrap(), seen), (payload.clone(), payload.len() as u64));
        let err = download(&format!("{base}/a"), &dir, "b.bin", "00", &cancel, |_| {}).unwrap_err();
        assert!(err.contains("empreinte") && !dir.join("b.bin").exists() && !dir.join("b.bin.part").exists(), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
