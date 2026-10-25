// Enregistrement automatique d'un enregistrement que l'on modifie à la main (une ligne de la médiathèque, par exemple).
// Chaque modification (`touch`) programme un envoi après un court moment sans nouvelle frappe ; `flush` l'envoie tout de suite
// (sortie du champ, changement de page). Deux envois du même enregistrement ne se chevauchent jamais, et une modification faite
// pendant un envoi est envoyée ensuite : le dernier mot est toujours celui de l'utilisateur. Pure logique (pas de DOM) : testée par `npm test`.
//
// États : idle (rien n'a changé) · pending (des modifications attendent leur envoi) · saving (un envoi est en cours) · saved · error.

export class Autosaver {
  /**
   * @param {() => Promise<void>} save  envoie ce qui a changé ; rejette si l'enregistrement échoue (le message de l'erreur est affiché)
   * @param {{ delay?: number, onState?: (state: string, error: string) => void, timers?: { set: Function, clear: Function }, now?: () => number }} [opts]
   *   `timers` et `now` : minuteries et horloge (celles du navigateur par défaut), remplaçables pour les tests.
   */
  constructor(save, { delay = 700, onState = () => {}, timers, now = () => Date.now() } = {}) {
    this._save = save
    this.delay = delay
    this._onState = onState
    this._now = now
    this._timers = timers || { set: (f, ms) => setTimeout(f, ms), clear: (t) => clearTimeout(t) }
    /** Dernier instant où l'utilisateur a modifié, ou où un envoi s'est terminé : une réponse du serveur demandée avant n'en tient pas compte. */
    this.lastActivity = 0
    this._rev = 0 // numéro de la dernière modification
    this._savedRev = 0 // numéro de la dernière modification dont l'envoi a abouti
    this._timer = null
    this._chain = Promise.resolve(true) // les envois se suivent : jamais deux à la fois
    this.state = 'idle'
    this.error = ''
  }

  /** Des modifications n'ont pas encore été enregistrées (en attente, en cours d'envoi, ou en échec). */
  get dirty() { return this._rev > this._savedRev }

  /** Une modification vient d'avoir lieu : elle sera envoyée dans `delay` ms, sauf nouvelle modification d'ici là. */
  touch() {
    this._rev++
    this.lastActivity = this._now()
    this._set('pending')
    this._timers.clear(this._timer)
    this._timer = this._timers.set(() => { this._timer = null; this.flush() }, this.delay)
  }

  /** Envoie tout de suite ce qui attend. Résout `true` quand plus rien n'est en attente, `false` après un échec. Ne rejette jamais. */
  flush() {
    this._timers.clear(this._timer)
    this._timer = null
    this._chain = this._chain.then(() => this._once())
    return this._chain
  }

  async _once() {
    if (!this.dirty) return true
    const rev = this._rev
    this._set('saving')
    try {
      await this._save()
    } catch (e) {
      this.lastActivity = this._now()
      this._set('error', (e && e.message) || String(e))
      return false // pas de nouvel essai tout seul : il part à la prochaine modification, ou à la demande (flush)
    }
    this.lastActivity = this._now()
    this._savedRev = rev
    if (this.dirty) { this._set('pending'); return false } // modifié pendant l'envoi : la minuterie s'en occupe
    this._set('saved')
    return true
  }

  _set(state, error = '') {
    this.state = state
    this.error = error
    this._onState(state, error)
  }
}
