const path = require('path');

// Dossier des données modifiables (db.json + uploads). Par défaut : le dossier backend (dev).
// L'app empaquetée (Electron/AppImage, lecture seule) fournit REPERE_DATA_DIR = dossier utilisateur.
const dataDir = process.env.REPERE_DATA_DIR || path.join(__dirname, '..');

module.exports = {
  dataDir,
  dbFile: path.join(dataDir, 'db.json'),
  uploadsDir: path.join(dataDir, 'uploads'),
};
