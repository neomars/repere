#!/usr/bin/env bash
# Prépare ./bin avec les serveurs IA pour GPU NVIDIA :
#   • llama-server  : binaire OFFICIEL précompilé (CUDA 12.8, Ubuntu 24.04), vérifié par SHA-256 — rien à compiler ;
#                     (LLAMA_MODE=source pour le compiler soi-même, p. ex. sur Ubuntu 22.04)
#   • whisper-server: compilé ici avec CUDA (whisper.cpp ne publie pas de binaire Linux CUDA).
#
# Prérequis : curl tar git ; pour whisper : build-essential cmake + CUDA Toolkit (nvcc).
#   sudo apt install build-essential cmake git curl
#   CUDA Toolkit : https://developer.nvidia.com/cuda-downloads
#
# Variables :
#   LLAMA_MODE=prebuilt|source   (défaut prebuilt)      LLAMA_CPP_TAG  (défaut b11146, tag « nightly » llama.cpp)
#   LLAMA_CUDA=12.8              (variante CUDA du binaire officiel ; 13.4 exige un pilote NVIDIA ≥ 580)
#   LLAMA_SHA256 / CUDART_SHA256 (empreintes ; obligatoires si tu changes LLAMA_CPP_TAG ou LLAMA_CUDA, sinon vérif ignorée)
#   LLAMA_BASE_URL (miroir/tests)  WHISPER_CPP_REF (défaut v1.9.4)  SKIP_WHISPER=1 (ne pas compiler whisper-server)
#   CUDA_ARCH (défaut : native si un GPU est visible, sinon "75;80;86;89" (+120 avec CUDA ≥ 12.8) ; ex. "86;89")
#   BUNDLE_CUDA_LIBS=1 (défaut ; mode source / whisper) copie libcudart/libcublas dans ./bin
#   FORCE=1 refait tout.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/bin"
SRC="$ROOT/.build"
LLAMA_MODE="${LLAMA_MODE:-prebuilt}"
LLAMA_CUDA="${LLAMA_CUDA:-12.8}"
WHISPER_CPP_REF="${WHISPER_CPP_REF:-v1.9.4}"
# native = la carte de CETTE machine ; sans GPU visible (serveur de build, conteneur) nvcc échoue avec « native » :
# on prend alors une liste fixe (Turing → Ada, + Blackwell avec CUDA ≥ 12.8).
if [ -z "${CUDA_ARCH:-}" ]; then
  if command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi -L >/dev/null 2>&1; then CUDA_ARCH=native
  else
    CUDA_ARCH="75;80;86;89"
    NVCC_VER="$(nvcc --version 2>/dev/null | grep -oE 'release [0-9]+\.[0-9]+' | cut -d' ' -f2 || true)"
    if [ -n "${NVCC_VER:-}" ] && [ "$(printf '%s\n12.8\n' "$NVCC_VER" | sort -V | head -1)" = "12.8" ]; then CUDA_ARCH="$CUDA_ARCH;120"; fi
  fi
fi
BUNDLE_CUDA_LIBS="${BUNDLE_CUDA_LIBS:-1}"
JOBS="${JOBS:-$(nproc)}"

# Empreintes SHA-256 mesurées après téléchargement de la release b11146 (CUDA 12.8, Ubuntu x64).
DEFAULT_TAG="b11146"
DEFAULT_LLAMA_SHA256="c2ab9e19838513ff69d1af8d999ad717dd3c7ee4714ac04c7ed5ab9077c50e4e"
DEFAULT_CUDART_SHA256="1466daea60aad1144819e151b2bae19d54556cf1da6c129c4f55a5ded2637c25"
LLAMA_CPP_TAG="${LLAMA_CPP_TAG:-$DEFAULT_TAG}"
if [ "$LLAMA_CPP_TAG" = "$DEFAULT_TAG" ] && [ "$LLAMA_CUDA" = "12.8" ]; then
  LLAMA_SHA256="${LLAMA_SHA256:-$DEFAULT_LLAMA_SHA256}"
  CUDART_SHA256="${CUDART_SHA256:-$DEFAULT_CUDART_SHA256}"
else
  LLAMA_SHA256="${LLAMA_SHA256:-}"
  CUDART_SHA256="${CUDART_SHA256:-}"
fi

need() { command -v "$1" >/dev/null 2>&1 || { echo "✗ « $1 » est introuvable. $2" >&2; exit 1; }; }
need git "sudo apt install git"
need tar "sudo apt install tar"
mkdir -p "$BIN" "$SRC"
record() { # exe description
  touch "$BIN/VERSIONS.txt"; sed -i "/^$1 :/d" "$BIN/VERSIONS.txt"; echo "$1 : $2" >> "$BIN/VERSIONS.txt"
}

