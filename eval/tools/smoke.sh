#!/usr/bin/env bash
# Contrôle de toute la chaîne sans modèle : un faux modèle (faux-llm.mjs), un moteur neuf à base vide, puis `eval/run.mjs --plumbing`.
# Vérifie l'import du corpus par l'interface REST, la recherche des passages, la directive [[source: #id]] et son garde-fou, les
# boutons de ticket et le résumé du ticket. Ne dit rien de la qualité des réponses d'un vrai modèle.
#
# Usage : eval/tools/smoke.sh [moteur compilé] [options de run.mjs]
#   moteur compilé : défaut live-engine/target/debug/live-engine (cargo build --locked, dans live-engine)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
engine="${1:-$root/live-engine/target/debug/live-engine}"
shift || true
[ -x "$engine" ] || { echo "Moteur introuvable ou non exécutable : $engine (compilez-le : cd live-engine && cargo build --locked)" >&2; exit 2; }

work="$(mktemp -d)"
pids=()
cleanup() {
  for p in "${pids[@]:-}"; do kill "$p" 2>/dev/null || true; done
  wait 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

llm_port="${FAUX_LLM_PORT:-18080}"
engine_port="${ENGINE_PORT:-13001}"
mkdir -p "$work/data" "$work/models" "$work/bin" "$work/uploads" "$work/nofront"
cat > "$work/live.toml" <<TOML
[llm]
url = "http://127.0.0.1:${llm_port}"
[stt]
enabled = false
[tts]
enabled = false
[engine]
managed = false
TOML

node "$here/faux-llm.mjs" --port "$llm_port" > "$work/faux-llm.log" 2>&1 &
pids+=($!)
LIVE_CONFIG="$work/live.toml" LIVE_BIND="127.0.0.1:${engine_port}" LIVE_DATA_DIR="$work/data" LIVE_MODELS_DIR="$work/models" \
  LIVE_BIN_DIR="$work/bin" LIVE_UPLOADS_DIR="$work/uploads" LIVE_FRONTEND_DIR="$work/nofront" "$engine" > "$work/engine.log" 2>&1 &
pids+=($!)

ready=0
for _ in $(seq 1 80); do
  if curl -fs "http://127.0.0.1:${engine_port}/api/live/health" 2>/dev/null | grep -q '"llm":true'; then ready=1; break; fi
  sleep 0.5
done
if [ "$ready" != 1 ]; then
  echo "Le moteur n'est pas prêt (ou ne voit pas le faux modèle) :" >&2
  tail -n 20 "$work/engine.log" >&2 || true
  exit 2
fi

set +e
node "$root/eval/run.mjs" --engine "http://127.0.0.1:${engine_port}" --plumbing "$@"
code=$?
set -e
if [ "$code" != 0 ]; then
  echo "--- journal du moteur (fin)" >&2
  tail -n 15 "$work/engine.log" >&2 || true
fi
exit "$code"
