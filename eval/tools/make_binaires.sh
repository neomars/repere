#!/usr/bin/env bash
# Fabrique les documents Word et PDF du corpus d'évaluation à partir des sources Markdown de eval/tools/sources.
# Prérequis : pandoc et LibreOffice (soffice). Les fichiers produits sont versionnés : ce script ne sert qu'à les régénérer.
set -euo pipefail
cd "$(dirname "$0")"
OUT=../corpus
for f in 03-vpn-client 10-teams-visioconference; do
  pandoc "sources/$f.md" -o "$OUT/$f.docx"
done
# PDF : un document Word converti par LibreOffice, comme en produisent les services
tmp=$(mktemp -d)
pandoc sources/04-teletravail-bonnes-pratiques.md -o "$tmp/04-teletravail-bonnes-pratiques.docx"
soffice --headless --convert-to pdf --outdir "$OUT" "$tmp/04-teletravail-bonnes-pratiques.docx" >/dev/null
rm -rf "$tmp"
ls -l "$OUT"/*.docx "$OUT"/*.pdf
