#!/usr/bin/env node
// Faux serveur de modèle (même interface que llama-server) qui répond en CITANT les passages que le moteur lui propose.
//
// Ce n'est pas un modèle : il ne comprend rien. Il sert à vérifier toute la chaîne sans GPU ni téléchargement — recherche des passages,
// prompt, directive [[source: #id]] et son garde-fou, boutons de ticket, résumé du ticket — par exemple dans l'intégration continue
// (`eval/tools/smoke.sh`). Il ne dit rien de la qualité des réponses d'un vrai modèle : pour cela, lancer `eval/run.mjs` sur un moteur
// démarré avec un vrai modèle.
//
//   node eval/tools/faux-llm.mjs [--port 18080]     (ou FAUX_LLM_PORT=18080)
//
// Comportement :
//   • des passages sont proposés dans le prompt système → il recopie les trois premiers et les cite ;
//   • aucun passage → « Je ne sais pas : je n'ai pas trouvé cela dans la base de connaissances » et les boutons de ticket ;
//   • demande de résumé (sans flux) → « Résumé : » suivi du premier message de l'utilisateur.
// FAUX_LLM_LOG=fichier.jsonl note chaque requête reçue (pour vérifier ce que le moteur envoie au modèle).

import http from 'node:http'
import fs from 'node:fs'

const arg = (name) => { const i = process.argv.indexOf(`--${name}`); return i > 0 ? process.argv[i + 1] : undefined }
const PORT = Number(arg('port') || process.env.FAUX_LLM_PORT || 18080)
const LOG = process.env.FAUX_LLM_LOG
if (LOG) fs.writeFileSync(LOG, '')

/** Les passages proposés dans le prompt : « #12 Extrait de « Titre › Section », page 3 :\n texte ». */
export function offeredPassages(system) {
  const out = []
  const re = /^#(\d+) Extrait de « (.+?) »(?:, page (\d+))? :\n([\s\S]*?)(?=\n#\d+ |\n- Appuie ta réponse|(?![\s\S]))/gm
  for (const m of system.matchAll(re)) out.push({ id: Number(m[1]), place: m[2], page: m[3] ? Number(m[3]) : null, text: m[4].trim() })
  return out
}

/** La réponse du faux modèle, d'après le prompt système et le dernier message de l'utilisateur. */
export function answer(system, lastUser) {
  if (/Début de la conversation/.test(lastUser)) return 'Bonjour ! Quel est votre problème informatique ?'
  const passages = offeredPassages(system)
  if (!passages.length) {
    return "Je ne sais pas : je n'ai pas trouvé cela dans la base de connaissances. Je peux vous aider à ouvrir un ticket. [[replies: Ouvrir un ticket | Reformuler]]"
  }
  const parts = passages.slice(0, 3).map((p) => `${p.text.replace(/\s+/g, ' ')} [[source: #${p.id}]]`)
  return `D'après la base de connaissances : ${parts.join(' ')} [[replies: Ça a marché | Ouvrir un ticket]]`
}

/** Le « résumé » d'une conversation : le premier message de l'utilisateur dans la demande de résumé. */
export function summary(lastUser) {
  const first = /^user: (.+)$/m.exec(lastUser)
  return `Résumé : ${first ? first[1].trim() : 'conversation sans message'}`
}

function serve(req, res) {
  const json = (status, body) => { res.writeHead(status, { 'Content-Type': 'application/json' }); res.end(JSON.stringify(body)) }
  if (req.method === 'GET' && (req.url === '/health' || req.url === '/v1/models')) return json(200, { status: 'ok' })
  if (req.method === 'GET' && req.url === '/props') return json(200, { modalities: { vision: false } })
  if (req.method !== 'POST' || req.url !== '/v1/chat/completions') { res.writeHead(404); return res.end() }

  let body = ''
  req.on('data', (c) => { body += c })
  req.on('end', () => {
    let j
    try { j = JSON.parse(body) } catch { return json(400, { error: 'JSON invalide' }) }
    if (LOG) fs.appendFileSync(LOG, JSON.stringify(j) + '\n')
    const messages = j.messages || []
    const system = (messages.find((m) => m.role === 'system') || {}).content || ''
    const lastUser = [...messages].reverse().find((m) => m.role === 'user')?.content || ''
    const text = typeof lastUser === 'string' ? lastUser : JSON.stringify(lastUser)
    if (!j.stream) return json(200, { choices: [{ message: { content: summary(text) }, finish_reason: 'stop' }] })

    res.writeHead(200, { 'Content-Type': 'text/event-stream' })
    // de petits morceaux, comme un vrai flux de jetons (y compris au milieu d'une directive)
    const pieces = answer(system, text).match(/.{1,9}/gs) || []
    let i = 0
    const tick = () => {
      if (i < pieces.length) {
        res.write('data: ' + JSON.stringify({ choices: [{ delta: { content: pieces[i++] } }] }) + '\n\n')
        setTimeout(tick, 2)
      } else {
        res.write('data: [DONE]\n\n')
        res.end()
      }
    }
    tick()
  })
}

if (import.meta.url === `file://${process.argv[1]}`) {
  http.createServer(serve).listen(PORT, '127.0.0.1', () => console.log(`faux modèle sur http://127.0.0.1:${PORT}`))
}
