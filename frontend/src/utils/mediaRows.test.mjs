import test from 'node:test'
import assert from 'node:assert/strict'
import { EDITABLE, parseTags, baseOf, rowFromServer, changes, mergeReloaded } from './mediaRows.js'

const server = (extra = {}) => ({ id: 7, title: 'capture12', description: '', tags: [], kind: 'photo', url: '/photos/capture12.png', thumb: null, analyzed: false, ...extra })

test('une ligne issue du moteur : tags en texte, version connue du moteur', () => {
  const row = rowFromServer(server({ tags: ['vpn', 'erreur 809'] }))
  assert.equal(row.tagsText, 'vpn, erreur 809')
  assert.deepEqual(row._base, { title: 'capture12', description: '', tagsText: 'vpn, erreur 809' })
  assert.equal(row._open, false)
  assert.equal(rowFromServer(server({ tags: undefined })).tagsText, '', 'sans tags')
})

test('les tags : séparés par des virgules, espaces et vides écartés', () => {
  assert.deepEqual(parseTags(' vpn ,, wifi , '), ['vpn', 'wifi'])
  assert.deepEqual(parseTags('VPN, '), ['VPN'])
  assert.deepEqual(parseTags(''), [])
  assert.deepEqual(parseTags(undefined), [])
})

test('changes : seuls les champs modifiés sont envoyés, et retenus comme version connue', () => {
  const row = rowFromServer(server())
  assert.deepEqual(changes(row), { patch: {}, sent: {} }, 'rien n\'a changé')
  row.title = 'Erreur VPN'
  assert.deepEqual(changes(row), { patch: { title: 'Erreur VPN' }, sent: { title: 'Erreur VPN' } }, 'le titre seul : pas la description (qu\'une analyse peut écrire)')
  row.tagsText = 'VPN, Erreur 809, '
  row.description = 'Le client VPN affiche l\'erreur 809.'
  assert.deepEqual(changes(row), {
    patch: { title: 'Erreur VPN', description: 'Le client VPN affiche l\'erreur 809.', tags: ['VPN', 'Erreur 809'] },
    sent: { title: 'Erreur VPN', description: 'Le client VPN affiche l\'erreur 809.', tagsText: 'VPN, Erreur 809, ' },
  }, 'les tags partent en liste, mais c\'est le texte tapé que l\'on retient')
  // une fois envoyé, plus rien à envoyer
  Object.assign(row._base, changes(row).sent)
  assert.deepEqual(changes(row), { patch: {}, sent: {} })
})

test('liste rechargée : une ligne que l\'on ne touche pas montre la version du moteur (normalisée) et reste dépliée', () => {
  const old = rowFromServer(server({ tags: ['vpn'] }))
  old._open = true
  old.tagsText = 'VPN, '; old._base.tagsText = 'VPN, ' // tapé puis enregistré tel quel
  const fresh = rowFromServer(server({ tags: ['vpn'], analyzed: true }))
  const row = mergeReloaded(fresh, old, { focused: false, dirty: false, lastActivity: 100 }, 200)
  assert.equal(row.tagsText, 'vpn', 'normalisé par le moteur : minuscules, virgule et espace nettoyés')
  assert.equal(row.analyzed, true)
  assert.equal(row._open, true, 'la ligne dépliée le reste')
  assert.equal(mergeReloaded(rowFromServer(server()), undefined).title, 'capture12', 'une ligne nouvelle : telle quelle')
})

test('liste rechargée : le curseur est dans la ligne — rien de ce qui est à l\'écran ne bouge (le texte « normalisé » changerait sous les doigts)', () => {
  const old = rowFromServer(server())
  old.tagsText = 'VPN, Wifi, '; old._base.tagsText = 'VPN, Wifi, ' // enregistré : le moteur a gardé vpn,wifi
  const fresh = rowFromServer(server({ tags: ['vpn', 'wifi'], analyzed: true, thumb: '/thumbnails/capture12.png' }))
  const row = mergeReloaded(fresh, old, { focused: true, dirty: false, lastActivity: 100 }, 200)
  assert.equal(row.tagsText, 'VPN, Wifi, ')
  assert.deepEqual(row._base, old._base, 'la version connue du moteur est celle d\'avant : seul ce qui en diffère sera renvoyé')
  assert.equal(row.analyzed, true, 'ce qui n\'est pas modifiable à la main vient du moteur')
  assert.equal(row.thumb, '/thumbnails/capture12.png')
  assert.deepEqual(changes(row), { patch: {}, sent: {} }, 'et la ligne n\'est pas considérée comme modifiée')
})

test('liste rechargée : une modification pas encore enregistrée, ou un enregistrement en cours ou en échec, est conservé', () => {
  const old = rowFromServer(server({ title: 'ancien' }))
  old.title = 'nouveau titre' // tapé, pas encore enregistré
  assert.equal(mergeReloaded(rowFromServer(server({ title: 'ancien' })), old).title, 'nouveau titre', 'champ qui diffère de ce que le moteur connaît')
  const saved = rowFromServer(server({ title: 'ancien' }))
  saved.title = 'nouveau'; saved._base.title = 'nouveau' // l\'enregistrement a abouti…
  assert.equal(mergeReloaded(rowFromServer(server({ title: 'ancien' })), saved, { dirty: true }).title, 'nouveau', '…mais l\'enregistreur a encore du travail (modifié depuis)')
  assert.equal(mergeReloaded(rowFromServer(server({ title: 'ancien' })), saved, { dirty: false }).title, 'ancien', 'tout enregistré, pas de curseur : version du moteur')
})

test('liste rechargée : une réponse demandée avant la frappe ou avant l\'enregistrement est périmée', () => {
  const old = rowFromServer(server({ title: 'ancien' }))
  old.title = 'Titre tout frais'; old._base.title = 'Titre tout frais' // enregistré pendant que la liste était demandée
  const stale = () => rowFromServer(server({ title: 'ancien' })) // l\'état du moteur d\'avant l\'enregistrement
  assert.equal(mergeReloaded(stale(), old, { lastActivity: 500 }, 400).title, 'Titre tout frais', 'activité (enregistrement) après la demande : on garde')
  assert.equal(mergeReloaded(stale(), old, { lastActivity: 300 }, 400).title, 'ancien', 'activité avant la demande : le moteur la connaît déjà')
})

test('EDITABLE : les trois champs que l\'on modifie à la main', () => {
  assert.deepEqual(EDITABLE, ['title', 'description', 'tagsText'])
  assert.deepEqual(Object.keys(baseOf(rowFromServer(server()))), EDITABLE)
})
