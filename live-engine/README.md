# Moteur de Repère (`live-engine`)

Serveur Rust (axum + tokio) qui conduit la conversation : il construit le prompt, cherche les passages de la base de connaissances, pilote
les serveurs d'IA (`llama-server` pour le texte, `whisper-server` pour le micro, un serveur de voix) et renvoie la réponse en flux
à l'interface. Il écoute sur `127.0.0.1:3001` ; Express (port 3000) lui transmet `/api/live` (REST et WebSocket) après contrôle de
la session. Le dépôt racine ([`README.md`](../README.md)) explique l'installation et le développement de l'ensemble.

## Un tour de conversation

1. **Entrée.** Le texte arrive par le WebSocket (`{"type":"user"}`) ; la voix arrive en binaire (WAV 16 kHz mono) et passe par
   `whisper-server`, qui renvoie une `transcript`.
2. **Sources.** `KnowledgeBase::relevant` rapproche les **derniers messages de l'utilisateur** (le plus récent compte le plus ; les réponses de
   l'assistant n'y comptent pas : son message d'accueil et ses propres réponses ramèneraient toujours les mêmes extraits) et le résumé de
   ce qui précède des passages des documents importés (voir [Recherche](#recherche-des-passages)). Au plus **4** sources sont proposées à
   chaque tour ; une image déjà montrée n'est pas reproposée.
3. **Prompt système** (`prompt.rs`), dans cet ordre : règles du moteur, règles de format, fiche du profil d'assistant, résumé de la
   conversation, **liste des sources** (`#12 image : description…`) ou la mention qu'aucune ne correspond, et la source que
   l'utilisateur a sous les yeux.
4. **Génération.** Flux de jetons de `llama-server` (`/v1/chat/completions`). Le `StreamParser` retire les directives du texte, même
   coupées entre deux jetons.
5. **Directives.** `[[source: #12]]` (ou `[[source: #12, #15]]`) cite un extrait : l'interface en fait un lien cliquable sous la réponse ;
   `[[show: #12]]` affiche une image à côté de la conversation ; `[[replies: Ça a marché | Ouvrir un ticket]]` affiche des boutons de
   réponse. Une directive n'est suivie que si le numéro figurait dans la liste proposée **à ce tour** (`allowed_pick`) : le modèle ne peut
   citer ni afficher une source qu'il n'a pas reçue, et une image n'est montrée qu'une fois. Toute autre directive est ignorée. Quand la
   réponse dit qu'elle ne sait pas et qu'elle n'a pas proposé de boutons, le moteur ajoute *Ouvrir un ticket* et *Reformuler*.
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

## Base de connaissances

Les **documents** importés (`documents` dans `live.db`) sont découpés en **passages**. Un passage est une ligne de la table `media`
(`kind = 'passage'`) : il partage ainsi la numérotation des sources (`#id`) avec les images, et les mêmes gardes-fous. Les images et vidéos
décrites (captures d'écran) restent possibles — routes `/media` et `analyzer.rs` — mais n'ont plus d'écran d'administration.

### Import

`ingest.rs` lit un fichier envoyé (`PUT /kb/documents?name=…`) ou un dossier (`POST /kb/import`, en tâche de fond, avec progression).
Chaque fichier passe par `extract/` (format reconnu à l'en-tête et à l'extension) :

| Format | Lecture |
|---|---|
| Markdown (`.md`) | titres, listes, tableaux, blocs de code ; en-tête YAML (`title`, `tags`/`keywords`, `url`) |
| HTML (`.html`) | titre (`<title>` sans le nom du site, sinon premier `<h1>`), listes, tableaux ; `meta keywords`, `link canonical` ; scripts et menus ignorés |
| Word (`.docx`) | styles de titre, listes numérotées ou à puces, tableaux ; pas de macros lues |
| PDF (`.pdf`) | texte page par page (`lopdf`, `pdf-extract`), en-têtes et pieds de page répétés retirés, titres déduits ; **un PDF scanné (images) n'a pas de texte et est refusé** : il faudrait un OCR |
| Texte (`.txt`) | encodage détecté, titres « 1. Titre » et lignes en capitales reconnus |

Le texte est découpé (`extract/chunk.rs`) en passages de 300 à 1 400 caractères environ, **sans couper une liste ni séparer une phrase
d'introduction de sa liste** ; les petites sections voisines sont réunies, chaque passage garde son chemin de titres (« Les erreurs courantes
› Erreur 809 ») et sa page (PDF). Le titre vient de l'en-tête du document, à défaut du premier titre, à défaut du nom du fichier. Un fichier
déjà importé (même nom, même empreinte SHA-256) n'est pas relu ; modifié, il remplace l'ancienne version ; le même contenu sous un autre nom est
signalé comme doublon. Un fichier illisible est refusé avec la cause ; la lecture est limitée en taille (50 Mo) et en durée.

La copie du fichier d'origine est gardée dans `kb-files/` (nommée par son empreinte) pour être téléchargée depuis le passage cité.

**Mots de passe oubliés dans un document** (`secrets.rs`) : un libellé (« mot de passe : », « token = ») suivi d'une valeur qui ressemble à un
secret, une clé connue (AWS, GitHub, JWT…), une clé privée ou une adresse avec identifiants sont repérés. L'import le signale sans recopier la
valeur ; la valeur est remplacée par `[masqué]` dans ce que reçoit l'IA, dans les passages affichés (`GET /kb/passages/{id}`) et dans les
tickets, et le fichier d'origine d'un tel document n'est pas proposé au téléchargement. C'est un filet de sécurité, pas une garantie : un
mot de passe écrit au milieu d'une phrase peut passer.

### Recherche des passages

`search.rs` construit un index en mémoire (reconstruit quand la base ou `synonyms.txt` change) : chaque racine de mot (`terms.rs`) est pesée selon
l'endroit où elle figure (étiquettes ×3, titre et section ×2,5, texte ×1) et le classement est un **BM25**. Les nombres de 2 à 6 chiffres comptent
(« erreur 809 », « ports 500 et 4500 », « 0x80070005 »).

**Dictionnaire de synonymes** (`thesaurus.rs`, `synonyms.txt` compilé dans le moteur, complété par le fichier du même nom du dossier de données ou
par l'écran d'administration) : une ligne = un concept, des mots de **même sens** (« mdp, mot de passe, password ») ; après `~`, des mots seulement
**liés**, qui classent mieux sans rien déclencher. Une expression l'emporte sur ses mots (« réseau sans fil » = Wi-Fi, pas « réseau »). Dans la
question, un mot compte comme retrouvé dans un passage si le passage contient le mot ou un mot de même sens.

**Quand rien n'est proposé.** Un passage n'est retenu que s'il répond à **au moins deux mots** de la question, dont le poids cumulé (leur rareté
dans la base) dépasse un seuil, ou à **un seul mot qui le désigne** (rare, ou présent dans son titre ou ses étiquettes, et dit dans le dernier
message). Les mots courants de la langue (« jour », « donner », « configurer »… : `GENERIC` dans `terms.rs`) comptent moins et ne désignent
jamais rien à eux seuls, sauf si le dictionnaire les connaît ; une question sans mot de la base ne ramène donc rien (« Comment poser un jour de
congé ? »). Au plus 3 passages d'un même document, et au moins 30 % du score du premier. Une question de suite (« ça ne marche toujours pas »)
retrouve le sujet dans les messages précédents de l'utilisateur.

*Limites connues* : pas de recherche par sens (« PC lent » ne retrouve « ordinateur très lent » que grâce au dictionnaire), pas de correction
des fautes de frappe (elles ne passent que si la racine reste proche), et un nouveau sujet posé après un autre dans la même conversation peut
ramener les extraits du sujet précédent ; la règle « si aucun extrait ne concerne le dernier message, dis que tu ne sais pas » est dans le prompt.

### Tickets

Quand l'assistant ne sait pas, l'utilisateur ouvre un ticket (bouton de réponse *Ouvrir un ticket*, ou bouton 🎫 de la conversation). Le message
`ticket_draft` demande au moteur de préparer le brouillon : un **résumé de la conversation** par le modèle (le même résumeur que `compact`,
`SUMMARY_RULES`), un titre tiré de ce résumé, le prénom, les extraits consultés (`tickets.rs::compose`). L'utilisateur relit, corrige, puis
enregistre (`POST /tickets`) ; le ticket reçoit une référence (« T-0012 ») et reste dans `tickets.db`, que l'administration lit
(*Administration → Tickets*). **Aucun outil de suivi n'est relié** : `TicketStore::create` est le point d'entrée prévu pour GLPI ou Teams.
Sans résumé du modèle, le ticket reprend les messages de l'utilisateur tels quels.

### Évaluation

Le dossier [`../eval`](../eval/README.md) contient un corpus de 16 documents, 30 questions avec réponses attendues et les outils qui les
posent. `cargo test eval` (`src/eval.rs`) importe le corpus et vérifie que la recherche propose le bon document (rappel, rang moyen inverse)
et ne propose rien pour les questions sans rapport.

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
| `LIVE_UPLOADS_DIR` | images importées |
| `LIVE_FRONTEND_DIR` | interface compilée (sert en développement sans Express) |
| `LIVE_EXIT_ON_STDIN_EOF=1` | le moteur s'arrête quand l'application qui l'a lancé disparaît (décharge les modèles, libère le GPU) |
| `HF_TOKEN`, `HF_ENDPOINT` | jeton et adresse de Hugging Face |
| `RUST_LOG` | niveau des journaux (défaut `info`) |

`server.admin_token` : si défini, toutes les routes d'écriture exigent `Authorization: Bearer <jeton>` (pour exposer le moteur seul
sur un réseau). Dans l'application, la protection passe par la session Express.

