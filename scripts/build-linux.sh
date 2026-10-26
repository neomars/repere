#!/usr/bin/env bash
# Construit l'application Ubuntu complète : interface Vue + moteur Rust + llama-server/whisper-server (CUDA)
# + Electron, et produit AppImage + .deb dans ./dist.
#
#   bash scripts/build-linux.sh [options]        (ou : npm run dist:linux)
#     --dir             ne produit que le dossier dist/linux-unpacked (plus rapide, pour tester)
#     --deb             ne produit que le .deb (pas d'AppImage) : c'est ce que fait GitHub Actions
#     --skip-install    ne relance pas npm install
#     --skip-sidecars   ne prépare pas ./bin (llama-server, whisper-server)
#     --no-doctor       n'exécute pas le diagnostic des prérequis
# Variables : voir scripts/build-sidecars.sh (CUDA_ARCH, LLAMA_MODE, SKIP_WHISPER, …).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

DIR_ONLY=0; DEB_ONLY=0; INSTALL=1; SIDECARS=1; DOCTOR=1
for a in "$@"; do
  case "$a" in
    --dir) DIR_ONLY=1 ;;
    --deb) DEB_ONLY=1 ;;
    --skip-install) INSTALL=0 ;;
    --skip-sidecars) SIDECARS=0 ;;
    --no-doctor) DOCTOR=0 ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "option inconnue : $a" >&2; exit 2 ;;
  esac
done

step() { printf '\n\033[1m→ %s\033[0m\n' "$*"; }

if [ "$DOCTOR" = 1 ]; then step "diagnostic"; bash scripts/doctor.sh || { echo "Corrige les ✗ ci-dessus (ou --no-doctor)." >&2; exit 1; }; fi

if [ "$INSTALL" = 1 ]; then
  step "dépendances npm"
  # --no-package-lock : package-lock.json ne change que lorsqu'on modifie les dépendances, pas à chaque build.
  # --omit=optional (racine et backend) : alasql déclare react-native-fs en dépendance optionnelle, ce qui ajouterait
  # react-native et ~150 paquets (≈ 190 Mo) à l'app. Pas pour le frontend : Vite a besoin de ses binaires optionnels.
  npm install --no-audit --no-fund --no-package-lock --omit=optional
  (cd backend && npm install --no-audit --no-fund --no-package-lock --omit=optional)
  (cd frontend && npm install --no-audit --no-fund --no-package-lock)
fi

step "interface (Vue)";            npm run --silent build:frontend
step "moteur (Rust, release)";     npm run --silent build:engine
if [ "$SIDECARS" = 1 ]; then step "serveurs IA (llama-server, whisper-server, voix)"; bash scripts/build-sidecars.sh; fi

step "paquets Ubuntu"
if [ "$DIR_ONLY" = 1 ]; then npx electron-builder --linux dir --publish never
elif [ "$DEB_ONLY" = 1 ]; then npx electron-builder --linux deb --publish never
else npx electron-builder --linux --publish never; fi

step "vérification du paquet"
bash scripts/verify-package.sh

echo; echo "✓ Terminé :"; ls -lh dist/*.AppImage dist/*.deb 2>/dev/null || ls dist/linux-unpacked | head
