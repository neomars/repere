// Dépendances de l'app empaquetée : node --test backend/packageDeps.test.js (aucun module à installer)
const test = require('node:test');
const assert = require('node:assert/strict');
const root = require('../package.json');
const backend = require('./package.json');

// Le paquet (.deb, AppImage) n'embarque PAS backend/node_modules (voir « build.files » du package.json racine) : le serveur y
// trouve ses modules parmi les dépendances de la racine. Une dépendance ajoutée au seul backend marche en développement, mais
// l'app installée ne démarre pas (« Cannot find module »).
test('chaque dépendance du backend est aussi une dépendance du package.json racine, à la même version', () => {
  const expected = Object.entries(backend.dependencies || {});
  const wrong = expected.filter(([name, version]) => root.dependencies?.[name] !== version);
  assert.deepEqual(
    wrong.map(([name, version]) => `${name}@${version}`),
    [],
    'à reporter dans les « dependencies » du package.json racine (même version que backend/package.json)'
  );
});
