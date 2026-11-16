//! Deux choses se décident à la compilation :
//! - sous Windows, l'icône est intégrée à l'exécutable (`assets/video-compressor.ico`) ;
//! - les bibliothèques dont FFmpeg dépend (x264, x265, libvpx, SVT-AV1, dav1d, Opus, LAME,
//!   libwebp, zlib) sont ajoutées à l'édition de liens.
//!
//! FFmpeg est lié statiquement dans l'exécutable. Ses bibliothèques sont compilées par vcpkg
//! (`vcpkg.json`, `scripts/ffmpeg-libs.sh`) et désignées par `FFMPEG_DIR`. La crate
//! `ffmpeg-sys-next` y trouve libavcodec et les autres, mais pas ce dont elles dépendent. Sous
//! Windows, on les nomme ici d'après le contenu du dossier ; ailleurs, `pkg-config` donne la liste
//! et son ordre.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=assets/video-compressor.rc");
    println!("cargo:rerun-if-changed=assets/video-compressor.ico");
    println!("cargo:rerun-if-env-changed=VIDEO_COMPRESSOR_VERSION");
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if windows {
        embed_resource::compile("assets/video-compressor.rc", embed_resource::NONE).manifest_optional().expect("icône Windows");
    }
    let Ok(dir) = std::env::var("FFMPEG_DIR") else { return };
    let lib = Path::new(&dir).join("lib");
    if !lib.is_dir() {
        panic!("Les bibliothèques de FFmpeg sont introuvables dans {}. Lance d'abord scripts/ffmpeg-libs.sh.", lib.display());
    }
    println!("cargo:rustc-link-search=native={}", lib.display());
    if windows { link_windows(&lib) } else { link_unix(&lib) }
}

/// Les bibliothèques de FFmpeg lui-même, déjà liées par `ffmpeg-sys-next`.
const FFMPEG: [&str; 7] = ["avcodec", "avdevice", "avfilter", "avformat", "avutil", "swresample", "swscale"];

/// Les bibliothèques de Windows dont FFmpeg et ses encodeurs se servent.
const SYSTEM: [&str; 13] = ["bcrypt", "ole32", "oleaut32", "user32", "gdi32", "secur32", "ws2_32", "psapi", "shlwapi", "strmiids", "mfuuid", "uuid", "advapi32"];

/// Windows : toutes les bibliothèques statiques du dossier, sauf celles de FFmpeg lui-même.
fn link_windows(lib: &Path) {
    let Ok(entries) = std::fs::read_dir(lib) else { return };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lib")))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .filter(|name| !FFMPEG.contains(&name.as_str()))
        .collect();
    names.sort();
    for name in names {
        println!("cargo:rustc-link-lib=static={name}");
    }
    for name in SYSTEM {
        println!("cargo:rustc-link-lib=dylib={name}");
    }
}

/// Linux et macOS : `pkg-config` lit les fichiers `.pc` posés par vcpkg et donne, dans l'ordre que
/// l'éditeur de liens attend, FFmpeg et tout ce dont il dépend.
#[cfg(unix)]
fn link_unix(lib: &Path) {
    let mut paths = vec![lib.join("pkgconfig")];
    paths.extend(std::env::var_os("PKG_CONFIG_PATH").iter().flat_map(std::env::split_paths));
    // SAFETY : un script de compilation n'a qu'un fil d'exécution.
    unsafe { std::env::set_var("PKG_CONFIG_PATH", std::env::join_paths(paths).expect("PKG_CONFIG_PATH")) };
    for name in ["libavfilter", "libavformat", "libavcodec", "libswscale", "libswresample", "libavutil"] {
        pkg_config::Config::new().statik(true).probe(name).unwrap_or_else(|e| panic!("pkg-config ne trouve pas {name} : {e}"));
    }
}

#[cfg(not(unix))]
fn link_unix(_lib: &Path) {}
