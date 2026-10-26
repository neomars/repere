// Tests de la logique de la base de connaissances et des sources citées : node --test src/utils/kb.test.mjs
import test from 'node:test'
import assert from 'node:assert/strict'
import {
  ACCEPT, addSource, adminHeaders, errorText, folderTags, formatBytes, formatLabel, isReadable, isTicketReply, outcomeView,
  parseTags, safeHref, sourceLabel, sourcePath, ticketText, uploadName, uploadQuery,
} from './kb.js'

test('les formats ont un nom lisible, les inconnus restent affichables', () => {
  assert.equal(formatLabel('docx'), 'Word')
  assert.equal(formatLabel('pdf'), 'PDF')
  assert.equal(formatLabel('xyz'), 'XYZ')
  assert.equal(formatLabel(undefined), '?')
})

test('seules les extensions lues par le moteur sont retenues', () => {
  for (const n of ['guide.md', 'GUIDE.PDF', 'a/b/page.html', 'note.txt', 'rapport.docx', 'x.markdown']) assert.ok(isReadable(n), n)
  for (const n of ['photo.png', 'tableur.xlsx', 'diapo.pptx', 'Makefile', '']) assert.ok(!isReadable(n), n)
  assert.match(ACCEPT, /\.docx/)
  assert.match(ACCEPT, /\.pdf/)
})

test('les tailles sont écrites à la française', () => {
  assert.equal(formatBytes(0), '0 o')
  assert.equal(formatBytes(900), '900 o')
  assert.equal(formatBytes(1536), '1,5 Ko')
  assert.equal(formatBytes(5 * 1024 * 1024), '5 Mo')
  assert.equal(formatBytes(2.5 * 1024 ** 3), '2,5 Go')
  assert.equal(formatBytes(undefined), '0 o')
})

test("un lien n'est ouvert que s'il est en http ou https", () => {
  assert.equal(safeHref('https://intranet.exemple.fr/vpn'), 'https://intranet.exemple.fr/vpn')
  assert.equal(safeHref('  http://a.fr/x  '), 'http://a.fr/x')
  for (const u of ['javascript:alert(1)', 'data:text/html,<b>', 'file:///etc/passwd', '//evil.example', 'ftp://x', '', null, undefined, 'https://a b']) {
    assert.equal(safeHref(u), '', String(u))
  }
})

test('une source citée se reconnaît à son titre et au dernier niveau de sa section', () => {
  assert.equal(sourceLabel({ title: 'Client VPN', section: 'Erreur 809' }), 'Client VPN › Erreur 809')
  assert.equal(sourceLabel({ title: 'Client VPN : installation, connexion et dépannage', section: 'Les erreurs courantes du client VPN › Erreur 809' }), 'Client VPN : installation, connexion et dépannage › Erreur 809')
  assert.equal(sourceLabel({ title: 'Wi-Fi' }), 'Wi-Fi')
  assert.equal(sourceLabel({ title: '' }), 'Document')
  assert.equal(sourceLabel(null), '')
  // quand la place manque, c'est le titre du document qui est raccourci, pas la fin du chemin
  const long = sourceLabel({ title: 'Guide complet de la messagerie et de la collaboration', section: 'Partager son calendrier' }, 40)
  assert.ok(long.length <= 40 && long.endsWith('› Partager son calendrier'), long)
  assert.ok(long.startsWith('Guide'), long)
  assert.ok(sourceLabel({ title: 'T'.repeat(80) }, 30).length <= 30)
})

test('le chemin complet garde tous les niveaux et la page', () => {
  assert.equal(sourcePath({ title: 'Télétravail', section: 'La sécurité', page: 3 }), 'Télétravail › La sécurité (p. 3)')
  assert.equal(sourcePath({ title: 'Client VPN', section: 'Les erreurs › Erreur 809' }), 'Client VPN › Les erreurs › Erreur 809')
  assert.equal(sourcePath({ title: '' }), 'Document')
  assert.equal(sourcePath(null), '')
})

