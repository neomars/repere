#!/usr/bin/env bash
# Diagnostic des prérequis pour compiler et exécuter l'app Ubuntu (GPU NVIDIA). Code de sortie 1 si un
# prérequis BLOQUANT manque ; les avertissements (!) n'empêchent pas la compilation.
#   bash scripts/doctor.sh          (ou : npm run doctor)
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FAIL=0
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }
bad()  { printf '  \033[31m✗\033[0m %s\n' "$*"; FAIL=1; }
ver_ge() { [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -1)" = "$2" ]; }   # ver_ge <trouvée> <requise>
has() { command -v "$1" >/dev/null 2>&1; }

echo "Système"
. /etc/os-release 2>/dev/null || true
GLIBC="$(ldd --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+$' || echo 0)"
if [ "$(uname -m)" = "x86_64" ]; then ok "architecture x86_64"; else bad "architecture $(uname -m) : seule x86_64 est gérée (binaires llama.cpp CUDA officiels)"; fi
ok "${PRETTY_NAME:-Linux} (glibc $GLIBC)"
if ver_ge "$GLIBC" 2.38; then ok "glibc ≥ 2.38 : binaire llama-server officiel utilisable"
else warn "glibc $GLIBC < 2.38 (Ubuntu 22.04) : utilise LLAMA_MODE=source (compile llama.cpp, exige nvcc)"; fi
FREE_GB=$(df -Pk "$ROOT" | awk 'NR==2{printf "%d", $4/1024/1024}')
if [ "$FREE_GB" -ge 10 ]; then ok "espace libre ici : ${FREE_GB} Go"; else warn "espace libre ici : ${FREE_GB} Go (compilation + paquets : ≥ 10 Go conseillés)"; fi
HOME_FREE=$(df -Pk "$HOME" | awk 'NR==2{printf "%d", $4/1024/1024}')
if [ "$HOME_FREE" -ge 25 ]; then ok "espace libre dans \$HOME : ${HOME_FREE} Go (modèles ≈ 8 + 4 + 1 Go, moteur de voix ≈ 3 Go)"
else warn "espace libre dans \$HOME : ${HOME_FREE} Go : prévois ≈ 20 Go pour les modèles (REPERE_MODELS_DIR pour les mettre ailleurs)"; fi

echo "Outils de compilation"
if has node; then
  NV="$(node -p 'process.versions.node')"
  if ver_ge "$NV" 20.19.0; then ok "Node $NV"; else bad "Node $NV : version ≥ 20.19 requise (Vite 7)"; fi
else bad "node introuvable (https://nodejs.org ou nvm)"; fi
has npm && ok "npm $(npm -v)" || bad "npm introuvable"
if has cargo && has rustc; then
  RV="$(rustc --version | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)"
  if ver_ge "$RV" 1.82.0; then ok "Rust $RV"; else bad "Rust $RV : version ≥ 1.82 requise (rustup update)"; fi
else bad "cargo introuvable : https://rustup.rs"; fi
for t in git curl tar; do has "$t" && ok "$t" || bad "$t introuvable (sudo apt install $t)"; done
if has g++ && has make; then ok "g++ / make"; else bad "g++/make introuvables (sudo apt install build-essential)"; fi

echo "GPU NVIDIA"
if has nvidia-smi && nvidia-smi >/dev/null 2>&1; then
  ok "$(nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader | head -1)"
  DRV="$(nvidia-smi --query-gpu=driver_version --format=csv,noheader | head -1 | cut -d. -f1-2)"
  if ver_ge "${DRV:-0}" 570.26; then ok "pilote $DRV : compatible CUDA 12.8 (binaire llama-server officiel)"
  else warn "pilote $DRV < 570.26 : le binaire CUDA 12.8 ne démarrera pas ; mets à jour le pilote NVIDIA"; fi
else warn "nvidia-smi indisponible : pas de GPU NVIDIA détecté ici (normal sur une machine de build sans GPU)"; fi
if has nvcc; then
  ok "CUDA Toolkit : $(nvcc --version | tail -1)"
  has cmake && ver_ge "$(cmake --version | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')" 3.24 && ok "cmake $(cmake --version | head -1 | grep -oE '[0-9]+\.[0-9.]+')" \
    || bad "cmake ≥ 3.24 requis pour whisper-server (sudo apt install cmake)"
else warn "nvcc introuvable : whisper-server (micro) sera compilé pour le processeur (plus lent) ; installe le CUDA Toolkit pour un micro rapide"; fi

echo "Bibliothèques d'exécution (llama-server officiel)"
for lib in libgomp.so.1 libssl.so.3; do
  if ldconfig -p 2>/dev/null | grep -q "$lib"; then ok "$lib"; else warn "$lib absente (sudo apt install libgomp1 libssl3t64)"; fi
done

echo "Moteur de voix (Python)"
if has python3 && python3 -c 'import venv, ensurepip' >/dev/null 2>&1; then ok "python3 $(python3 -V | cut -d' ' -f2) + venv (uv fournira Python 3.11)"
else warn "python3-venv absent (sudo apt install python3-venv) : nécessaire pour « Installer le moteur de voix »"; fi

echo
if [ "$FAIL" = 0 ]; then echo "Prêt : npm run dist:linux"; else echo "Des prérequis bloquants manquent (✗)."; fi
exit "$FAIL"