# ---------------------------------------------------------------------------------- llama-server
llama_prebuilt() {
  need curl "sudo apt install curl"
  local glibc; glibc="$(ldd --version | head -1 | grep -oE '[0-9]+\.[0-9]+$')"
  if [ "$(printf '%s\n2.38\n' "$glibc" | sort -V | head -1)" != "2.38" ]; then
    echo "! glibc $glibc < 2.38 : les binaires officiels exigent Ubuntu 24.04. Bascule en compilation (LLAMA_MODE=source)." >&2
    LLAMA_MODE=source; llama_source; return
  fi
  local base="${LLAMA_BASE_URL:-https://github.com/ggml-org/llama.cpp/releases/download}/$LLAMA_CPP_TAG"
  local a="llama-$LLAMA_CPP_TAG-bin-ubuntu-cuda-$LLAMA_CUDA-x64.tar.gz"
  local b="cudart-llama-$LLAMA_CPP_TAG-bin-ubuntu-cuda-$LLAMA_CUDA-x64.tar.gz"
  local tmp="$SRC/llama-prebuilt"; rm -rf "$tmp"; mkdir -p "$tmp/a" "$tmp/b"
  for f in "$a:$LLAMA_SHA256" "$b:$CUDART_SHA256"; do
    local name="${f%%:*}" sha="${f#*:}"
    echo "→ téléchargement $name"
    curl -fL --retry 3 -o "$tmp/$name" "$base/$name"
    if [ -n "$sha" ]; then
      echo "$sha  $tmp/$name" | sha256sum -c - || { echo "✗ SHA-256 incorrect pour $name : fichier rejeté" >&2; exit 1; }
    else
      echo "  ! pas d'empreinte fournie pour $name : vérification ignorée"
    fi
  done
  tar -xzf "$tmp/$a" -C "$tmp/a"; tar -xzf "$tmp/$b" -C "$tmp/b"
  # Mise à plat : llama-server + toutes les bibliothèques (RUNPATH=$ORIGIN, donc un seul dossier suffit).
  find "$tmp/a" "$tmp/b" \( -name 'llama-server' -o -name '*.so' -o -name '*.so.*' \) \( -type f -o -type l \) \
       -exec cp -a {} "$BIN/" \;
  chmod +x "$BIN/llama-server"
  record llama-server "llama.cpp $LLAMA_CPP_TAG officiel (CUDA $LLAMA_CUDA)"
}

llama_source() {
  need cmake "sudo apt install cmake"; need g++ "sudo apt install build-essential"
  need nvcc "Installe le CUDA Toolkit (https://developer.nvidia.com/cuda-downloads) et ajoute /usr/local/cuda/bin au PATH."
  local ref="${LLAMA_CPP_REF:-master}"  # Gemma 4 exige une version récente
  local dir="$SRC/llama.cpp"
  [ -d "$dir/.git" ] && { git -C "$dir" fetch --depth 1 origin "$ref" && git -C "$dir" checkout -q FETCH_HEAD; } \
    || git clone --depth 1 --branch "$ref" https://github.com/ggml-org/llama.cpp "$dir"
  cmake -S "$dir" -B "$dir/build" -DCMAKE_BUILD_TYPE=Release -DGGML_CUDA=ON -DCMAKE_CUDA_ARCHITECTURES="$CUDA_ARCH" \
        -DBUILD_SHARED_LIBS=ON -DCMAKE_INSTALL_RPATH='$ORIGIN' -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON -DGGML_CUDA_NCCL=OFF \
        -DLLAMA_BUILD_TESTS=OFF -DLLAMA_USE_PREBUILT_UI=OFF
  cmake --build "$dir/build" --config Release --target llama-server -j "$JOBS"
  find "$dir/build/bin" \( -name 'llama-server' -o -name '*.so' -o -name '*.so.*' \) -exec cp -a {} "$BIN/" \;
  record llama-server "llama.cpp $ref @ $(git -C "$dir" rev-parse --short HEAD) compilé (CUDA arch $CUDA_ARCH)"
  copy_cuda_libs
}

