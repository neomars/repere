#!/usr/bin/env node
// Évaluation de bout en bout : pose les 30 questions de eval/questions.json à un moteur Repère démarré, comme le ferait un utilisateur
// (WebSocket de la conversation), et vérifie ce qu'on peut vérifier à la machine : les sources citées, les informations attendues
// dans la réponse, les réponses interdites, le « je ne sais pas » et le bouton de ticket des questions sans réponse, et qu'aucune adresse
// ne soit inventée. Ce que dit un vrai modèle varie d'une exécution à l'autre : le résultat est un score, pas une preuve.
//
//   node eval/run.mjs [--engine http://127.0.0.1:3001] [--persona <id ou nom>] [--only q01,q07] [--plumbing] [--min 0.8] [--force]
//
//   --plumbing  contrôle de la chaîne avec le faux modèle (eval/tools/faux-llm.mjs) : les questions qui dépendent du jugement du modèle
//               (`judgment` dans questions.json) sont laissées de côté et tout le reste doit passer (voir eval/tools/smoke.sh)
//   --min       score minimal (0 à 1) ; en dessous, le code de sortie est 1 (par défaut 0 : toujours 0 sauf en --plumbing, où il faut 1)
//   --force     lance l'évaluation même si la base contient d'autres documents que le corpus (déconseillé : voir ci-dessous)
//
// À lancer sur un moteur DÉDIÉ, avec une base vide : l'évaluation importe le corpus de eval/corpus, et chaque question ouvre une
// nouvelle conversation, qui remplace la conversation sauvegardée du profil d'assistant. Node 22 ou plus récent (WebSocket intégré).

import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))

// ───────────────────────────── options

export function parseArgs(argv) {
  const o = { engine: 'http://127.0.0.1:3001', persona: '', only: [], plumbing: false, min: null, force: false, timeout: 180 }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    const next = () => { if (i + 1 >= argv.length) throw new Error(`${a} attend une valeur`); return argv[++i] }
    if (a === '--engine') o.engine = next().replace(/\/+$/, '')
    else if (a === '--persona') o.persona = next()
    else if (a === '--only') o.only = next().split(',').map((s) => s.trim()).filter(Boolean)
    else if (a === '--plumbing') o.plumbing = true
    else if (a === '--min') o.min = Number(next())
    else if (a === '--force') o.force = true
    else if (a === '--timeout') o.timeout = Number(next())
    else if (a === '--help' || a === '-h') o.help = true
    else throw new Error(`option inconnue : ${a}`)
  }
  if (o.min !== null && !(o.min >= 0 && o.min <= 1)) throw new Error('--min doit être compris entre 0 et 1')
  return o
}

// ───────────────────────────── vérifications (pures, testées par run.test.mjs)

