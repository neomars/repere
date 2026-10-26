// Tests de l'outil d'évaluation (sans moteur ni modèle) : node --test eval/run.test.mjs
import test from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { addressesIn, fold, gather, judge, parseArgs, saysUnknown } from './run.mjs'
import { answer, offeredPassages, summary } from './tools/faux-llm.mjs'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const questions = JSON.parse(fs.readFileSync(path.join(HERE, 'questions.json'), 'utf8')).questions

test('le jeu compte 30 questions bien formées, avec les fichiers du corpus', () => {
  assert.equal(questions.length, 30)
  assert.equal(new Set(questions.map((q) => q.id)).size, 30, 'identifiants uniques')
  const corpus = new Set(fs.readdirSync(path.join(HERE, 'corpus')))
  for (const q of questions) {
    assert.ok(q.question.trim() && q.reference.trim() && q.category, q.id)
    assert.ok(q.expect && typeof q.expect === 'object', q.id)
    for (const d of q.expect.documents || []) assert.ok(corpus.has(d), `${q.id} : ${d} n'est pas dans eval/corpus`)
    for (const t of q.turns || []) assert.ok(['user', 'assistant'].includes(t.role) && t.text, q.id)
    const attendu = q.expect
    assert.ok(attendu.documents || attendu.unknown || attendu.facts || attendu.no_sources, `${q.id} n'attend rien`)
  }
})

test('chaque document du corpus sert à au moins une question', () => {
  const used = new Set(questions.flatMap((q) => q.expect.documents || []))
  const unused = fs.readdirSync(path.join(HERE, 'corpus')).filter((f) => !used.has(f))
  // Certains documents ne servent que de bruit (ils doivent être écartés) : on en tolère quelques-uns, pas la majorité.
  assert.ok(unused.length <= 6, `documents jamais attendus : ${unused.join(', ')}`)
})

test('les options de la ligne de commande', () => {
  const o = parseArgs(['--engine', 'http://moteur:3001/', '--only', 'q01, q07', '--plumbing', '--min', '0.8'])
  assert.deepEqual([o.engine, o.only, o.plumbing, o.min], ['http://moteur:3001', ['q01', 'q07'], true, 0.8])
  assert.throws(() => parseArgs(['--inconnue']), /option inconnue/)
  assert.throws(() => parseArgs(['--min', '2']), /entre 0 et 1/)
  assert.throws(() => parseArgs(['--engine']), /attend une valeur/)
})

test('« je ne sais pas » est reconnu sous ses formes courantes', () => {
  for (const t of ['Je ne sais pas : je n’ai pas trouvé cela dans la base de connaissances.', 'JE NE SAIS PAS', "Je n'ai pas trouvé de réponse", 'Aucune source ne parle de cela']) {
    assert.ok(saysUnknown(t), t)
  }
  for (const t of ['Voici la marche à suivre.', 'Je sais que le port est 4500.', '']) assert.ok(!saysUnknown(t), t)
  assert.equal(fold('  Éléments Supprimés  '), 'elements supprimes')
})

test('les adresses de la réponse sont relevées, sans la ponctuation de fin', () => {
  assert.deepEqual(addressesIn('Allez sur https://mdp.exemple.fr. Ou écrivez à support@exemple.fr, merci (https://a.fr/x).'), ['https://mdp.exemple.fr', 'https://a.fr/x', 'support@exemple.fr'])
  assert.deepEqual(addressesIn('Rien à signaler.'), [])
})

const q = (expect, extra = {}) => ({ id: 'qx', question: '?', expect, ...extra })
const got = (over = {}) => ({ text: 'ok', sources: [], replies: null, ...over })
const failed = (checks) => checks.filter((c) => !c.ok).map((c) => c.name)

test('une bonne réponse sourcée passe tous les contrôles', () => {
  const checks = judge(
    q({ documents: ['07-wifi.md'], facts: ['24 heures'] }),
    got({ text: "Demandez un code à l'accueil, valable 24 heures. https://mdp.exemple.fr", sources: [{ id: 1, doc: '07-wifi.md', section: 'Le réseau INVITES' }] }),
    'le code est valable 24 heures. https://mdp.exemple.fr/'.toLowerCase(),
  )
  assert.deepEqual(failed(checks), [])
})

test('une source manquante, une information absente et une adresse inventée sont relevées', () => {
  const checks = judge(
    q({ documents: ['03-vpn-client.docx'], facts: ['UDP'] }),
    got({ text: 'Essayez https://faux.exemple.fr', sources: [{ id: 2, doc: '07-wifi.md', section: '' }] }),
    'udp',
  )
  assert.deepEqual(failed(checks), ['source attendue citée', 'dit « UDP »', 'aucune adresse inventée'])
})