copy_cuda_libs() {
  [ "$BUNDLE_CUDA_LIBS" = "1" ] || return 0
  local home="${CUDA_HOME:-$(dirname "$(dirname "$(command -v nvcc)")")}"
  echo "→ bibliothèques d'exécution CUDA (depuis $home)"
  for lib in libcudart.so libcublas.so libcublasLt.so; do
    local found=0
    for d in "$home/lib64" "$home/targets/x86_64-linux/lib" /usr/lib/x86_64-linux-gnu; do
      for f in "$d/$lib".*; do [ -e "$f" ] && { cp -na "$f" "$BIN/"; found=1; }; done
      [ $found = 1 ] && break
    done
    [ $found = 1 ] || echo "  ! $lib introuvable : le PC cible devra avoir le runtime CUDA installé" >&2
  done
}

# ------------------------------------------------------------------------------- whisper-server
whisper_source() {
  need cmake "sudo apt install cmake"; need g++ "sudo apt install build-essential"
  # Sans nvcc, on compile quand même whisper-server, pour le processeur : le micro marche (plus lentement) au lieu
  # d'être absent du paquet. Le fichier « whisper-server.cpu » prévient le moteur (pas de VRAM réservée, option -ng).
  local cuda=1
  if ! command -v nvcc >/dev/null 2>&1; then
    cuda=0
    echo "! nvcc introuvable : whisper-server sera compilé SANS CUDA (sur le processeur, plus lent)." >&2
    echo "  Installe le CUDA Toolkit puis relance avec FORCE=1 pour un micro rapide ; choisis alors un modèle Whisper « small »." >&2
  fi
  local dir="$SRC/whisper.cpp"
  [ -d "$dir/.git" ] && { git -C "$dir" fetch --depth 1 origin "$WHISPER_CPP_REF" && git -C "$dir" checkout -q FETCH_HEAD; } \
    || git clone --depth 1 --branch "$WHISPER_CPP_REF" https://github.com/ggml-org/whisper.cpp "$dir"
  if [ "$cuda" = "1" ]; then
    cmake -S "$dir" -B "$dir/build" -DCMAKE_BUILD_TYPE=Release -DGGML_CUDA=1 -DCMAKE_CUDA_ARCHITECTURES="$CUDA_ARCH" \
          -DBUILD_SHARED_LIBS=OFF -DWHISPER_BUILD_TESTS=OFF
  else
    rm -rf "$dir/build" # un ancien cache CMake avec CUDA ferait échouer la configuration sans nvcc
    cmake -S "$dir" -B "$dir/build" -DCMAKE_BUILD_TYPE=Release -DGGML_CUDA=0 -DBUILD_SHARED_LIBS=OFF -DWHISPER_BUILD_TESTS=OFF
  fi
  cmake --build "$dir/build" --config Release --target whisper-server -j "$JOBS"
  install -m 0755 "$dir/build/bin/whisper-server" "$BIN/whisper-server"
  if [ "$cuda" = "1" ]; then
    rm -f "$BIN/whisper-server.cpu"
    record whisper-server "whisper.cpp $WHISPER_CPP_REF @ $(git -C "$dir" rev-parse --short HEAD) compilé (CUDA arch $CUDA_ARCH)"
    # Même soname CUDA que llama-server si possible ; sinon on ajoute les bibliothèques de la machine de build.
    if ! ldd "$BIN/whisper-server" | grep -q "not found"; then :; else copy_cuda_libs; fi
  else
    echo "cpu" > "$BIN/whisper-server.cpu"
    record whisper-server "whisper.cpp $WHISPER_CPP_REF @ $(git -C "$dir" rev-parse --short HEAD) compilé SANS CUDA (processeur)"
  fi
}

# ------------------------------------------------------------------------------- serveur de voix
# Lanceur + serveur Python (le moteur de voix lui-même s'installe depuis l'app : « Installer le moteur de voix »).
install -m 0755 "$ROOT/scripts/tts/tts-server" "$ROOT/scripts/tts/setup-tts.sh" "$ROOT/scripts/tts/chatterbox_server.py" "$BIN/"

if [ -x "$BIN/llama-server" ] && [ "${FORCE:-0}" != "1" ]; then
  echo "✓ llama-server déjà présent (FORCE=1 pour refaire)"
elif [ "$LLAMA_MODE" = "source" ]; then llama_source
else llama_prebuilt; fi

if [ "${SKIP_WHISPER:-0}" = "1" ]; then
  echo "- whisper-server ignoré (SKIP_WHISPER=1)"
elif [ -x "$BIN/whisper-server" ] && [ "${FORCE:-0}" != "1" ]; then
  echo "✓ whisper-server déjà présent (FORCE=1 pour refaire)"
else whisper_source; fi

echo "✓ Sidecars prêts dans $BIN :"
cat "$BIN/VERSIONS.txt" 2>/dev/null | sed 's/^/   /'
du -sh "$BIN" | sed 's/^/   taille : /'