### Dossier de données

| Chemin | Contenu |
|---|---|
| `live.db` | base de connaissances : documents, passages, images (SQLite) |
| `kb-files/` | copies des fichiers d'origine des documents importés |
| `tickets.db` | tickets enregistrés (SQLite) |
| `personas/` | profils d'assistant (JSON) |
| `conversations/` | une conversation sauvegardée par profil |
| `voices/` | échantillons de voix de référence (wav, mp3, flac, ogg) |
| `catalog.json` | modèles ajoutés au catalogue (facultatif) |
| `synonyms.txt` | synonymes personnels (facultatif, modifiable dans l'administration, relu quand il change) |
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
| `GET /kb/stats` · `GET` · `PUT /kb/documents` (`?name=` obligatoire ; `title`, `url`, `tags` facultatifs ; le corps est le fichier) | statistiques, liste, envoi d'un document (réponse : `added`, `unchanged`, `duplicate` ou `failed`) |
| `GET` · `DELETE /kb/documents/{id}` · `GET /kb/documents/{id}/file` | un document et ses passages, suppression, fichier d'origine (en téléchargement ; refusé si des mots de passe y ont été repérés) |
| `GET /kb/passages/{id}` | un passage (mots de passe masqués) avec son document et les passages voisins |
| `GET` · `POST /kb/import` · `POST /kb/import/cancel` | import d'un dossier de la machine du moteur (`path`, `recursive`, `tags`, `folder_tags`, `base_url`), progression, annulation |
| `GET /kb/search?q=…&limit=` | essai de la recherche : mots retenus, concepts reconnus, mots inconnus, passages classés (les 4 premiers sont ceux que l'IA recevrait) |
| `GET` · `PUT /kb/synonyms` | dictionnaire de synonymes (de base en lecture seule, personnel modifiable) et remarques sur les lignes douteuses |
| `GET` · `POST /tickets` · `PATCH /tickets/{id}` | liste (administration), création (ouverte à tous), changement de statut (`nouveau`, `pris en charge`, `clos`) |
| `GET /media` · `PATCH /media/{id}` · `POST /media/bulk` · `POST /media/delete` · `POST /media/scan` | images décrites (filtrable : `limit`, `q`, `kind`, `untagged`, `analysis`, `offset`) |
| `GET` · `POST /media/import` · `POST /media/import/cancel` | import d'un dossier ou de liens |
| `GET` · `POST /media/analyze` · `POST /media/analyze/cancel` | description automatique des images par un modèle de vision |
| `GET /ws` | WebSocket de conversation (origines locales seulement) |

## Protocole WebSocket

Client → serveur (JSON) : `start` (ouvre ou reprend la conversation d'un profil ; `"resume": false` en commence une nouvelle, `user_name`
donne le prénom), `user` (texte), `interrupt`, `settings` (voix activée ou non), `reset` (supprime la conversation sauvegardée),
`status`, `ticket_draft` (prépare le ticket de la conversation), `voice_test`, `speak` (relit un texte à voix haute). Un message binaire est la parole de l'utilisateur (WAV 16 kHz mono).

Serveur → client : `hello`, `status`, `persona`, `restored`, `reset_done`, `transcript`, `gen_start` (identifiant de la génération, envoyé
**avant** ses jetons), `token`, `source` (un extrait cité par la réponse en cours), `media` (l'image à afficher), `media_miss`, `replies`
(avec `ticket` : l'intitulé du bouton qui ouvre un ticket), `ticket_draft`, `audio` (suivi de sa trame binaire),
`speak_start`, `voice_end`, `tts_error`, `done`, `interrupted`, `error`. Le détail est en tête de [`src/ws.rs`](src/ws.rs).

## Organisation du code

| Module | Rôle |
|---|---|
| `main.rs`, `config.rs`, `state.rs` | démarrage, configuration, état partagé |
| `api.rs` | routes REST, origine du WebSocket |
| `ws.rs` | session de conversation, tour de parole, voix, résumé |
| `prompt.rs` | règles de support et prompt système |
| `director.rs` | directives `[[source]]` / `[[show]]` / `[[replies]]`, découpage en phrases, texte lu à voix haute |
| `persona.rs`, `conversation.rs` | profils d'assistant, conversations sauvegardées |
| `kb.rs`, `search.rs`, `terms.rs`, `thesaurus.rs` | base de connaissances (documents, passages, images), recherche BM25, racines de mots, synonymes |
| `extract/` | lecture des formats (Markdown, HTML, Word, PDF, texte) et découpage en passages |
| `ingest.rs`, `secrets.rs` | import de fichiers et de dossiers, détection des mots de passe dans les documents |
| `tickets.rs` | brouillon et enregistrement des tickets |
| `importer.rs`, `analyzer.rs`, `webp.rs`, `files.rs` | import d'images, analyse par un modèle de vision, miniatures, suppression des fichiers |
| `eval.rs` | test d'évaluation de la recherche sur le corpus de `eval/` |
| `models.rs`, `gguf.rs`, `hardware.rs`, `engine.rs` | catalogue et téléchargements, lecture des GGUF, plan mémoire, lancement des serveurs d'IA |
| `llm.rs`, `voice.rs` | clients de `llama-server`, de `whisper-server` et du serveur de voix |

## Tests

```bash
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Aucun test ne charge de vrai modèle : le moteur est testé contre de faux serveurs (modèle de texte, Hugging Face, voix). La CI
(`.github/workflows/ci.yml`) lance les deux commandes ; le code mort fait échouer `clippy`. `cargo test eval -- --nocapture` affiche, question
par question, le rang du bon document (`DETAIL=1` ajoute les mots retenus et les scores). Pour la chaîne complète sans modèle :
`eval/tools/smoke.sh` (voir [`../eval/README.md`](../eval/README.md)).