test("une section attendue n'est exigée qu'avec un vrai modèle", () => {
  const attendu = q({ documents: ['03-vpn-client.docx'], sections: ['809'] })
  const reponse = got({ sources: [{ id: 1, doc: '03-vpn-client.docx', section: 'Erreur 691' }] })
  assert.deepEqual(failed(judge(attendu, reponse, '')), ['bonne section citée'])
  assert.deepEqual(failed(judge(attendu, reponse, '', { plumbing: true })), [])
})

test("une question sans réponse exige le « je ne sais pas », aucune source et le bouton de ticket", () => {
  const attendu = q({ unknown: true, no_sources: true })
  assert.deepEqual(failed(judge(attendu, got({ text: 'Voici comment faire.', sources: [{ id: 1, doc: 'a', section: '' }] }), '')), ['aucune source citée', 'dit ne pas savoir', "propose d'ouvrir un ticket"])
  const bonne = got({ text: "Je ne sais pas : je n'ai pas trouvé cela.", replies: { items: ['Ouvrir un ticket', 'Reformuler'], ticket: 'Ouvrir un ticket' } })
  assert.deepEqual(failed(judge(attendu, bonne, '')), [])
})

test("un texte interdit ou une réponse vide échouent", () => {
  assert.deepEqual(failed(judge(q({ forbidden: ['Azerty2024'] }), got({ text: 'Votre mot de passe Azerty2024 est faible' }), '')), ['ne dit pas « Azerty2024 »'])
  assert.ok(failed(judge(q({}), got({ text: '  ' }), '')).includes('a répondu'))
})

test('gather rassemble le texte, les sources sans doublon et les boutons du tour', () => {
  const events = [
    { type: 'gen_start', gen: 1 },
    { type: 'token', gen: 1, text: 'Bon' }, { type: 'token', gen: 1, text: 'jour' },
    { type: 'source', gen: 1, source: { id: 7, doc_id: 3, section: 'S', title: 'T' } },
    { type: 'source', gen: 1, source: { id: 7, doc_id: 3, section: 'S', title: 'T' } },
    { type: 'replies', items: ['Ouvrir un ticket'], ticket: 'Ouvrir un ticket' },
    { type: 'done', gen: 1 },
  ]
  const r = gather(events, (id) => `doc${id}`)
  assert.equal(r.text, 'Bonjour')
  assert.deepEqual(r.sources, [{ id: 7, doc: 'doc3', section: 'S', title: 'T' }])
  assert.equal(r.replies.ticket, 'Ouvrir un ticket')
  assert.throws(() => gather([{ type: 'error', message: 'LLM : hors service' }], () => ''), /hors service/)
})

// ── le faux modèle

const SYSTEM = [
  'Tu es Repère.',
  'SOURCES DE LA BASE DE CONNAISSANCES (…) :',
  '',
  '#12 Extrait de « Client VPN › Erreur 809 », page 3 :',
  'Autorisez les ports UDP 500 et 4500.',
  'Relancez le client.',
  '',
  '#15 Extrait de « Wi-Fi » :',
  'Le code est valable 24 heures.',
  '',
  "- Appuie ta réponse sur ces sources. Si aucune ne répond à la question, dis que tu ne sais pas.",
].join('\n')

test('le faux modèle lit les passages proposés dans le prompt, sur plusieurs lignes', () => {
  const p = offeredPassages(SYSTEM)
  assert.deepEqual(p.map((x) => [x.id, x.place, x.page]), [[12, 'Client VPN › Erreur 809', 3], [15, 'Wi-Fi', null]])
  assert.equal(p[0].text, 'Autorisez les ports UDP 500 et 4500.\nRelancez le client.')
  assert.equal(p[1].text, 'Le code est valable 24 heures.')
})

test('le faux modèle cite ce qu\'on lui propose, et ne sait pas sinon', () => {
  const a = answer(SYSTEM, 'Que signifie 809 ?')
  assert.match(a, /UDP 500 et 4500\. Relancez le client\. \[\[source: #12\]\]/)
  assert.match(a, /\[\[source: #15\]\]/)
  assert.match(a, /\[\[replies: Ça a marché \| Ouvrir un ticket\]\]/)
  const rien = answer('Tu es Repère.\nSOURCES DE LA BASE DE CONNAISSANCES : aucune ne correspond à ce message.', 'Capitale de la Mongolie ?')
  assert.match(rien, /^Je ne sais pas/)
  assert.match(rien, /\[\[replies: Ouvrir un ticket \| Reformuler\]\]/)
  assert.equal(summary('Résumé précédent :\n\n\nNouveaux échanges :\nuser: Mon VPN échoue\nassistant: Quel code ?\n\nNouveau résumé :'), 'Résumé : Mon VPN échoue')
})