test("une source n'est ajoutée qu'une fois à la réponse", () => {
  const a = { id: 1, title: 'A' }
  const b = { id: 2, title: 'B' }
  let list = addSource(undefined, a)
  list = addSource(list, b)
  assert.deepEqual(list.map((s) => s.id), [1, 2])
  assert.equal(addSource(list, { id: 1, title: 'A bis' }), list, 'même numéro : la liste ne change pas')
  assert.equal(addSource(list, null), list)
})

test("le résultat d'un envoi de document est dit clairement", () => {
  assert.deepEqual(outcomeView({ status: 'added', passages: 1, replaced: false }), { tone: 'ok', text: 'Ajouté (1 passage)' })
  assert.deepEqual(outcomeView({ status: 'added', passages: 12, replaced: true }), { tone: 'ok', text: 'Remplacé (12 passages)' })
  assert.equal(outcomeView({ status: 'unchanged', id: 3, title: 'x' }).tone, 'same')
  assert.deepEqual(outcomeView({ status: 'duplicate', of: 3, title: 'Guide VPN' }), { tone: 'same', text: 'Même contenu que « Guide VPN » : ignoré' })
  assert.deepEqual(outcomeView({ status: 'failed', error: 'format non pris en charge' }), { tone: 'bad', text: 'format non pris en charge' })
  assert.equal(outcomeView(null).tone, 'bad')
})

test('un fichier de dossier garde son chemin, les dossiers deviennent des étiquettes', () => {
  assert.equal(uploadName({ name: 'a.md', webkitRelativePath: 'Guides/VPN/a.md' }), 'Guides/VPN/a.md')
  assert.equal(uploadName({ name: 'a.md', webkitRelativePath: '' }), 'a.md')
  assert.equal(uploadName({ name: 'a.md' }), 'a.md')
  assert.deepEqual(folderTags('Guides/VPN/client.docx'), ['Guides', 'VPN'])
  assert.deepEqual(folderTags('client.docx'), [])
  assert.deepEqual(folderTags(''), [])
})

test("les étiquettes saisies et l'adresse d'envoi sont propres", () => {
  assert.deepEqual(parseTags(' vpn, réseau ,, '), ['vpn', 'réseau'])
  assert.deepEqual(parseTags(undefined), [])
  assert.equal(uploadQuery('guide.md'), 'name=guide.md')
  assert.equal(uploadQuery('Guides/Accès & droits.md', ['a', 'b']), 'name=Guides%2FAcc%C3%A8s+%26+droits.md&tags=a%2Cb')
})

test('le ticket se copie en texte, avec sa référence quand il en a une', () => {
  assert.equal(ticketText({ title: 'VPN erreur 809', description: ' Le client VPN échoue.\n\nDemandeur : Marc ' }), 'VPN erreur 809\n\nLe client VPN échoue.\n\nDemandeur : Marc')
  assert.equal(ticketText({ reference: 'T-0012', title: 'VPN', description: 'x' }), 'T-0012 — VPN\n\nx')
})

test("seule la réponse rapide qui porte l'intitulé du ticket l'ouvre", () => {
  assert.ok(isTicketReply('Ouvrir un ticket', 'Ouvrir un ticket'))
  assert.ok(!isTicketReply('Reformuler', 'Ouvrir un ticket'))
  assert.ok(!isTicketReply('Ouvrir un ticket', null))
  assert.ok(!isTicketReply('Ouvrir un ticket', ''))
})

test("sans navigateur, pas d'en-tête d'administration et pas d'erreur", () => {
  assert.deepEqual(adminHeaders(), {})
  const saved = globalThis.localStorage
  globalThis.localStorage = { getItem: () => 'jeton' }
  assert.deepEqual(adminHeaders(), { Authorization: 'Bearer jeton' })
  globalThis.localStorage = { getItem: () => { throw new Error('bloqué') } }
  assert.deepEqual(adminHeaders(), {})
  if (saved === undefined) delete globalThis.localStorage
  else globalThis.localStorage = saved
})

test("le message d'erreur du moteur est préféré à celui du réseau", () => {
  assert.equal(errorText({ response: { data: { message: 'jeton admin requis' } }, message: 'Request failed with status code 401' }), 'jeton admin requis')
  assert.equal(errorText({ message: 'Network Error' }), 'Network Error')
  assert.equal(errorText(null), 'Erreur inconnue')
})
