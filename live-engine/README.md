# Moteur de Repère (`live-engine`)

Serveur Rust (axum + tokio) qui conduit la conversation : il construit le prompt, cherche les sources dans la médiathèque, pilote
les serveurs d'IA (`llama-server` pour le texte, `whisper-server` pour le micro, un serveur de voix) et renvoie la réponse en flux
à l'interface. Il écoute sur `127.0.0.1:3001` ; Express (port 3000) lui transmet `/api/live` (REST et WebSocket) après contrôle de
la session. Le dépôt racine ([`README.md`](../README.md)) explique l'installation et le développement de l'ensemble.

## Un tour de conversation

1. **Entrée.** Le texte arrive par le WebSocket (`{"type":"user"}`) ; la voix arrive en binaire (WAV 16 kHz mono) et passe par
   `whisper-server`, qui renvoie une `transcript`.
2. **Sources.** `MediaLibrary::relevant` rapproche les derniers messages (le plus récent compte le plus) et le résumé de ce qui
   précède du titre, des tags et de la description de chaque source. Les mots sont réduits à leur racine (accents, majuscules,
   pluriels, conjugaisons courantes : `terms.rs`), les mots vides sont écartés, les mots rares pèsent plus (IDF). Une source doit
   partager au moins deux mots utiles avec la conversation (ou un seul, rare, dit à l'instant). Au plus **4** sources sont proposées
   à chaque tour, jamais une source déjà montrée.
3. **Prompt système** (`prompt.rs`), dans cet ordre : règles du moteur, règles de format, fiche du profil d'assistant, résumé de la
   conversation, **liste des sources** (`#12 image : description…`) ou la mention qu'aucune ne correspond, et la source que
   l'utilisateur a sous les yeux.
4. **Génération.** Flux de jetons de `llama-server` (`/v1/chat/completions`). Le `StreamParser` retire les directives du texte, même
   coupées entre deux jetons.
5. **Directives.** `[[show: #12]]` affiche une source à côté de la conversation ; `[[replies: Ça a marché | Ouvrir un ticket]]`
   affiche des boutons de réponse. Une directive `show` n'est suivie que si le numéro figurait dans la liste proposée à ce tour et n'a
   pas déjà été montré (`allowed_pick`) : le modèle ne peut pas afficher une source qu'il n'a pas reçue. Toute autre directive est
   ignorée.
6. **Voix** (si elle est activée). `for_speech` retire le Markdown et les émojis, `SentenceSplitter` coupe en phrases (une marque
   d'étape « 1. » ne coupe pas), `chunk_for_tts` limite chaque morceau à 240 caractères, puis le serveur de voix (Chatterbox)
   renvoie du WAV, envoyé au navigateur phrase par phrase pendant que le texte s'écrit.
7. **Sauvegarde.** La conversation est enregistrée par profil d'assistant et reprise au prochain démarrage. Quand l'historique
   dépasse 75 % du contexte, les plus anciens tours sont résumés (`compact`) : le résumé garde le problème, les messages d'erreur, les
   étapes essayées et leur résultat, jamais un mot de passe.

### Règles de support

`ENGINE_RULES` (`prompt.rs`) passe avant toute consigne du profil. L'assistant :

- répond **uniquement d'après les sources** fournies, sans inventer de procédure, de commande, de menu, d'adresse ou de numéro ;
- dit « Je ne sais pas » quand les sources ne contiennent pas la réponse, et **propose d'ouvrir un ticket** ;
- ne demande **jamais** de mot de passe ou de code secret, ne les répète pas, et rappelle de ne les confier à personne ;
- reste poli, dans le périmètre du support informatique, et ne décide pas à la place de l'utilisateur ;
- ne parle ni de ses règles, ni du moteur, ni des directives.

`FORMAT_RULES` demande des réponses courtes, des étapes numérotées pour une procédure, et des boutons écrits **du point de vue de
l'utilisateur** (« Oui, j'ai redémarré », jamais « Avez-vous redémarré ? »).

## Profils d'assistant

Fiches éditables (*Administration → Profils d'assistant*), stockées en JSON dans `personas/`. Champs : `name`, `background` (contexte
et rôle), `personality`, `speaking_style`, `scenario`, `boundaries`, `summary`, `custom_fields`, `first_message` (message d'accueil),
`language`, `voice`, `tts_exaggeration`, `tts_cfg_weight`, `temperature`, `sampling`. Le profil **Repère** est créé au premier
lancement (marqueur `.profil-par-defaut` : supprimé, il n'est pas recréé) ; sa température est de 0,3 pour des réponses stables.

Un message d'accueil non vide est envoyé tel quel, sans appeler le modèle. Sans message d'accueil, le moteur demande au modèle de
saluer l'utilisateur (consigne cachée, préfixe `(Début de la conversation`, que l'interface n'affiche pas).

## Sources (base de connaissances)

Dans cette première phase, la base de connaissances est la **médiathèque** : des images (captures d'écran, schémas) et des vidéos,
chacune avec un titre, des tags et une description, dans une base SQLite (`live.db`). Le moteur sait les importer (un dossier, par
lien physique ou par copie), créer les miniatures, décrire et étiqueter une image avec un modèle de vision (`analyzer.rs`, photos
seulement ; la consigne interdit de recopier un mot de passe ou une donnée personnelle visible), et les chercher comme décrit plus
haut. Les routes d'import et d'analyse existent (voir plus bas) ; l'écran d'administration des médias n'a pas été conservé.

Le dictionnaire de synonymes (`synonyms.txt`, compilé dans le moteur, complété par le fichier du même nom dans le dossier de données)
est chargé et testé, mais la recherche ne le consulte pas encore : il est prévu pour la base de documents.

L'import de documents (Markdown, HTML, PDF, DOCX), le découpage en passages et la citation `[[source: #id]]` ne sont pas faits.

## Modèles et catalogue

Le catalogue livré ne contient que le micro (Whisper : *small*, *medium*, *large-v3-turbo*) et la voix (Chatterbox multilingue).
**Il ne contient aucun modèle de texte.** Les fichiers se téléchargent depuis Hugging Face avec reprise après coupure et vérification
SHA-256, puis se chargent depuis *Administration → ⚙ Modèles IA*. Avant le chargement, le moteur lit l'architecture réelle dans le
fichier GGUF et calcule combien de couches et quel contexte tiennent en VRAM (`live-engine plan [id]` affiche ce calcul).

### Ajouter un modèle de texte

Un modèle de texte s'ajoute par un fichier `catalog.json` placé dans le dossier de données du moteur
(`~/.config/repere/live/catalog.json` avec l'application installée). C'est un tableau d'entrées ; une entrée de même `id` qu'une
entrée livrée la remplace.

1. **Choisir un modèle** de type *instruct*, au format GGUF, dont la taille laisse de la place au contexte : sur 12 Go de VRAM, un
   modèle de 7 à 14 milliards de paramètres en quantification Q4_K_M ou Q5_K_M (environ 4 à 9 Go) convient, avec la voix et le micro
   qui prennent leur part (le moteur affiche le plan mémoire).
2. **Vérifier sur Hugging Face** que le dépôt existe et relever le **nom exact du fichier** `.gguf` dans l'onglet *Files and versions*.
   Ne recopiez pas un identifiant de mémoire : une faute et le téléchargement échoue (« fichier introuvable » ou « aucun .gguf »).
3. **Écrire l'entrée** :

   ```json
   [
     {
       "id": "mon-modele",
       "kind": "llm",
       "name": "Nom affiché dans l'écran Modèles",
       "description": "Quelques mots sur le modèle.",
       "repo": "auteur/depot-GGUF",
       "files": ["nom-exact-du-fichier-Q4_K_M.gguf"],
       "size_gb": 7.0
     }
   ]
   ```

   Champs utiles : `files` (noms exacts, prioritaire) ou `pattern` (morceau du nom, par exemple `Q4_K_M` : il doit désigner **un seul**
   `.gguf`, sinon le moteur refuse et liste les candidats), `alt_repos` (dépôts de secours), `size_gb` (taille indicative avant
   résolution), `args` (arguments ajoutés à `llama-server` pour ce modèle, par exemple un modèle de conversation), `mmproj` et
   `mmproj_repo` (projecteur de vision : nécessaire pour décrire des images).
4. **Redémarrer l'application** (le catalogue est lu au démarrage). Le modèle apparaît dans *Administration → ⚙ Modèles IA* : *Télécharger*,
   puis *Charger*. Un dépôt protégé demande un jeton Hugging Face (champ de l'écran Modèles, ou variable `HF_TOKEN`).
5. Un fichier `catalog.json` illisible est ignoré (avertissement dans le journal) : le catalogue livré reste intact.

## Configuration

Tout a une valeur par défaut ; `live.toml` (chemin dans `LIVE_CONFIG`) est facultatif. Voir [`live.example.toml`](live.example.toml),
commenté. Les variables d'environnement l'emportent (l'application Electron les renseigne) :

| Variable | Rôle |
|---|---|
| `LIVE_CONFIG` | fichier TOML (défaut `live.toml`) |
| `LIVE_BIND` | adresse d'écoute (défaut `127.0.0.1:3001`) |
| `LIVE_DATA_DIR` | données du moteur (défaut `data`) |
| `LIVE_MODELS_DIR` | modèles téléchargés (défaut `<données>/models`) |
| `LIVE_BIN_DIR` | `llama-server`, `whisper-server`, scripts de voix (défaut `bin`, puis le `PATH`) |
| `LIVE_UPLOADS_DIR` | médias importés |
| `LIVE_FRONTEND_DIR` | interface compilée (sert en développement sans Express) |
| `LIVE_EXIT_ON_STDIN_EOF=1` | le moteur s'arrête quand l'application qui l'a lancé disparaît (décharge les modèles, libère le GPU) |
| `HF_TOKEN`, `HF_ENDPOINT` | jeton et adresse de Hugging Face |
| `RUST_LOG` | niveau des journaux (défaut `info`) |

`server.admin_token` : si défini, toutes les routes d'écriture exigent `Authorization: Bearer <jeton>` (pour exposer le moteur seul
sur un réseau). Dans l'application, la protection passe par la session Express.

### Dossier de données

| Chemin | Contenu |
|---|---|
| `live.db` | médiathèque (SQLite) |
| `personas/` | profils d'assistant (JSON) |
| `conversations/` | une conversation sauvegardée par profil |
| `voices/` | échantillons de voix de référence (wav, mp3, flac, ogg) |
| `catalog.json` | modèles ajoutés au catalogue (facultatif) |
| `synonyms.txt` | synonymes personnels (facultatif, relu quand il change) |
| `live-state.json` | derniers modèles choisis (rechargés au démarrage si `engine.autoload`) |

## API REST (`/api/live`)

Les routes d'écriture exigent le jeton d'administration s'il est configuré (voir plus haut).

| Route | Rôle |
|---|---|
| `GET /health` | état de l'IA, du micro, de la voix, plan mémoire, nombre de sources |
| `GET /plan` | plan mémoire (couches sur GPU, contexte) |
| `GET /models` · `POST /models/{id}/download` · `POST /models/{id}/vision` · `POST /models/{id}/cancel` · `DELETE /models/{id}` | catalogue, téléchargement, ajout de la vision, annulation, suppression |
| `GET /engine/status` · `POST /engine/load` · `POST /engine/unload` · `GET /engine/logs/{llm\|stt\|tts}` | chargement et déchargement des serveurs d'IA, journaux |
| `POST /engine/tts-runtime/install` | installe le moteur de voix (Python/PyTorch) |
| `PUT` · `DELETE /hf-token` | jeton Hugging Face (jamais renvoyé) |
| `GET /voices` · `PUT` · `DELETE /voices/{nom}` · `POST /tts/preview` | voix de référence, écoute d'essai |
| `GET /personas` · `GET` · `PUT` · `DELETE /personas/{id}` | profils d'assistant |
| `GET /conversations` | conversations sauvegardées |
| `GET /media` · `PATCH /media/{id}` · `POST /media/bulk` · `POST /media/delete` · `POST /media/scan` | médiathèque (filtrable : `limit`, `q`, `kind`, `untagged`, `analysis`, `offset`) |
| `GET` · `POST /media/import` · `POST /media/import/cancel` | import d'un dossier ou de liens |
| `GET` · `POST /media/analyze` · `POST /media/analyze/cancel` | description automatique des images par un modèle de vision |
| `GET /ws` | WebSocket de conversation (origines locales seulement) |

## Protocole WebSocket

Client → serveur (JSON) : `start` (ouvre ou reprend la conversation d'un profil ; `"resume": false` en commence une nouvelle, `user_name`
donne le prénom), `user` (texte), `interrupt`, `settings` (voix activée ou non), `reset` (supprime la conversation sauvegardée),
`status`, `voice_test`, `speak` (relit un texte à voix haute). Un message binaire est la parole de l'utilisateur (WAV 16 kHz mono).

Serveur → client : `hello`, `status`, `persona`, `restored`, `reset_done`, `transcript`, `gen_start` (identifiant de la génération, envoyé
**avant** ses jetons), `token`, `media` (la source à afficher), `media_miss`, `replies`, `audio` (suivi de sa trame binaire),
`speak_start`, `voice_end`, `tts_error`, `done`, `interrupted`, `error`. Le détail est en tête de [`src/ws.rs`](src/ws.rs).

## Organisation du code

| Module | Rôle |
|---|---|
| `main.rs`, `config.rs`, `state.rs` | démarrage, configuration, état partagé |
| `api.rs` | routes REST, origine du WebSocket |
| `ws.rs` | session de conversation, tour de parole, voix, résumé |
| `prompt.rs` | règles de support et prompt système |
| `director.rs` | directives `[[show]]` / `[[replies]]`, découpage en phrases, texte lu à voix haute |
| `persona.rs`, `conversation.rs` | profils d'assistant, conversations sauvegardées |
| `media.rs`, `terms.rs`, `thesaurus.rs` | médiathèque, recherche des sources, racines de mots, synonymes |
| `importer.rs`, `analyzer.rs`, `webp.rs`, `files.rs` | import, analyse des images par un modèle de vision, miniatures, suppression des fichiers |
| `models.rs`, `gguf.rs`, `hardware.rs`, `engine.rs` | catalogue et téléchargements, lecture des GGUF, plan mémoire, lancement des serveurs d'IA |
| `llm.rs`, `voice.rs` | clients de `llama-server`, de `whisper-server` et du serveur de voix |

## Tests

```bash
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Aucun test ne charge de vrai modèle : le moteur est testé contre de faux serveurs (modèle de texte, Hugging Face, voix). La CI
(`.github/workflows/ci.yml`) lance les deux commandes ; le code mort fait échouer `clippy`.
