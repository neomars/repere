import test from 'node:test'
import assert from 'node:assert/strict'
import { Autosaver } from './autosave.js'

/** Minuteries à la main : le temps n'avance que quand le test le décide. */
function manualTimers() {
  let next = 1
  const pending = new Map()
  return {
    set: (f) => { const id = next++; pending.set(id, f); return id },
    clear: (id) => { pending.delete(id) },
    count: () => pending.size,
    /** Fait sonner toutes les minuteries programmées. */
    fire() { const fs = [...pending.values()]; pending.clear(); fs.forEach((f) => f()) },
  }
}

/** Un envoi que le test termine (ou fait échouer) quand il veut. */
function manualSave() {
  const calls = []
  const save = () => new Promise((resolve, reject) => calls.push({ resolve, reject }))
  return { save, calls }
}

const tick = () => new Promise((r) => setImmediate(r))

test('plusieurs modifications rapprochées : un seul envoi, après le délai', async () => {
  const timers = manualTimers()
  let sent = 0
  const a = new Autosaver(async () => { sent++ }, { timers })
  a.touch(); a.touch(); a.touch()
  assert.equal(a.state, 'pending')
  assert.equal(timers.count(), 1, 'une seule minuterie : chaque frappe repousse l\'envoi')
  assert.equal(sent, 0, 'rien n\'est envoyé avant le délai')
  timers.fire()
  await a.flush()
  assert.equal(sent, 1)
  assert.equal(a.state, 'saved')
  assert.equal(a.dirty, false)
})

test('flush envoie tout de suite (sortie du champ) et ne fait rien quand tout est enregistré', async () => {
  const timers = manualTimers()
  let sent = 0
  const a = new Autosaver(async () => { sent++ }, { timers })
  a.touch()
  assert.equal(await a.flush(), true)
  assert.equal(sent, 1)
  assert.equal(timers.count(), 0, 'la minuterie en attente est annulée')
  assert.equal(await a.flush(), true)
  assert.equal(sent, 1, 'rien de nouveau : pas de second envoi')
  assert.equal(new Autosaver(async () => { sent++ }, { timers }).state, 'idle')
})

test('deux envois ne se chevauchent jamais ; une modification faite pendant un envoi est envoyée ensuite', async () => {
  const timers = manualTimers()
  const { save, calls } = manualSave()
  const states = []
  const a = new Autosaver(save, { timers, onState: (s) => states.push(s) })
  a.touch()
  const first = a.flush()
  await tick()
  assert.equal(calls.length, 1)
  assert.equal(a.state, 'saving')
  a.touch() // l'utilisateur continue d'écrire pendant l'envoi
  assert.equal(a.state, 'pending')
  const second = a.flush() // et quitte le champ : un second envoi est demandé
  await tick()
  assert.equal(calls.length, 1, 'le second attend la fin du premier')
  calls[0].resolve()
  await first
  assert.equal(a.dirty, true, 'la modification faite pendant l\'envoi n\'est pas encore enregistrée')
  await tick()
  assert.equal(calls.length, 2, 'puis le second part')
  calls[1].resolve()
  assert.equal(await second, true)
  assert.equal(a.dirty, false)
  assert.equal(a.state, 'saved')
  assert.deepEqual(states, ['pending', 'saving', 'pending', 'pending', 'saving', 'saved'])
})

test('une modification faite pendant un envoi et sans flush part grâce à la minuterie', async () => {
  const timers = manualTimers()
  const { save, calls } = manualSave()
  const a = new Autosaver(save, { timers })
  a.touch()
  timers.fire()
  await tick()
  a.touch() // pendant l'envoi
  calls[0].resolve()
  await tick()
  assert.equal(a.state, 'pending')
  assert.equal(timers.count(), 1, 'la minuterie de la seconde modification est en attente')
  timers.fire()
  await tick()
  assert.equal(calls.length, 2)
  calls[1].resolve()
  await tick()
  assert.equal(a.state, 'saved')
})

test('échec : l\'erreur est signalée, rien n\'est perdu, et un nouvel essai part à la demande ou à la modification suivante', async () => {
  const timers = manualTimers()
  let fail = true
  let sent = 0
  const a = new Autosaver(async () => { if (fail) throw new Error('moteur injoignable'); sent++ }, { timers })
  a.touch()
  assert.equal(await a.flush(), false)
  assert.equal(a.state, 'error')
  assert.equal(a.error, 'moteur injoignable')
  assert.equal(a.dirty, true, 'la modification reste à enregistrer')
  assert.equal(timers.count(), 0, 'pas de boucle de nouveaux essais')
  fail = false
  assert.equal(await a.flush(), true, 'Réessayer')
  assert.equal(sent, 1)
  assert.equal(a.state, 'saved')
  assert.equal(a.error, '')

  fail = true
  a.touch()
  await a.flush()
  assert.equal(a.state, 'error')
  fail = false
  a.touch() // une nouvelle frappe relance l'enregistrement
  assert.equal(a.state, 'pending')
  timers.fire()
  await a.flush()
  assert.equal(a.state, 'saved')
  assert.equal(sent, 2)
})

test('une erreur sans message (rejet d\'une valeur quelconque) est quand même affichable', async () => {
  const a = new Autosaver(() => Promise.reject('refusé'), { timers: manualTimers() })
  a.touch()
  await a.flush()
  assert.equal(a.error, 'refusé')
})

test('lastActivity : l\'instant de la dernière modification ou de la fin d\'un envoi (une réponse du serveur plus ancienne est périmée)', async () => {
  let t = 100
  const a = new Autosaver(async () => {}, { timers: manualTimers(), now: () => t })
  assert.equal(a.lastActivity, 0, 'rien ne s\'est passé')
  t = 150; a.touch()
  assert.equal(a.lastActivity, 150, 'une modification')
  t = 400; await a.flush()
  assert.equal(a.lastActivity, 400, 'la fin d\'un envoi réussi')
  const b = new Autosaver(async () => { throw new Error('non') }, { timers: manualTimers(), now: () => t })
  t = 500; b.touch(); t = 650; await b.flush()
  assert.equal(b.lastActivity, 650, 'la fin d\'un envoi en échec aussi')
})
