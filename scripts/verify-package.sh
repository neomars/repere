#!/usr/bin/env bash
# Vérifie le contenu de dist/linux-unpacked : ce qui doit y être (moteur, serveurs IA, interface) et ce qui ne doit
# JAMAIS y être (base de données locale avec le hash du mot de passe, uploads, .env, node_modules du backend).
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
U="${1:-dist/linux-unpacked}"; R="$U/resources"; FAIL=0
ok()  { printf '  \033[32m✓\033[0m %s\n' "$*"; }
bad() { printf '  \033[31m✗\033[0m %s\n' "$*"; FAIL=1; }
warn(){ printf '  \033[33m!\033[0m %s\n' "$*"; }

[ -d "$U" ] || { echo "$U introuvable : lance d'abord le build." >&2; exit 1; }
[ -x "$U/repere" ] && ok "exécutable repere" || bad "exécutable repere absent"
[ -x "$R/engine/live-engine" ] && ok "moteur engine/live-engine (exécutable)" || bad "moteur engine/live-engine absent ou non exécutable"
for f in llama-server tts-server; do
  [ -x "$R/bin/$f" ] && ok "bin/$f" || bad "bin/$f absent ou non exécutable (scripts/build-sidecars.sh)"
done
[ -f "$R/bin/chatterbox_server.py" ] && [ -f "$R/bin/setup-tts.sh" ] && ok "scripts de voix (chatterbox_server.py, setup-tts.sh)" || bad "scripts de voix absents de bin/"
if [ -x "$R/bin/whisper-server" ]; then ok "bin/whisper-server"; else warn "bin/whisper-server absent (micro indisponible ; SKIP_WHISPER=1 ?)"; fi

ASAR="$R/app.asar"
if [ -f "$ASAR" ]; then
  LIST="$(npx --no-install asar list "$ASAR" 2>/dev/null || npx asar list "$ASAR")"
  need() { grep -qE "$1" <<<"$LIST" && ok "app.asar contient $2" || bad "app.asar ne contient pas $2"; }
  deny() { grep -qE "$1" <<<"$LIST" && bad "app.asar contient $2 (à exclure !)" || ok "app.asar ne contient pas $2"; }
  need '^/main\.js$' "main.js"
  need '^/preload\.js$' "preload.js"
  need '^/backend/server\.js$' "backend/server.js"
  need '^/backend/liveProxy\.js$' "backend/liveProxy.js"
  need '^/frontend/dist/index\.html$' "frontend/dist/index.html"
  need '^/node_modules/express/' "express (dépendances de production)"
  # backend/node_modules est exclu : le serveur charge ses modules depuis /node_modules. Un seul absent, et l'app installée
  # s'arrête au démarrage (« Cannot find module »).
  for dep in $(node -p "Object.keys(require('./backend/package.json').dependencies || {}).join(' ')"); do
    need "^/node_modules/$dep/package\.json$" "$dep (module du serveur)"
  done
  deny '^/backend/db\.json$' "backend/db.json (base locale + hash admin)"
  deny '^/backend/uploads' "backend/uploads"
  deny '^/backend/node_modules' "backend/node_modules (doublon)"
  deny '^/backend/\.env' "backend/.env"
else
  bad "app.asar introuvable"
fi
if [ -x "$R/app.asar.unpacked/node_modules/ffmpeg-static/ffmpeg" ]; then ok "ffmpeg embarqué (miniatures, import massif)"
elif [ -d "$R/app.asar.unpacked/node_modules/ffmpeg-static" ]; then warn "ffmpeg-static présent mais sans binaire (téléchargement échoué à npm install ?) : l'app utilisera le ffmpeg du système, sinon pas de miniatures"
else warn "ffmpeg-static non dépaqueté (miniatures indisponibles ?)"; fi
exit "$FAIL"
