# Repère

Assistant de support informatique de **premier niveau (niveau 0)** pour une DSI. Un agent y décrit son problème par écrit ou à
voix haute ; Repère cherche la réponse dans la **base de connaissances interne**, la donne en étapes courtes, **cite ses sources**,
**dit quand il ne sait pas** et propose alors d'**ouvrir un ticket**. L'IA tourne **en local**, sur un GPU NVIDIA : aucune
question ne quitte le poste.

Application de bureau pour Ubuntu (Electron), avec un moteur d'IA en Rust, un serveur Express et une interface Vue 3.

## Où en est le projet

| Phase | Contenu | État |
|---|---|---|
| **1. Socle** | Conversation en direct (texte, micro, voix), règles de support, profil d'assistant, gestion des modèles, administration | **fait** (ce dépôt) |
| 2. Base de connaissances | Import de documents (Markdown, HTML, PDF, DOCX), découpage en passages, recherche, citation des sources (`[[source: #id]]`), ticket prérempli, jeu d'évaluation de 30 questions | à faire |
| 3. Déploiement | Connexion unique (OIDC), conversations par utilisateur, PostgreSQL, déploiement sur serveur, GLPI, Teams | à faire |

Ce que la phase 1 ne fait pas encore, et qu'il faut savoir :

- **Il n'y a pas encore de base de connaissances de documents.** L'assistant s'appuie sur les images décrites que le moteur sait
  importer et analyser (captures d'écran, schémas) ; sans source pertinente, il répond qu'il ne sait pas et propose un ticket.
- **Aucun modèle de texte n'est livré dans le catalogue** : un modèle s'ajoute par `catalog.json` (voir
  [`live-engine/README.md`](live-engine/README.md#ajouter-un-modèle-de-texte)). Sans modèle de texte chargé, l'assistant ne répond pas.
- La sécurité est celle d'un poste de travail local (voir [Sécurité](#sécurité--état-actuel)).

## Architecture

```
Electron (application Ubuntu)
 ├─ Express :3000        interface Vue, connexion, utilisateurs, médias servis
 │    └─ proxy /api/live (REST + WebSocket) ──┐
 └─ live-engine :3001    moteur Rust (axum) ◄──┘
      ├─ llama-server   (IA texte, CUDA)             modèles .gguf téléchargés depuis Hugging Face
      ├─ whisper-server (micro → texte)
      └─ tts-server     (voix de l'assistant, Chatterbox)   voix clonée depuis un échantillon
```

| Couche | Technologie |
|---|---|
| Interface | Vue 3 + Vite |
| Serveur web | Node.js + Express (sessions, utilisateurs) |
| Données du serveur web | fichier JSON (`db.json`, alasql) : une base PostgreSQL le remplacera en phase 3 |
| Moteur d'IA | Rust (axum, tokio, SQLite pour la base de médias) |
| Inférence | llama.cpp (texte), whisper.cpp (reconnaissance vocale), Chatterbox (voix) |
| Paquet | Electron + electron-builder (`.deb`, AppImage) |

Le fonctionnement du moteur (prompt, sources, directives, voix, profils, API) est décrit dans
[`live-engine/README.md`](live-engine/README.md).

## Installer

### Depuis GitHub (rien à compiler)

Le paquet `.deb` est construit par GitHub Actions (`.github/workflows/build.yml`) et joint à chaque release. Téléchargez
`repere-<version>-amd64.deb` dans les *Assets* de la release, puis :

```bash
sudo apt install ./repere-*-amd64.deb
```

Ubuntu 24.04 ou plus récent, GPU NVIDIA avec un pilote ≥ 570. Pour mettre à jour, installez la nouvelle version par-dessus : les
données restent dans `~/.config/repere`.

**Publier une version** : créez le tag `v1.2.3` sur le dernier commit de `main` (page *Releases* de GitHub, ou
`git tag v1.2.3 && git push origin v1.2.3`). La construction prend une trentaine de minutes (onglet *Actions* → *Build*) et le
`.deb` est joint à la release ; la version du paquet est celle du tag. Pour **essayer sans publier** : *Actions* → *Build* →
*Run workflow* en laissant « Tag de la release » vide (le `.deb` reste 1 jour dans les *Artifacts*). Les détails (release déjà
publiée sans paquet, quota d'artefacts) sont en tête de `.github/workflows/build.yml`.

### Compiler soi-même (Ubuntu + NVIDIA)

Prérequis : Ubuntu 24.04 (22.04 : `LLAMA_MODE=source`), pilote NVIDIA ≥ 570, Node ≥ 20.19, Rust ≥ 1.82, `build-essential cmake git
curl`, et le CUDA Toolkit (`nvcc`) seulement pour compiler `whisper-server` avec CUDA.

```bash
git clone https://github.com/neomars/repere && cd repere
npm run doctor           # diagnostic : liste ce qui manque (✓ / ! / ✗) sans rien modifier
npm run dist:linux       # build complet : AppImage + .deb dans ./dist
npm run pack:linux       # variante rapide : seulement dist/linux-unpacked, pour tester
```

`dist:linux` enchaîne : dépendances npm → interface Vue → moteur Rust (release) → `bin/` (llama-server officiel CUDA vérifié par
SHA-256, whisper-server compilé, scripts de voix) → electron-builder → `scripts/verify-package.sh`, qui contrôle le contenu du
paquet (moteur et serveurs présents ; **base locale, médias importés et `.env` absents**). Options : `--skip-install`,
`--skip-sidecars`, `--no-doctor`, `--dir` ; variables `CUDA_ARCH`, `SKIP_WHISPER=1`, `LLAMA_MODE=source` (voir l'en-tête de
`scripts/build-sidecars.sh`).

Notes :

- **AppImage** : construite avec le runtime statique d'electron-builder 26, qui n'exige pas `libfuse2`. Sur Ubuntu 23.10+,
  AppArmor interdit le bac à sable de Chromium aux AppImage : l'application le désactive (elle ne charge que sa propre page
  locale). Le `.deb` garde le bac à sable.
- **Paquet et dépendances** : le paquet n'embarque pas `backend/node_modules` ; le serveur charge ses modules depuis les
  `dependencies` du `package.json` **racine**. Toute dépendance ajoutée à `backend/package.json` doit l'être aussi à la racine, à la
  même version (`backend/packageDeps.test.js` le vérifie en CI).
- **Licences du paquet** : il embarque les bibliothèques CUDA de NVIDIA (`libcudart`, `libcublas`, `libcublasLt`) livrées avec le
  binaire llama.cpp officiel, et `ffmpeg-static` (GPL-3.0-or-later). Vérifiez ces licences avec la DSI avant toute diffusion.

### Première utilisation

1. Ouvrez Repère. L'utilisateur `admin` / `admin` est créé au premier lancement : **changez le mot de passe** dans
   *Administration → Utilisateurs*.
2. *Administration → ⚙ Modèles IA* : téléchargez et chargez un modèle de texte (à ajouter d'abord au catalogue, voir plus haut), le
   micro (Whisper) et la voix (*Installer le moteur de voix*, puis Chatterbox). Les téléchargements reprennent après une coupure et sont
   vérifiés par SHA-256. Un profil d'assistant « Repère » est créé au premier lancement ; modifiez-le dans *Profils d'assistant*.
3. *Assistant* : choisissez le profil, cliquez sur *Commencer*, écrivez ou parlez.

Si l'IA semble très lente, l'écran Modèles affiche « Ce serveur ne semble pas utiliser le GPU » : `llama-server` tourne alors sur le
processeur (pilote ou bibliothèques CUDA manquants).

## Données et configuration

| Quoi | Où |
|---|---|
| Base du serveur web (utilisateurs), médias importés | `~/.config/repere` (`REPERE_DATA_DIR`) |
| Données du moteur (profils, conversations, base de médias, voix, `catalog.json`) | `~/.config/repere/live` |
| Modèles téléchargés | `~/.local/share/repere/models` (`REPERE_MODELS_DIR`) |
| Moteur de voix Python | `~/.local/share/repere/tts-venv` (`REPERE_TTS_VENV`) |
| Journaux | `~/.config/repere/logs/` |

Variables utiles (à **exporter dans le shell** : aucun fichier `.env` n'est lu) :

- serveur web : `SESSION_SECRET` (sinon une clé aléatoire est tirée à chaque démarrage), `PORT` (3000), `HOST`, `REPERE_DATA_DIR`,
  `LIVE_PORT` (port du moteur, 3001) ;
- moteur : `LIVE_CONFIG` (fichier TOML, voir `live-engine/live.example.toml`), `LIVE_BIND`, `LIVE_DATA_DIR`, `LIVE_MODELS_DIR`,
  `LIVE_BIN_DIR`, `HF_TOKEN` (dépôts Hugging Face protégés), `REPERE_FFMPEG`.

## Développement

Prérequis : Node ≥ 20.19, Rust ≥ 1.82, `npm`. Pour tester avec un vrai modèle : `llama-server` dans `bin/` ou dans le `PATH`.

```bash
(cd backend && npm install)        # serveur Express
(cd frontend && npm install)       # interface Vue
(cd live-engine && cargo build)    # moteur d'IA
node backend/init-db.js            # première fois : crée la base JSON et l'utilisateur admin
./run                              # Express :3000 + Vite :5173 + moteur :3001
```

L'interface de développement est <http://localhost:5173> (Vite proxifie `/api` vers Express et `/api/live` vers le moteur).
L'application Electron en développement : `npm run build:engine && npm start`.

### Tests

```bash
(cd live-engine && cargo test --locked && cargo clippy --locked --all-targets -- -D warnings)   # moteur
(cd frontend && npm test && npm run build)                                                      # interface
node --test backend/liveProxy.test.js backend/packageDeps.test.js                               # proxy, dépendances du paquet
python3 -m unittest discover -s scripts/tts                                                     # serveur de voix (sans GPU)
```

GitHub Actions les lance à chaque envoi et chaque pull request (`.github/workflows/ci.yml`). Aucun test ne charge de vrai modèle :
le moteur est testé contre de faux serveurs (modèle de texte, téléchargement Hugging Face, serveur de voix).

### Organisation du dépôt

| Dossier | Contenu |
|---|---|
| `live-engine/` | moteur d'IA (Rust) : conversation, sources, voix, modèles, API |
| `backend/` | serveur Express : session, utilisateurs, proxy du moteur |
| `frontend/` | interface Vue 3 |
| `scripts/` | construction du paquet (`build-linux.sh`, `build-sidecars.sh`), diagnostic, serveur de voix (`tts/`) |
| `docs/INVENTAIRE.md` | inventaire de la reprise du code d'origine : ce qui a été supprimé, adapté ou gardé, et pourquoi |
| `main.js`, `preload.js`, `package.json` | application Electron |

## Sécurité : état actuel

Repère est pour l'instant **une application locale** : le serveur n'écoute que sur `127.0.0.1` dans l'application empaquetée, et ne doit
pas être exposé sur un réseau en l'état. À traiter avant tout déploiement (phase 3) :

- le compte `admin` / `admin` est créé au premier lancement, et la fenêtre de connexion l'affiche ;
- les routes en lecture du moteur (profils, conversations, modèles) et le WebSocket de conversation ne demandent pas de connexion ;
  seules les écritures passent par la session ;
- il n'y a qu'un rôle (administrateur) et une seule conversation sauvegardée par profil, partagée par tous les utilisateurs du poste.

## Licence

[MIT](LICENSE). Les composants tiers gardent leur licence (llama.cpp, whisper.cpp, Chatterbox : MIT ; Electron : MIT ; bibliothèques
CUDA : licence NVIDIA ; ffmpeg-static : GPL-3.0-or-later).