/** Minuscules, sans accents, apostrophes en espaces. */
export const fold = (s) => String(s ?? '').normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase().replace(/[’']/g, ' ').replace(/\s+/g, ' ').trim()

/** La réponse dit-elle qu'elle ne sait pas ? (les formules que demandent les règles de l'assistant ; même liste que le moteur) */
export const saysUnknown = (text) => /je ne sais pas|je n ai pas trouve|je ne trouve pas|aucune source ne/.test(fold(text))

/** Les adresses (liens, courriels) écrites dans une réponse. */
export function addressesIn(text) {
  const clean = (a) => a.replace(/[.,;:!?)»\]]+$/u, '')
  const urls = [...String(text).matchAll(/https?:\/\/[^\s)»"'<>\]]+/giu)].map((m) => clean(m[0]))
  const mails = [...String(text).matchAll(/[\w.+-]+@[\w-]+(?:\.[\w-]+)+/gu)].map((m) => clean(m[0]))
  return [...new Set([...urls, ...mails])]
}

/**
 * Juge une réponse. `got` : { text, sources: [{ id, doc, section }], replies: { items, ticket } | null }.
 * Rend la liste des contrôles : { name, ok, detail }.
 * @param {object} q          la question de questions.json
 * @param {object} got        ce que le moteur a répondu
 * @param {string} allowed    tout le texte de la base (pour reconnaître les adresses inventées)
 * @param {{plumbing?: boolean}} mode
 */
export function judge(q, got, allowed, { plumbing = false } = {}) {
  const e = q.expect || {}
  const checks = []
  const add = (name, ok, detail = '') => checks.push({ name, ok: !!ok, detail })
  const answer = fold(got.text)
  const cited = got.sources || []

  if (e.documents?.length) {
    const hit = cited.filter((s) => e.documents.includes(s.doc))
    add('source attendue citée', hit.length > 0, hit.length ? '' : `citées : ${[...new Set(cited.map((s) => s.doc))].join(', ') || 'aucune'} ; attendue : ${e.documents.join(', ')}`)
    if (e.sections?.length && !plumbing) {
      const bonne = hit.some((s) => e.sections.some((sec) => fold(s.section).includes(fold(sec))))
      add('bonne section citée', bonne, bonne ? '' : `sections citées : ${hit.map((s) => s.section).join(' | ') || 'aucune'}`)
    }
  }
  for (const f of e.facts || []) add(`dit « ${f} »`, answer.includes(fold(f)))
  for (const f of e.forbidden || []) add(`ne dit pas « ${f} »`, !answer.includes(fold(f)))
  if (e.no_sources) add('aucune source citée', cited.length === 0, cited.length ? `citées : ${cited.map((s) => s.doc).join(', ')}` : '')
  if (e.unknown && !(plumbing && cited.length)) add('dit ne pas savoir', saysUnknown(got.text))
  if (e.unknown || e.ticket) {
    const t = got.replies?.ticket
    add('propose d\'ouvrir un ticket', !!t, t ? '' : `boutons : ${got.replies?.items?.join(' | ') || 'aucun'}`)
  }
  const invented = addressesIn(got.text).filter((a) => !allowed.includes(a.toLowerCase()))
  add('aucune adresse inventée', invented.length === 0, invented.join(', '))
  if (!got.text.trim()) add('a répondu', false, 'réponse vide')
  return checks
}

// ───────────────────────────── moteur

async function json(base, method, url, body, headers = {}) {
  const r = await fetch(base + url, {
    method,
    headers: body === undefined ? headers : { 'Content-Type': 'application/json', ...headers },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const data = await r.json().catch(() => null)
  if (!r.ok) throw new Error(`${method} ${url} : ${data?.message || r.status}`)
  return data
}

async function importCorpus(base, dir) {
  const files = fs.readdirSync(dir).filter((f) => fs.statSync(path.join(dir, f)).isFile()).sort()
  const outcomes = []
  for (const f of files) {
    const r = await fetch(`${base}/api/live/kb/documents?name=${encodeURIComponent(f)}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/octet-stream' },
      body: fs.readFileSync(path.join(dir, f)),
    })
    const data = await r.json().catch(() => ({}))
    if (!r.ok || data.status === 'failed') throw new Error(`import de ${f} : ${data.error || data.message || r.status}`)
    outcomes.push({ name: f, status: data.status })
  }
  return outcomes
}

/** Tout le texte de la base, en minuscules : de quoi reconnaître une adresse que le modèle n'a pas inventée. */
async function knowledgeText(base) {
  const { documents } = await json(base, 'GET', '/api/live/kb/documents')
  let text = ''
  for (const d of documents) {
    const full = await json(base, 'GET', `/api/live/kb/documents/${d.id}`)
    text += `\n${full.document.title}\n${full.passages.map((p) => p.description).join('\n')}`
  }
  return { text: text.toLowerCase(), documents }
}

class Conversation {
  constructor(ws) {
    this.ws = ws
    this.events = []
    this.waiters = []
    ws.addEventListener('message', (m) => {
      if (typeof m.data !== 'string') return
      this.events.push(JSON.parse(m.data))
      for (const w of this.waiters.splice(0)) w()
    })
  }

  static open(url) {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url)
      const c = new Conversation(ws)
      ws.addEventListener('open', () => resolve(c))
      ws.addEventListener('error', () => reject(new Error(`connexion impossible à ${url}`)))
    })
  }

  send(o) { this.ws.send(JSON.stringify(o)) }

  async waitFor(from, pred, seconds) {
    const end = Date.now() + seconds * 1000
    for (;;) {
      const i = this.events.findIndex((e, k) => k >= from && pred(e))
      if (i >= 0) return i
      const left = end - Date.now()
      if (left <= 0) throw new Error(`pas de réponse du moteur en ${seconds} s`)
      await new Promise((resolve) => {
        const t = setTimeout(resolve, Math.min(left, 250))
        this.waiters.push(() => { clearTimeout(t); resolve() })
      })
    }
  }

  /** Envoie un message et attend la fin de la réponse. Rend les événements de ce tour. */
  async say(text, seconds) {
    const from = this.events.length
    this.send({ type: 'user', text })
    const end = await this.waitFor(from, (e) => e.type === 'done' || e.type === 'error', seconds)
    return this.events.slice(from, end + 1)
  }

  close() { try { this.ws.close() } catch { /* déjà fermée */ } }
}

/** Ce que le moteur a répondu pendant un tour, de la forme attendue par `judge`. */
export function gather(events, docName) {
  const err = events.find((e) => e.type === 'error')
  if (err) throw new Error(err.message)
  const text = events.filter((e) => e.type === 'token').map((e) => e.text).join('')
  const sources = []
  for (const e of events) {
    if (e.type === 'source' && !sources.some((s) => s.id === e.source.id)) {
      sources.push({ id: e.source.id, doc: docName(e.source.doc_id), section: e.source.section || '', title: e.source.title })
    }
  }
  const replies = [...events].reverse().find((e) => e.type === 'replies') || null
  return { text, sources, replies }
}

async function ask(base, persona, q, docName, seconds) {
  const wsUrl = base.replace(/^http/, 'ws') + '/api/live/ws'
  const conv = await Conversation.open(wsUrl)
  try {
    conv.send({ type: 'start', persona, tts: false, resume: false, user_name: 'Évaluation' })
    // l'ouverture de la conversation (message d'accueil) se termine par « done »
    await conv.waitFor(0, (e) => e.type === 'done' || e.type === 'error', seconds)
    for (const t of q.turns || []) if (t.role === 'user') await conv.say(t.text, seconds)
    const events = await conv.say(q.question, seconds)
    return gather(events, docName)
  } finally {
    conv.close()
  }
}

// ───────────────────────────── programme

const HELP = fs.readFileSync(fileURLToPath(import.meta.url), 'utf8').split('\n').filter((l) => l.startsWith('//')).slice(0, 14).map((l) => l.slice(3)).join('\n')

async function main() {
  const opt = parseArgs(process.argv.slice(2))
  if (opt.help) { console.log(HELP); return 0 }
  if (typeof WebSocket === 'undefined') { console.error('Node 22 ou plus récent est nécessaire (WebSocket intégré).'); return 2 }

  const base = opt.engine
  const questions = JSON.parse(fs.readFileSync(path.join(HERE, 'questions.json'), 'utf8')).questions
  const selected = questions.filter((q) => !opt.only.length || opt.only.includes(q.id))
  if (!selected.length) { console.error('Aucune question ne correspond à --only.'); return 2 }

  const health = await json(base, 'GET', '/api/live/health').catch((e) => { throw new Error(`moteur injoignable (${base}) : ${e.message}`) })
  if (!health.llm) throw new Error("l'IA n'est pas prête sur ce moteur (aucun modèle de texte chargé) : l'évaluation ne peut pas répondre")
  const personas = await json(base, 'GET', '/api/live/personas')
  const persona = personas.find((p) => p.id === opt.persona || p.name === opt.persona) || (opt.persona ? null : personas[0])
  if (!persona) throw new Error(`profil d'assistant introuvable : ${opt.persona || '(aucun profil)'}`)

  const corpusDir = path.join(HERE, 'corpus')
  const corpusFiles = fs.readdirSync(corpusDir).filter((f) => fs.statSync(path.join(corpusDir, f)).isFile())
  const before = (await json(base, 'GET', '/api/live/kb/documents')).documents
  const foreign = before.filter((d) => !corpusFiles.includes(d.name))
  if (foreign.length && !opt.force) {
    throw new Error(`la base contient ${foreign.length} document(s) qui ne sont pas ceux du corpus d'évaluation (${foreign.slice(0, 3).map((d) => d.name).join(', ')}…). ` +
      "Lancez l'évaluation sur un moteur dédié à base vide (voir eval/README.md), ou --force pour passer outre.")
  }

  console.log(`Moteur ${base} · profil « ${persona.name} » · ${opt.plumbing ? 'contrôle de la chaîne (faux modèle)' : 'évaluation avec le modèle chargé'}`)
  const imported = await importCorpus(base, corpusDir)
  const tally = imported.reduce((t, o) => ({ ...t, [o.status]: (t[o.status] || 0) + 1 }), {})
  console.log(`Corpus : ${imported.length} documents (${Object.entries(tally).map(([k, v]) => `${v} ${k}`).join(', ')})`)
  const { text: allowed, documents } = await knowledgeText(base)
  const docName = (id) => documents.find((d) => d.id === id)?.name || '?'

  const results = []
  for (const q of selected) {
    const skipped = opt.plumbing && q.expect?.judgment
    if (skipped) { results.push({ id: q.id, category: q.category, question: q.question, skipped: true, checks: [] }); console.log(`– ${q.id} ${q.category.padEnd(22)} laissée de côté (jugement du modèle)`); continue }
    const t0 = Date.now()
    let got
    let failure = ''
    try { got = await ask(base, persona.id, q, docName, opt.timeout) } catch (e) { failure = e.message }
    const checks = got ? judge(q, got, allowed, { plumbing: opt.plumbing }) : [{ name: 'a répondu', ok: false, detail: failure }]
    const ok = checks.every((c) => c.ok)
    results.push({ id: q.id, category: q.category, question: q.question, ok, seconds: (Date.now() - t0) / 1000, answer: got?.text ?? '', sources: got?.sources ?? [], replies: got?.replies?.items ?? [], checks })
    console.log(`${ok ? '✓' : '✗'} ${q.id} ${q.category.padEnd(22)} ${q.question}`)
    for (const c of checks.filter((c) => !c.ok)) console.log(`    ✗ ${c.name}${c.detail ? ` — ${c.detail}` : ''}`)
  }

  const judged = results.filter((r) => !r.skipped)
  const passed = judged.filter((r) => r.ok).length
  const score = judged.length ? passed / judged.length : 0
  const byCategory = {}
  for (const r of judged) { const c = (byCategory[r.category] ||= { ok: 0, n: 0 }); c.n++; if (r.ok) c.ok++ }
  console.log(`\nScore : ${passed}/${judged.length} (${Math.round(score * 100)} %)` + (results.length > judged.length ? ` — ${results.length - judged.length} question(s) à juger avec un vrai modèle` : ''))
  for (const [c, v] of Object.entries(byCategory)) console.log(`  ${c.padEnd(24)} ${v.ok}/${v.n}`)

  const dir = path.join(HERE, 'resultats')
  fs.mkdirSync(dir, { recursive: true })
  const file = path.join(dir, `${new Date().toISOString().replace(/[:.]/g, '-')}.json`)
  fs.writeFileSync(file, JSON.stringify({ date: new Date().toISOString(), engine: base, persona: persona.id, plumbing: opt.plumbing, score, passed, total: judged.length, results }, null, 2))
  console.log(`Détail (réponses comprises) : ${path.relative(process.cwd(), file)}`)

  const min = opt.min ?? (opt.plumbing ? 1 : 0)
  return score >= min ? 0 : 1
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().then((code) => process.exit(code), (e) => { console.error(`Erreur : ${e.message}`); process.exit(2) })
}
