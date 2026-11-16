#!/bin/bash
# Compile les bibliothèques statiques de FFmpeg avec vcpkg (voir vcpkg.json) et les range là où
# cargo les attend : vcpkg_installed/current, ou vcpkg_installed/<nom> si un nom est donné.
#
# À lancer une fois avant le premier `cargo build`, puis à chaque changement de vcpkg.json.
# Compter dix à vingt minutes la première fois ; vcpkg garde ensuite son travail en cache.
#
# Usage : scripts/ffmpeg-libs.sh [triplet] [nom]
#   scripts/ffmpeg-libs.sh                      la machine courante
#   scripts/ffmpeg-libs.sh arm64-osx-release arm64   une architecture précise (binaire universel de macOS)
#
# Il faut : git, un compilateur C et C++ (Visual Studio sous Windows), et sous Linux ou macOS
# nasm, pkg-config, autoconf, automake et libtool.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
triplet="${1:-}"
name="${2:-current}"
windows=false
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) windows=true ;;
esac

if [ -z "$triplet" ]; then
  case "$(uname -s)-$(uname -m)" in
    # Windows : tout en statique, runtime C++ compris, pour un exécutable sans aucune DLL à fournir.
    MINGW* | MSYS* | CYGWIN*) triplet="x64-windows-static-release" ;;
    Darwin-arm64) triplet="arm64-osx-release" ;;
    Darwin-x86_64) triplet="x64-osx-release" ;;
    Linux-x86_64) triplet="x64-linux-release" ;;
    Linux-aarch64) triplet="arm64-linux" ;;
    *) echo "système non prévu : $(uname -s) $(uname -m)" && exit 1 ;;
  esac
fi

# Le dossier de travail de vcpkg, hors du projet et sans espace dans son chemin : les scripts de
# compilation de FFmpeg et de x264 ne les supportent pas.
if $windows; then
  build="${VIDEO_COMPRESSOR_BUILD:-$(cygpath -u "$LOCALAPPDATA")/video-compressor-build}"
else
  build="${VIDEO_COMPRESSOR_BUILD:-$HOME/.cache/video-compressor-build}"
fi
mkdir -p "$build"
cp "$root/vcpkg.json" "$build/vcpkg.json"

# vcpkg : celui de VCPKG_ROOT, du PATH ou de Visual Studio ; à défaut, on le récupère.
vcpkg=""
for candidate in "${VCPKG_ROOT:-}/vcpkg" "${VCPKG_INSTALLATION_ROOT:-}/vcpkg" "$(command -v vcpkg || true)" \
  "/c/Program Files/Microsoft Visual Studio/2022/Community/VC/vcpkg/vcpkg.exe" \
  "/c/Program Files/Microsoft Visual Studio/2022/Professional/VC/vcpkg/vcpkg.exe" \
  "/c/Program Files/Microsoft Visual Studio/2022/Enterprise/VC/vcpkg/vcpkg.exe" \
  "/c/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/VC/vcpkg/vcpkg.exe"; do
  for exe in "$candidate" "$candidate.exe"; do
    if [ -z "$vcpkg" ] && [ -n "$candidate" ] && [ -f "$exe" ]; then vcpkg="$exe"; fi
  done
done

# vcpkg.json fixe la version des ports par un commit du dépôt de vcpkg. Un vcpkg installé comme
# un clone git (celui des machines de GitHub) lit sa base de versions dans ses propres fichiers :
# s'il n'est pas exactement à ce commit, il ne connaît pas ces versions, et on se passe de lui.
# (Celui de Visual Studio n'est pas un clone : il va chercher le commit lui-même.)
# VIDEO_COMPRESSOR_OWN_VCPKG=1 force l'usage de notre propre clone.
baseline="$(sed -n 's/.*"builtin-baseline": *"\([0-9a-f]\{40\}\)".*/\1/p' "$root/vcpkg.json")"
if [ -n "$vcpkg" ] && [ -e "$(dirname "$vcpkg")/.git" ] && [ "$(git -C "$(dirname "$vcpkg")" rev-parse HEAD 2>/dev/null)" != "$baseline" ]; then
  echo "vcpkg : $vcpkg n'est pas au commit fixé par vcpkg.json, on prend notre propre clone"
  vcpkg=""
fi
if [ -n "${VIDEO_COMPRESSOR_OWN_VCPKG:-}" ]; then vcpkg=""; fi

if [ -z "$vcpkg" ]; then
  if [ ! -d "$build/vcpkg/.git" ]; then
    git init -q "$build/vcpkg"
    git -C "$build/vcpkg" remote add origin https://github.com/microsoft/vcpkg
  fi
  # Exactement le commit de vcpkg.json (à défaut, la branche principale).
  if [ "$(git -C "$build/vcpkg" rev-parse HEAD 2>/dev/null)" != "$baseline" ] || [ -z "$baseline" ]; then
    git -C "$build/vcpkg" fetch --depth 1 origin "${baseline:-master}"
    git -C "$build/vcpkg" checkout -q FETCH_HEAD
  fi
  if $windows; then
    [ -f "$build/vcpkg/vcpkg.exe" ] || cmd //c "$(cygpath -w "$build/vcpkg/bootstrap-vcpkg.bat")" -disableMetrics
  else
    [ -f "$build/vcpkg/vcpkg" ] || "$build/vcpkg/bootstrap-vcpkg.sh" -disableMetrics
  fi
  vcpkg="$build/vcpkg/vcpkg"
fi
echo "vcpkg : $vcpkg"
echo "triplet : $triplet, dossier de travail : $build"

native() { if $windows; then cygpath -w "$1"; else echo "$1"; fi; }
"$vcpkg" install --triplet "$triplet" \
  --x-manifest-root="$(native "$build")" \
  --x-install-root="$(native "$build/installed")" \
  --x-buildtrees-root="$(native "$build/bt")" \
  --x-packages-root="$(native "$build/pkg")" \
  --downloads-root="$(native "$build/dl")"

# Les en-têtes et les bibliothèques, là où `.cargo/config.toml` les désigne.
dest="$root/vcpkg_installed/$name"
rm -rf "$dest"
mkdir -p "$dest"
cp -R "$build/installed/$triplet/include" "$build/installed/$triplet/lib" "$dest/"
echo "bibliothèques de FFmpeg prêtes dans $dest"
