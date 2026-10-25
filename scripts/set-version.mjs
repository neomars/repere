// Écrit la version d'une release (« v1.2.3 » ou « 1.2.3 ») dans package.json et live-engine/Cargo.toml :
// elle nomme le paquet (repere-1.2.3-amd64.deb) et s'affiche dans l'application.
//   node scripts/set-version.mjs v1.2.3
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const version = (process.argv[2] || '').trim().replace(/^v/, '')
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`Version invalide : « ${process.argv[2] ?? ''} » (attendu : 1.2.3, v1.2.3 ou 1.2.3-beta.1)`)
  process.exit(2)
}

const pkgPath = join(root, process.env.SET_VERSION_ROOT_PKG || 'package.json')
const pkg = JSON.parse(readFileSync(pkgPath, 'utf8'))
pkg.version = version
writeFileSync(pkgPath, JSON.stringify(pkg, null, 2) + '\n')

const cargoPath = join(root, process.env.SET_VERSION_CARGO || 'live-engine/Cargo.toml')
const cargo = readFileSync(cargoPath, 'utf8')
const updated = cargo.replace(/^version = "[^"]*"/m, `version = "${version}"`) // la première : celle de [package]
if (updated === cargo && !cargo.includes(`version = "${version}"`)) {
  console.error('version introuvable dans Cargo.toml')
  process.exit(1)
}
writeFileSync(cargoPath, updated)
console.log(`Version ${version} écrite dans package.json et Cargo.toml`)
