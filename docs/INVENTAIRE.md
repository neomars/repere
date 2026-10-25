# Inventaire : de Virtual Story à Repère

> **Statut : phase 1 appliquée.** Cet inventaire a été validé (D1 : supprimer ; D3 : option B ; D2 et D4 à D8 : recommandations suivies), puis exécuté. Il garde la trace de la reprise : ce qui est parti, ce qui a été réécrit, ce qui est resté, et pourquoi. Les tableaux du §4 sont le plan validé ; les écarts constatés à l'exécution sont au §4 bis ; les mesures sont au §0 et au §6.
> Le code d'origine est celui de `virtual-story` (dépôt privé, branche `claude/gracious-rubin-yftbq5`, 156 fichiers suivis). Il a été copié sans `.git` et hors `LICENSE` : Repère garde sa licence MIT et ne reprend aucun historique.
> **Ce fichier est le seul du dépôt qui cite encore l'ancien nom et le vocabulaire de l'ancien projet : c'est son objet.** La recherche finale de la phase 1 l'exclut (§6).

## 0. Résultat de la phase 1

### Supprimé

64 fichiers entiers (12 945 lignes, §1) et de larges parties de fichiers conservés (`ws.rs`, `media.rs`, `models.rs`, `analyzer.rs`, `prompt.rs`, `persona.rs`, `Live.vue`, `LivePersonas.vue`…).

| Famille | Retiré |
|---|---|
| Graphe d'histoire (D1) | éditeur et lecteur de scènes vidéo (`SceneList`, `SceneEdit`, `Player`, `StoryHost`…), routes `scenes`, `parts`, `choices`, `player` et `settings`, ffmpeg côté serveur, `storygen.rs`, `storyplan.rs`, `ideas.rs` |
| Avatar animé | visage 2D (`TalkingFace`, `FaceCalibrator`, synchronisation de la bouche), avatar neuronal AVTR-1 (`neural.rs`, `scripts/avatar/`), tons de voix émotionnels (`expression.rs`) |
| Jeu de rôle | niveaux de contenu, détection de mineurs et de refus (`lexicon.rs`), champs de la fiche de personnage (âge, sexe, sexualité, envies, sons, rôles, visage, avatar), règles d'expression, les 22 modèles de texte du catalogue, le contenu adulte du dictionnaire de synonymes |
| Médiathèque côté interface | `LiveMedia.vue`, illustration automatique par mots-clés (`by_keywords`), import de l'ancienne base, analyse des vidéos, transcription audio |
| Divers | les trois `pnpm-lock.yaml` (D6), `build.xml`, `seed-db.js`, `test_api.js`, `backend/db.json`, `backend/cleanup.js`, `.Jules/`, `docs/images/`, `frontend/README.md`, fichiers morts ou vides (§2.10) |

### Renommé

| Avant | Après |
|---|---|
| Virtual Story (produit, fenêtre, journal, messages) | Repère |
| `virtual-story` (paquet npm, exécutable, nom du `.deb`) | `repere` |
| `com.virtualstory.app`, catégorie de menu `AudioVideo` | `com.repere.app`, `Utility` |
| `~/.config/Virtual Story`, `~/.local/share/virtual-story` | `~/.config/repere`, `~/.local/share/repere` |
| variables `VS_*` | `REPERE_*` |
| cookie `vs.sid`, clés du navigateur `vs.*` | `repere.sid`, `repere.*` |
| pont Electron `vsNative` | `repereNative` |
| « personnage » dans l'interface | « profil d'assistant » (identifiants du code inchangés, D2) |

### Réécrit

- `prompt.rs` : règles de support (répondre d'après les sources seulement, « je ne sais pas » puis proposition de ticket, ne jamais demander ni répéter un mot de passe, rester dans le périmètre informatique, réponses courtes en étapes numérotées) à la place des règles de jeu de rôle ;
- `persona.rs` : profil d'assistant (nom, contexte, personnalité, style, périmètre, limites, message d'accueil, voix, température) et profil par défaut « Repère » créé au premier lancement ;
- `ws.rs` et `director.rs` : protocole WebSocket allégé (plus d'avatar, de carrefours ni d'histoires), directives réduites à `[[show: #id]]` (accepté seulement pour une source proposée à ce tour : `allowed_pick`) et `[[replies: …]]`, résumé de conversation orienté problème et étapes essayées ;
- `analyzer.rs` : lecture de captures d'écran par un modèle de vision, photos seulement ; la consigne interdit de recopier un mot de passe ou une donnée personnelle ;
- `models.rs` : catalogue réduit à Whisper et Chatterbox, complété par `catalog.json` ; `media.rs`, `terms.rs`, `thesaurus.rs`, `synonyms.txt` : recherche de sources sans niveau de contenu, dictionnaire de 61 concepts de jargon informatique ;
- interface : accueil, conversation de support (message d'accueil, étapes, réponses rapides, source à côté du texte), `LivePersonas.vue`, `LiveModels.vue`, menu, textes anglais traduits ;
- construction du paquet, CI (trois jobs : moteur, interface et proxy, serveur de voix), scripts, README et README du moteur.

### Mesures

| Contrôle | Avant | Après |
|---|---|---|
| fichiers suivis | 156 | 94 (dont `LICENSE` MIT et ce document) |
| lignes de code³ | 31 570 | 14 823 |
| `cargo test --locked` | 216 tests | 140 tests |
| `cargo clippy --locked --all-targets -- -D warnings` | sans avertissement | sans avertissement |
| `npm test` (interface) | 46 tests, 7 fichiers | 15 tests, 2 fichiers ; `vite build` réussi |
| `node --test` (backend) | 12 tests (3 fichiers sur 4) | 4 tests, 2 fichiers |
| `python3 -m unittest` (serveur de voix) | 10 tests | 10 tests |
| parcours complet dans Chromium (vrai moteur, vrai Express, interface compilée, faux serveur de modèle) | — | 40 contrôles verts, console sans erreur |

La baisse du nombre de tests est celle des fonctions retirées (carrefours, visage animé, graphe, vidéo, intensité…). L'installation depuis chacun des trois verrous (`npm ci` : racine, backend, interface) réussit. ³ Même définition qu'au §1 : hors verrous et images, documentation comprise.

### Ce qui reste à décider

1. **Modèles de texte (D4).** Le catalogue n'en contient aucun : `huggingface.co` est refusé (403) par le réseau de l'environnement de reprise, et aucun dépôt ni nom de fichier n'a pu être vérifié. Sans modèle de texte chargé, l'assistant ne répond pas. Deux voies : autoriser `huggingface.co` pour qu'un modèle standard (Mistral, Qwen ou Gemma, au format GGUF, pour 12 Go de VRAM) soit ajouté après vérification, ou ajouter le vôtre par `catalog.json` ([`live-engine/README.md`](../live-engine/README.md#ajouter-un-modèle-de-texte)).
2. **Licence (D5).** Le dépôt est sous MIT. Le code d'origine venait d'un dépôt sous GPL-2.0, `ffmpeg-static` est sous GPL-3.0-or-later et le paquet embarque des bibliothèques CUDA sous licence NVIDIA : à faire confirmer avant toute diffusion du paquet.
3. **Lecture de D3.** « delete D3 » a été lu comme l'option B : `by_keywords` supprimé, thésaurus conservé en attendant la phase 2. Si la suppression du thésaurus lui-même était voulue, c'est un commit de plus (`thesaurus.rs`, `synonyms.txt`, le rechargement du fichier de synonymes dans `media.rs`).
4. **Identifiants `persona`** (D2) : le renommage mécanique en `profile` reste possible, dans un commit à part.
5. **Données de l'ancienne installation** : aucun code de migration. Pour garder les modèles déjà téléchargés : `mv ~/.local/share/virtual-story ~/.local/share/repere` (ou pointer `REPERE_MODELS_DIR` dessus).
6. **Icône et favicon** (D7) : placeholders, à remplacer par l'identité visuelle de la DSI.
7. **Médias.** L'écran d'administration des médias est supprimé, mais les routes d'import et d'analyse du moteur restent, sans écran : la phase 2 les remplace par la base de connaissances. Le panneau « source affichée » de la conversation et les dossiers `/videos`, `/thumbnails` et `/photos` d'Express restent aussi, car `[[show: #id]]` en dépend (§4 bis).
8. **Sécurité**, avant tout déploiement (phase 3) : voir §7.
9. **Tutoiement.** L'interface vouvoie ; les messages d'erreur des routes d'administration du moteur et les commentaires du code tutoient encore.

## Méthode de l'inventaire

**Comment c'est vérifié.** Chaque module a été lu, pas seulement son nom. Le graphe des dépendances entre modules Rust a été relevé. La liste de contrôle de la phase 1 (`sexe`, `sexualit`, `intime`, `explicit`, `intensit`, `gémiss`, `onomatop`, `heretic`, `abliter`, `sutra`, `avtr`, `lifeselector`, `virtual-story`, `virtual story`, sans tenir compte de la casse) a été passée sur chaque fichier suivi. Les tests ont été lancés avant toute modification pour avoir un point de comparaison.

## 1. En bref

| Décision | Fichiers | Lignes de code¹ | Occurrences de la liste de contrôle |
|---|--:|--:|--:|
| supprimer | 64 | 12 945 | 123 |
| adapter | 68 | 16 016 | 459 |
| garder | 24 | 2 609 | 1 |
| **Total** | **156** | **31 570** | **583** |

¹ Hors fichiers de verrous (`package-lock.json`, `pnpm-lock.yaml`, `Cargo.lock`) et hors images.

Ce n'est donc pas qu'un nettoyage par suppression : 459 des 583 occurrences sont dans 36 fichiers **à réécrire**, surtout `media.rs`, `models.rs`, `ws.rs`, `analyzer.rs`, `prompt.rs`, `persona.rs`, `Live.vue` et `LivePersonas.vue`.

**Point de départ mesuré, avant toute modification :**

| Contrôle | Résultat |
|---|---|
| `cargo test --locked` (moteur Rust) | 216 tests, tous verts |
| `cargo clippy --locked --all-targets -- -D warnings` | sans avertissement |
| `npm test` (interface) | 46 tests dans 7 fichiers, tous verts |
| `node --test` (backend) | 12 tests verts dans 3 fichiers sur 4 (`videoUtils.test.js` exige `npm install`) |
| `python3 -m unittest` (serveur de voix) | 10 tests verts (le test du serveur d'avatar est sauté : `opencv` absent, sans importance puisqu'il est supprimé) |

L'environnement de reprise n'avait pas de GPU : `llama-server`, `whisper-server` et l'application Electron n'y tournent pas. Les vérifications finales sont donc les tests, clippy, `vite build` et un parcours complet dans un navigateur avec un faux serveur de modèle (§6). L'assistant n'a pas été essayé avec un vrai modèle.

## 2. Ce que la lecture du code change par rapport à votre analyse

Vos deux listes tiennent : aucun des fichiers que vous voulez garder ou adapter n'est à supprimer, et aucun de ceux que vous voulez supprimer n'est à conserver. Ce qui diffère, c'est ce qu'il y a **à l'intérieur** de fichiers que leur nom laisse croire neutres. Les écarts :

1. **`lexicon.rs` n'est pas qu'à supprimer.** `fold()` (pliage des accents et de la casse) y vit, et 8 modules l'importent (`analyzer`, `expression`, `ideas`, `media`, `storygen`, `storyplan`, `terms`, `ws`). Il est déplacé dans `terms.rs` avant la suppression. Le reste (niveaux de contenu sexuel de 1 à 5, détection de personnes mineures, détection d'un refus du modèle) disparaît.
2. **Le contenu de `synonyms.txt`, livré avec `thesaurus.rs`, est un lexique adulte** : 122 lignes classées en tenues, nudité, lieux, gestes, désir et « intime ». Le mécanisme de `thesaurus.rs` reste ; son dictionnaire est **remplacé** par un jeu de départ de jargon informatique (« mdp = mot de passe », « wifi = réseau sans fil »…). Le vrai dictionnaire reste la tâche de la phase 2. Les 7 tests du thésaurus sont réécrits sur ce jeu.
3. **`prompt.rs` contient trois blocs de règles, pas deux.** En plus de `ENGINE_RULES` et `FORMAT_RULES`, `EXPRESSION_RULES` (vocalisations à caractère sexuel) est à supprimer. Et `system_prompt()` injecte aussi des champs de la fiche : sexe, sexualité, « goûts, envies et fantasmes », « sons et onomatopées ».
4. **`analyzer.rs` : seule la plomberie de vision est réutilisable.** Les prompts demandent une description « sans aucune censure » ; un mode « légendeur » non censuré (JoyCaption) est suivi d'une traduction « sans rien adoucir » ; il y a une détection de mineurs, une note d'intensité, une analyse vidéo par images clés et une transcription audio. Environ 60 % du fichier part (estimation). Restent : le client du serveur de vision (`Vlm`), l'état (`vision_info`), le redimensionnement d'image par ffmpeg et la tâche annulable. Le prompt est réécrit pour lire une capture d'écran : message d'erreur, application, code.
5. **`media.rs` : l'intensité traverse tout** (colonne SQL, `MediaItem`, `SearchQuery`, `relevant()`, `vocabulary()`, 21 tests). S'y ajoutent `story_sample`, `story_pool`, `import_legacy` et `by_keywords` (illustration automatique par mots-clés, avec tirage aléatoire photo/vidéo). 11 des 21 tests exercent `by_keywords` ou `story_*` et disparaissent avec eux ; les 10 autres sont conservés avec des fixtures informatiques. `relevant()` reste, sans plafond d'intensité. **Conséquence pour le thésaurus** : son seul appelant est aujourd'hui `by_keywords` (voir D3).
6. **`compact()` est dans `ws.rs`, pas dans `conversation.rs`**, qui ne fait que la persistance. Il reste. Sa consigne de résumé parle de « personnages », de « relation » et de « promesses » : elle est reformulée pour résumer un problème et les étapes déjà essayées, ce qui préremplira le ticket en phase 2.
7. **`ws.rs` est le fichier le plus intriqué** (environ 45 % à retirer, estimation) : avatar neuronal, carrefours d'histoire, propositions d'histoires, plafond d'intensité, tons de voix, illustration automatique. Ce qui reste, et que la phase 2 réutilisera : le cycle de session WebSocket, la génération en flux, la chaîne `relevant()` → liste `offered` → `[[show: #id]]` accepté seulement s'il a été proposé à ce tour (`allowed_pick`). C'est le garde-fou à reprendre pour `[[source: #id]]`. Restent aussi `[[replies: …]]`, `compact()` et la reprise de conversation.
8. **`models.rs` : les 22 entrées de modèles de texte** (16 au catalogue, 6 dans `legacy_catalog`) sont toutes non censurées ou orientées jeu de rôle, y compris le légendeur d'images. Restent 3 modèles Whisper et 1 Chatterbox. Le catalogue n'aura donc plus de modèle de texte tant que D4 n'est pas levé. `main.rs` prend aussi `gemma4-e4b-heretic` comme modèle par défaut de la commande `plan`.
9. **`backend/db.json` est versionné** alors qu'il figure dans `.gitignore` : il contient le hachage du mot de passe de `admin` et 3 scènes d'exemple. Il est supprimé (l'admin est recréé au premier lancement).
10. **Fichiers morts ou cassés** : `frontend/src/srcs/views/Admin/SceneList.vue` (doublon d'une ancienne version, dans un dossier mal nommé), `frontend/public/placeholder-thumb.png` (0 octet), `test_api.js` (utilise `axios`, absent des dépendances), `seed-db.js` (commandes MySQL), `build.xml` (Ant, jamais appelé), route `/api/admin/db-sync` (migrations MariaDB périmées, sans garde `isAuthenticated`). Dépendances inutilisées : `dotenv`, `fluent-ffmpeg`. Après nettoyage, `multer` et `ffmpeg-static` ne servent plus au serveur (`ffmpeg-static` reste à la racine pour `main.js`).
11. **Votre liste de contrôle manque des cas et en attrape à tort.** Elle ne voit pas le cas spécial de racinisation lié à la nudité dans `terms.rs`, les rôles par défaut du direct (« Son mari », « Son amant »…) dans `Live.vue` et `conversation.rs`, ni le commentaire « dirty talk » de `llm.rs`. À l'inverse, `explicit` attrape des usages techniques de « explicite » (`main.js`, `llm.rs`, `scripts/build-sidecars.sh`, `README.md`) : ils sont reformulés pour que le contrôle final reste à zéro sans l'assouplir. Une liste complémentaire est proposée (§6).
12. **Interface mi-français, mi-anglais** : `LoginModal.vue`, `UserManagement.vue`, `App.vue` et `rateLimiter.js` ont des textes anglais (« Username », « Skip to main content », « Too many requests »…). Ils sont traduits au passage, ces fichiers étant déjà modifiés.
13. **Licence d'AVTR-1** : le script d'installation indique « PolyForm Noncommercial : usage personnel seulement, pas de redistribution ». C'est bien incompatible avec un usage en DSI, comme vous l'aviez noté.

## 3. Décisions à prendre

### D1. Graphe d'histoire (scènes + choix) : **supprimer** — décidé, appliqué

- **Ce que c'est vraiment** : un éditeur et un lecteur de **vidéos**. Une scène est un fichier vidéo et une miniature (`video_path`, `thumbnail_path`) ; le serveur concatène des scènes avec ffmpeg, chaque chapitre a sa boucle d'ambiance, les choix portent un ton « dramatique » ou « positif », les scènes une « émotion » et une narration. Un nœud de diagnostic est du texte (question, réponses, solution) : du modèle actuel il ne resterait que `scenes(id, titre)` et `choices(source, destination, texte)`.
- **Côté Rust**, `storyplan.rs` est un planificateur de récit : il regroupe des médias qui se ressemblent et sépare des voies dramatique et positive. Rien à voir avec un arbre de diagnostic.
- **Ce que coûterait de le garder** : 5 028 lignes (Rust 640, backend 1 069, interface 3 237, deux scripts de la racine 82), 14 tests Rust, cinq routes de lecture sans connexion (`GET /api/scenes/:id`, `/api/scenes/:id/choices`, `/api/parts`, `/api/player/scenes/:id`, `/api/settings/background`), du téléversement de fichiers (`multer`) et ffmpeg côté serveur. C'est de la surface à maintenir et à sécuriser pour une fonction qui n'est pas dans le MVP.
- **Rien n'est perdu** : tout reste dans `virtual-story`, qui est privé. Si les arbres de diagnostic deviennent une priorité, mieux vaut les concevoir en texte (un document JSON ou YAML importé comme le reste de la base de connaissances, affiché par un petit composant Vue) que d'adapter des scènes vidéo.
- **Ce qui existe déjà** et couvre une partie du besoin : `[[replies: Oui | Non]]` (dans `director.rs`) permet des questions de triage guidées dans la conversation, sans graphe.
- **Contre-argument** : si des procédures à suivre à la lettre (« poste verrouillé », « mot de passe oublié ») doivent sortir avant la phase 3, on perd l'éditeur visuel existant. « Garder » reste possible, mais il faudrait de toute façon retirer toute la partie vidéo. Je ne le recommande pas.

### D2. `persona` devient « profil d'assistant » : libellés maintenant, identifiants plus tard — appliqué

Dès la phase 1, l'interface dit « Profil d'assistant ». Les identifiants de code (`persona.rs`, `/api/live/personas`, clé des conversations) restent inchangés pour garder les diffs relisibles. Le renommage mécanique (`persona` → `profile`) peut faire un commit à part quand vous voulez, y compris en phase 1 si vous préférez. Un profil par défaut « Repère » est créé au premier lancement (sinon l'écran de départ est vide).

### D3. Thésaurus sans `by_keywords` : option B — décidée, appliquée

Une fois `by_keywords` supprimé, `Thesaurus::triggers` et `doc_concepts` n'ont plus d'appelant, et clippy (`-D warnings`) refuse le code mort.
- **A** : garder `by_keywords` neutralisé (sans intensité ni tirage photo/vidéo). Ça conserve une illustration automatique sans objet pour du support.
- **B** : le supprimer, garder le thésaurus et ses tests, avec un `#[allow(dead_code)]` commenté jusqu'à ce que `relevant()` l'appelle en phase 2. Le test de rechargement du fichier de synonymes de l'utilisateur est conservé, appliqué directement au thésaurus.
- **C** : brancher dès maintenant le thésaurus dans `relevant()` (expansion de la requête, une trentaine de lignes). C'est le début de la phase 2, et ça change le comportement de la recherche.

**Appliqué (B).** « delete D3 » a été lu comme l'option B. `by_keywords`, `story_sample`, `story_pool` et `import_legacy` sont supprimés ; `Thesaurus` garde son mécanisme et ses tests, avec des `#[allow(dead_code)]` commentés tant que `MediaLibrary::relevant` ne le consulte pas (phase 2). Le dictionnaire livré est le jeu de départ de 61 concepts de jargon informatique.

### D4. Modèles du catalogue — **ouvert**

**Blocage** : `huggingface.co` n'est pas joignable d'ici (403 du proxy sortant). Pour avancer, au choix : autoriser `huggingface.co` dans les réglages réseau de l'environnement (menu de l'environnement cloud, dans la barre de titre de la session, puis *Edit* → *Network access* : un niveau plus large, ou le domaine ajouté sous *Allowed domains* en laissant cochée la case des gestionnaires de paquets), ou m'indiquer les dépôts que vous voulez.

**Ce que je proposerai, sans identifiant pour l'instant** : 2 à 3 modèles d'instruction **standard**, à licence claire (à lire sur la fiche de chaque modèle), bons en français, qui tiennent sur 12 Go avec le micro (Whisper sur processeur, réglage par défaut) :
1. un modèle de la famille Qwen 3 de 4 à 8 milliards de paramètres, en variante sans mode réflexion ;
2. un modèle de la famille Mistral de 8 à 12 milliards ;
3. un modèle de la famille Gemma de 4 à 12 milliards.

En quantification Q4_K_M ou équivalente (de l'ordre de 2,5 à 7,5 Go selon la taille, à confirmer sur les fichiers réels), plus un modèle qui voit les images (fichier « mmproj » du même dépôt) pour lire les captures d'écran. Chaque entrée n'est ajoutée qu'après avoir listé le dépôt et vérifié le nom exact du fichier.

**En attendant** : le catalogue livré ne contient que Whisper et Chatterbox. Un modèle de texte peut s'ajouter par `catalog.json` (mécanisme existant, fusionné au catalogue intégré), mais sans modèle l'assistant ne répond pas.

**État.** Aucun modèle de texte n'a été ajouté : `huggingface.co` est resté injoignable depuis l'environnement de reprise (403 du proxy sortant), donc aucun dépôt ni nom de fichier n'a été vérifié, et aucun identifiant n'a été écrit de mémoire. La procédure pour en ajouter un est dans [`live-engine/README.md`](../live-engine/README.md#ajouter-un-modèle-de-texte).

### D5. Licence — **ouvert**

Le dépôt Repère est sous licence MIT (créée avec lui, conservée). Le code copié vient d'un dépôt sous GPL-2.0 (texte non copié). `ffmpeg-static` est sous GPL-3.0-or-later (indiqué par le verrou npm), et le paquet redistribue des bibliothèques CUDA sous licence NVIDIA (README d'origine). AVTR-1 est retiré. Le détenteur des droits peut relicencier son propre code, pas celui de ses dépendances. **À confirmer par vous (et la DSI) avant toute diffusion du paquet.** Je ne change rien tant que vous ne dites rien.

### D6. Verrous de dépendances — appliqué

Deux familles (npm et pnpm) dans trois dossiers, soit environ 310 Ko de doublons. La CI et le script de build utilisent npm. **Recommandation** : supprimer les trois `pnpm-lock.yaml`, régénérer les trois `package-lock.json`. Fait ; le verrou de la racine n'était qu'un squelette de 147 lignes, il décrit maintenant les 627 paquets du projet.

### D7. Icône — appliqué (placeholder)

L'icône actuelle est un lecteur vidéo avec une bulle ; le favicon est une icône 32×32 générique. Je fournis un **placeholder** neutre, à remplacer par l'identité visuelle de la DSI.

### D8. Chemins de données, variables et identifiants — appliqué

| Avant | Après |
|---|---|
| `~/.config/Virtual Story` | `~/.config/repere` (dossier forcé sans accent, malgré le nom affiché « Repère ») |
| `~/.local/share/virtual-story` | `~/.local/share/repere` |
| variables `VS_*` | `REPERE_*` |
| cookie `vs.sid`, clés du navigateur `vs.*` | `repere.sid`, `repere.*` |
| `com.virtualstory.app`, catégorie de menu `AudioVideo` | `com.repere.app`, `Utility` |

Vos modèles déjà téléchargés (plusieurs Go) ne seront pas retrouvés tout seuls : déplacez le dossier une fois (`mv ~/.local/share/virtual-story ~/.local/share/repere`) ou pointez `REPERE_MODELS_DIR` dessus. Aucun code de migration n'est prévu.

## 4. Inventaire fichier par fichier

**Légende.** *garder* : inchangé hors renommage. *adapter* : le fichier reste, avec les retraits indiqués. *supprimer* : le fichier disparaît. ¹ **Occ.** : nombre de lignes du fichier d'origine contenant au moins un terme de la liste de contrôle.

### Racine, CI et documentation

_18 fichiers : 1 à garder, 10 à adapter, 7 à supprimer._

| Fichier | Décision | Lignes | Occ.¹ | Raison |
|---|---|--:|--:|---|
| `.Jules/palette.md` | **supprimer** | 49 | 3 | Journal de conception d'un agent, centré sur le lecteur d'histoire et l'éditeur de graphe |
| `.github/workflows/build.yml` | adapter | 211 | 3 | Construction du .deb à garder ; renommer paquet et artefact, retirer les scripts d'avatar du paquet |
| `.github/workflows/ci.yml` | adapter | 59 | 1 | Tests moteur, interface et proxy à garder ; retirer le job « avatar », garder les tests du serveur de voix |
| `.gitignore` | adapter | 51 |  | Garder db.json, uploads, target ; retirer ce qui concerne vidéos et avatars |
| `LICENSE` | **supprimer** | 339 | 1 | Non copié : le dépôt Repère a déjà sa licence (MIT) ; la GPL-2.0 d'origine est à reconsidérer (D5) |
| `README.md` | adapter | 408 | 41 | Réécriture complète (objectif, architecture, installation, développement) ; reprendre la partie construction du .deb |
| `bin/.gitkeep` | garder | 0 |  | Dossier des binaires (llama-server, whisper-server) rempli par scripts/build-sidecars.sh |
| `build.xml` | **supprimer** | 67 | 1 | Script Ant jamais appelé (ni CI, ni package.json, ni README) ; cibles pnpm et Windows périmées |
| `build/icon.png` | adapter | — |  | Icône « lecteur vidéo + bulle » : à remplacer par l'identité de Repère (D7) |
| `docs/images/admin_story_graph.png` | **supprimer** | — |  | Capture de l'éditeur de graphe d'histoire, à la marque Virtual Story |
| `main.js` | adapter | 180 | 5 | Fenêtre Electron et lancement du moteur à garder ; renommer `VS_*`, titres, chemins de données, boîte « choisir un dossier » |
| `package-lock.json` | adapter | 147 |  | À régénérer après renommage et retrait de dépendances |
| `package.json` | adapter | 129 | 8 | Renommer (nom, productName, appId, exécutable, catégorie, description) ; retirer dotenv, fluent-ffmpeg, multer |
| `pnpm-lock.yaml` | **supprimer** | 5 230 |  | Doublon du verrou npm utilisé par la CI (D6) |
| `preload.js` | adapter | 7 |  | Pont Electron `vsNative.pickFolder` à renommer |
| `run` | adapter | 46 | 2 | Lanceur de développement ; renommer les messages |
| `seed-db.js` | **supprimer** | 49 |  | Scènes d'exemple de l'histoire ; écrit en commandes MySQL qui ne fonctionnent plus |
| `test_api.js` | **supprimer** | 33 |  | Script manuel sur le graphe d'histoire ; utilise axios, absent des dépendances |

### `live-engine/src` (code Rust)

_29 fichiers : 6 à garder, 17 à adapter, 6 à supprimer._

| Fichier | Décision | Lignes | Occ.¹ | Raison |
|---|---|--:|--:|---|
| `live-engine/src/analyzer.rs` | adapter | 988 | 33 | Plomberie de vision réutilisable (client, état, redimensionnement, tâche annulable) ; prompts « sans censure », légendeur, mineurs/intensité et vidéo à retirer |
| `live-engine/src/api.rs` | adapter | 622 | 2 | Routes REST à garder ; retirer avatar, visages, histoire, import de l'ancienne base ; plus d'intensité sur les médias |
| `live-engine/src/config.rs` | adapter | 201 | 1 | Retirer la réserve VRAM de l'avatar (défaut 4 → 0 Go) et les commentaires « ma machine » ; température par défaut ≈ 0,3 |
| `live-engine/src/conversation.rs` | adapter | 139 |  | Persistance à garder (par utilisateur en phase 3) ; retirer `plot`, mode histoire, fixtures de jeu de rôle |
| `live-engine/src/director.rs` | adapter | 723 | 12 | Parseur `[[…]]`, `replies`, `ShowId` à garder ; retirer `voice`, `ambient`, `show` par tags/intensité, `suggest`, `choose`, `state` |
| `live-engine/src/engine.rs` | adapter | 1 017 | 15 | Superviseur llama/whisper/voix à garder ; retirer l'avatar (état, installation, mesure), `VS_*`, chemins `virtual-story` |
| `live-engine/src/engine/neural.rs` | **supprimer** | 372 | 4 | Client du serveur d'avatar neuronal AVTR-1 (licence non commerciale) |
| `live-engine/src/expression.rs` | **supprimer** | 229 | 5 | Tons de voix émotionnels, onomatopées, vocalisations |
| `live-engine/src/files.rs` | garder | 112 |  | Suppression de fichiers confinée au dossier d'uploads ; neutre |
| `live-engine/src/gguf.rs` | garder | 393 |  | Lecture des métadonnées GGUF pour le plan VRAM ; neutre |
| `live-engine/src/hardware.rs` | garder | 378 |  | Plan mémoire GPU et arguments de llama-server ; neutre |
| `live-engine/src/ideas.rs` | **supprimer** | 373 | 12 | Histoires à vivre inventées à partir des médias ; `Plot` à retirer de conversation, prompt et ws |
| `live-engine/src/importer.rs` | adapter | 882 | 5 | Parcours, empreinte anti-doublons, lien/copie, progression : socle de l'import de la phase 2 ; retirer humeur, intensité, ambiance |
| `live-engine/src/lexicon.rs` | **supprimer** | 179 | 15 | Niveaux de contenu sexuel, détection de mineurs et de refus ; `fold()` déplacé dans terms.rs (8 modules l'utilisent) |
| `live-engine/src/llm.rs` | garder | 199 | 1 | Client compatible OpenAI en flux ; un commentaire de test à neutraliser |
| `live-engine/src/main.rs` | adapter | 170 | 2 | Retirer modules supprimés, modèle par défaut de `plan`, nettoyage des personnages d'anciennes versions (remplacé par un profil par défaut « Repère » au premier lancement) ; nom dans le journal |
| `live-engine/src/media.rs` | adapter | 1 467 | 88 | Index, `relevant()`, requête paginée, empreintes : futur socle de la base de connaissances ; retirer intensité, exclusion, `story_*`, `by_keywords`, `import_legacy` |
| `live-engine/src/models.rs` | adapter | 1 030 | 48 | Téléchargement reprenable + SHA-256 à garder ; retirer les 22 entrées LLM, `Kind::Avatar`, `captioner` |
| `live-engine/src/persona.rs` | adapter | 504 | 10 | Devient le profil d'assistant (D2) : garder `Sampling`, champs libres, voix, message d'accueil ; retirer âge ≥ 18, sexe, sexualité, envies, sons, rôles, visage, avatar |
| `live-engine/src/prompt.rs` | adapter | 521 | 31 | Réécriture : règles de support ; retirer `EXPRESSION_RULES`, histoire, carrefours, fiche de personnage |
| `live-engine/src/state.rs` | adapter | 58 |  | Retirer `storygen` |
| `live-engine/src/storygen.rs` | **supprimer** | 1 109 | 11 | Histoire à embranchements écrite à partir des médias (tons, carrefours) |
| `live-engine/src/storyplan.rs` | **supprimer** | 640 |  | Planificateur de récit (fils de médias, voies dramatique/positive) ; sans rapport avec un arbre de diagnostic |
| `live-engine/src/synonyms.txt` | adapter | 122 | 3 | Contenu à remplacer : le dictionnaire livré est un lexique adulte ; jeu de départ de jargon informatique |
| `live-engine/src/terms.rs` | adapter | 118 |  | Racines et mots vides à garder ; reçoit `fold()` ; retirer le cas spécial de la nudité et les mots vides « photo/vidéo » |
| `live-engine/src/thesaurus.rs` | adapter | 282 | 1 | Mécanisme (concepts fort/lié, expressions) à garder ; tests réécrits sur le nouveau dictionnaire |
| `live-engine/src/voice.rs` | garder | 156 |  | Micro (whisper) et voix (API OpenAI), optionnels ; neutre |
| `live-engine/src/webp.rs` | garder | 188 |  | Première image d'un WebP animé (importer, analyzer) ; neutre, sortira avec l'import de médias |
| `live-engine/src/ws.rs` | adapter | 1 511 | 42 | Session, flux, `relevant()` → médias proposés → `ShowId`, `compact()` à garder ; retirer avatar, carrefours, histoires, intensité, tons de voix, illustration automatique |

### `live-engine` (configuration et documentation)

_4 fichiers : 1 à garder, 3 à adapter, 0 à supprimer._

| Fichier | Décision | Lignes | Occ.¹ | Raison |
|---|---|--:|--:|---|
| `live-engine/Cargo.lock` | garder | 2 074 |  | Verrou des dépendances Rust |
| `live-engine/Cargo.toml` | adapter | 26 | 1 | Description « pour Virtual Story » |
| `live-engine/README.md` | adapter | 413 | 50 | Réécriture : garder API, configuration, plan VRAM, construction ; retirer histoires, avatar, voix expressive, analyse de médias |
| `live-engine/live.example.toml` | adapter | 42 |  | Température 0,9 → ≈ 0,3 ; réserve VRAM ; commentaires |

### `backend` (Express)

_30 fichiers : 5 à garder, 12 à adapter, 13 à supprimer._

| Fichier | Décision | Lignes | Occ.¹ | Raison |
|---|---|--:|--:|---|
| `backend/.env.example` | adapter | 6 |  | Variables MariaDB obsolètes (la base est un fichier JSON) ; garder SESSION_SECRET |
| `backend/.gitignore` | garder | 25 |  | Ignore node_modules, .env et journaux |
| `backend/bootstrap.js` | adapter | 26 |  | Création de l'admin au premier lancement à garder ; retirer le réglage d'arrière-plan du lecteur |
| `backend/cleanup.js` | **supprimer** | 72 |  | Nettoyage des vidéos et miniatures orphelines |
| `backend/db.js` | adapter | 161 |  | Base JSON (alasql) à garder jusqu'à PostgreSQL (phase 3) ; ne garder que les tables users et settings |
| `backend/db.json` | **supprimer** | 56 |  | État d'exécution versionné par erreur (hachage du mot de passe admin, 3 scènes d'exemple) ; déjà dans .gitignore |
| `backend/init-db.js` | adapter | 36 |  | Initialisation de la base ; retirer le réglage d'arrière-plan |
| `backend/liveProxy.js` | garder | 70 |  | Proxy REST + WebSocket vers le moteur, session exigée pour toute écriture |
| `backend/liveProxy.test.js` | adapter | 37 |  | Tests de sécurité du proxy à garder ; remplacer les URL `/faces/…` par une autre route d'écriture |
| `backend/middleware/auth.js` | garder | 8 |  | Garde de session ; point d'appui de l'OIDC (phase 3) |
| `backend/middleware/rateLimiter.js` | adapter | 19 |  | Limiteurs à garder ; message anglais à traduire |
| `backend/package-lock.json` | adapter | 1 438 |  | À régénérer après retrait de dépendances |
| `backend/package.json` | adapter | 27 |  | Renommer ; script de test ; retirer dotenv, fluent-ffmpeg, ffmpeg-static, multer (inutilisés après nettoyage) |
| `backend/packageDeps.test.js` | garder | 18 |  | Contrôle que les dépendances du serveur figurent aussi à la racine (paquet Electron) |
| `backend/pnpm-lock.yaml` | **supprimer** | 2 930 |  | Doublon du verrou npm (D6) |
| `backend/remise_zero.py` | garder | 32 |  | Réinitialisation de la base locale ; neutre |
| `backend/routes/admin.js` | adapter | 224 |  | Garder utilisateurs et mot de passe ; retirer db-sync (migrations MariaDB, route sans authentification), arrière-plan, miniatures, graphe |
| `backend/routes/auth.js` | adapter | 46 |  | Connexion/déconnexion à garder ; renommer le cookie de session |
| `backend/routes/choices.js` | **supprimer** | 16 |  | Suppression d'un choix du graphe d'histoire |
| `backend/routes/parts.js` | **supprimer** | 103 |  | Chapitres de l'histoire (vidéo de boucle d'ambiance) |
| `backend/routes/player.js` | **supprimer** | 104 |  | Lecteur d'histoire : scène courante, choix, chapitre |
| `backend/routes/scenes.js` | **supprimer** | 371 |  | Scènes vidéo : téléversement, concaténation ffmpeg, import, histoire écrite par l'IA, choix |
| `backend/routes/settings.js` | **supprimer** | 15 |  | Arrière-plan du lecteur d'histoire |
| `backend/server.js` | adapter | 132 | 1 | Serveur Express à garder ; retirer routes et dossiers vidéo/miniatures/arrière-plans ; cookie et secret par défaut à renommer |
| `backend/utils/config.js` | **supprimer** | 26 |  | Dossiers et filtre de téléversement des vidéos |
| `backend/utils/paths.js` | adapter | 11 |  | Emplacement des données ; `VS_DATA_DIR` devient `REPERE_DATA_DIR` |
| `backend/utils/storyGraph.js` | **supprimer** | 80 |  | Validation d'une histoire à embranchements (médias, tons, carrefours) |
| `backend/utils/storyGraph.test.js` | **supprimer** | 72 |  | Tests de storyGraph.js |
| `backend/utils/videoUtils.js` | **supprimer** | 122 |  | ffmpeg : miniatures, concaténation de scènes, suppression de scène |
| `backend/utils/videoUtils.test.js` | **supprimer** | 88 |  | Tests de videoUtils.js |

### `frontend` (Vue 3)

_59 fichiers : 10 à garder, 18 à adapter, 31 à supprimer._

| Fichier | Décision | Lignes | Occ.¹ | Raison |
|---|---|--:|--:|---|
| `frontend/.gitignore` | garder | 36 |  | Fichiers générés et éditeurs |
| `frontend/.vscode/extensions.json` | garder | 3 |  | Recommande l'extension Vue |
| `frontend/README.md` | **supprimer** | 38 |  | Modèle Vite générique en anglais ; le README racine couvre le développement |
| `frontend/index.html` | adapter | 13 |  | Titre « Vite App » et langue vide : `Repère`, `lang="fr"` |
| `frontend/jsconfig.json` | garder | 8 |  | Alias `@` |
| `frontend/package-lock.json` | adapter | 2 882 |  | À régénérer |
| `frontend/package.json` | adapter | 25 |  | Renommer ; script `test` réduit aux fichiers conservés |
| `frontend/pnpm-lock.yaml` | **supprimer** | 1 850 |  | Doublon du verrou npm (D6) |
| `frontend/public/favicon.ico` | adapter | — |  | Favicon générique 32×32 : à remplacer (D7) |
| `frontend/public/placeholder-thumb.png` | **supprimer** | — |  | Fichier vide (0 octet), ne sert qu'au graphe d'histoire |
| `frontend/src/App.vue` | adapter | 123 | 1 | Garder session et connexion ; retirer chapitres et lien Player ; menu admin : utilisateurs, modèles, profils ; textes anglais à franciser |
| `frontend/src/assets/styles/App.css` | adapter | 138 |  | Styles généraux à garder ; retirer ceux des chapitres |
| `frontend/src/assets/styles/Home.css` | adapter | 82 |  | Cartes histoire / reprise de partie à remplacer par l'accueil de l'assistant |
| `frontend/src/assets/styles/LoginModal.css` | garder | 141 |  | Fenêtre de connexion |
| `frontend/src/assets/styles/Player.css` | **supprimer** | 275 |  | Lecteur d'histoire |
| `frontend/src/assets/styles/SceneEdit.css` | **supprimer** | 314 |  | Éditeur de scène |
| `frontend/src/assets/styles/SceneList.css` | **supprimer** | 358 |  | Graphe d'histoire |
| `frontend/src/assets/styles/SceneNode.css` | **supprimer** | 89 |  | Nœud du graphe |
| `frontend/src/assets/styles/UserManagement.css` | garder | 146 |  | Gestion des utilisateurs |
| `frontend/src/components/LoginModal.vue` | adapter | 139 |  | Connexion à garder ; textes anglais à franciser |
| `frontend/src/components/SceneNode.vue` | **supprimer** | 95 |  | Nœud du graphe d'histoire |
| `frontend/src/components/StoryBeatCard.vue` | **supprimer** | 50 |  | Carte d'étape de l'histoire écrite par l'IA |
| `frontend/src/components/StoryGenerator.vue` | **supprimer** | 262 | 4 | Écran de génération d'histoire par l'IA |
| `frontend/src/components/StoryHost.vue` | **supprimer** | 686 | 7 | Animateur d'histoire : avatar, émotions, voix, carrefours |
| `frontend/src/components/face/FaceCalibrator.vue` | **supprimer** | 416 |  | Calibrage du visage animé |
| `frontend/src/components/face/NeuralFace.vue` | **supprimer** | 74 | 2 | Affichage de l'avatar neuronal AVTR-1 |
| `frontend/src/components/face/TalkingFace.vue` | **supprimer** | 1 127 |  | Visage 2D animé (bouche synchronisée sur la voix) |
| `frontend/src/main.js` | garder | 18 |  | Point d'entrée Vue |
| `frontend/src/router/index.js` | adapter | 111 |  | Retirer routes lecteur, scènes, médiathèque ; garder garde admin, rechargement après mise à jour ; clé `vs.reloaded` |
| `frontend/src/srcs/views/Admin/SceneList.vue` | **supprimer** | 96 |  | Doublon d'une ancienne version, dans un dossier mal nommé |
| `frontend/src/utils/audio.js` | garder | 135 |  | Enregistrement du micro en WAV 16 kHz |
| `frontend/src/utils/autosave.js` | garder | 73 |  | Enregistrement automatique générique ; sans écran jusqu'à la base de connaissances (phase 2) |
| `frontend/src/utils/autosave.test.mjs` | garder | 150 |  | Tests d'autosave.js |
| `frontend/src/utils/emotion.js` | **supprimer** | 48 |  | Couleurs et émoticônes d'émotion de l'avatar |
| `frontend/src/utils/emotion.test.mjs` | **supprimer** | 20 |  | Tests d'emotion.js |
| `frontend/src/utils/face/mouth.js` | **supprimer** | 383 |  | Synchronisation de la bouche du visage animé |
| `frontend/src/utils/face/mouth.test.mjs` | **supprimer** | 64 |  | Tests de mouth.js |
| `frontend/src/utils/face/rig.js` | **supprimer** | 148 |  | Repères du visage animé |
| `frontend/src/utils/face/rig.test.mjs` | **supprimer** | 47 | 1 | Tests de rig.js |
| `frontend/src/utils/image.js` | **supprimer** | 25 |  | Préparation d'une image de visage |
| `frontend/src/utils/localFiles.js` | **supprimer** | 46 |  | IndexedDB pour l'« image par défaut » du direct |
| `frontend/src/utils/mediaRows.js` | adapter | 59 | 4 | Fusion d'une liste rechargée avec les lignes en cours d'édition : réutilisable pour la base de connaissances ; retirer humeur, intensité, ambiance |
| `frontend/src/utils/mediaRows.test.mjs` | adapter | 97 | 10 | Tests à réécrire sans humeur ni intensité |
| `frontend/src/utils/neuralStage.js` | **supprimer** | 62 | 1 | File d'images de l'avatar neuronal |
| `frontend/src/utils/neuralStage.test.mjs` | **supprimer** | 74 |  | Tests de neuralStage.js |
| `frontend/src/utils/speaker.js` | garder | 135 |  | Lecture audio en file (voix de l'IA) |
| `frontend/src/utils/storage.js` | adapter | 14 |  | localStorage tolérant à garder ; clés `vs.*` renommées, commentaire sur l'histoire retiré |
| `frontend/src/utils/storyGraph.js` | **supprimer** | 94 |  | Vue et édition des étapes d'une histoire |
| `frontend/src/utils/storyGraph.test.mjs` | **supprimer** | 104 |  | Tests de storyGraph.js |
| `frontend/src/views/Admin/LiveMedia.vue` | **supprimer** | 695 | 19 | Médiathèque vidéo/photo côté interface (suppression demandée) ; refaite en base de connaissances en phase 2 |
| `frontend/src/views/Admin/LiveModels.vue` | adapter | 261 | 6 | Modèles, moteur, jeton Hugging Face à garder ; retirer carte avatar AVTR-1, vision « médiathèque », légendeur |
| `frontend/src/views/Admin/LivePersonas.vue` | adapter | 448 | 14 | Devient « Profil d'assistant » ; retirer âge, sexe, sexualité, envies, sons, rôles, visage, avatar, style « explicite » |
| `frontend/src/views/Admin/SceneEdit.vue` | **supprimer** | 622 |  | Éditeur de scène vidéo |
| `frontend/src/views/Admin/SceneList.vue` | **supprimer** | 611 |  | Éditeur du graphe d'histoire |
| `frontend/src/views/Admin/UserManagement.vue` | adapter | 134 |  | Utilisateurs et mot de passe à garder ; interface en anglais à franciser ; lien de retour vers /admin/scenes |
| `frontend/src/views/Home.vue` | adapter | 62 | 1 | Accueil histoire/direct à remplacer par l'entrée vers l'assistant |
| `frontend/src/views/Live.vue` | adapter | 902 | 10 | Réécriture du gabarit en chat de support ; garder connexion WebSocket, flux, micro, voix, réponses rapides ; retirer scène, avatar, médias, histoires |
| `frontend/src/views/Player.vue` | **supprimer** | 579 |  | Lecteur d'histoire interactive |
| `frontend/vite.config.js` | adapter | 57 |  | Garder les proxys /api et /api/live ; retirer /videos, /thumbnails, /parts, /photos, /backgrounds |

### `scripts`

_16 fichiers : 1 à garder, 8 à adapter, 7 à supprimer._

| Fichier | Décision | Lignes | Occ.¹ | Raison |
|---|---|--:|--:|---|
| `scripts/avatar/avatar_server.py` | **supprimer** | 395 | 16 | Serveur d'avatar neuronal AVTR-1 |
| `scripts/avatar/probe-avtr1.sh` | **supprimer** | 156 | 16 | Installation et mesure d'AVTR-1 (licence non commerciale) |
| `scripts/avatar/tests/fake/avtr1_renderer/__init__.py` | **supprimer** | 0 |  | Faux moteur AVTR-1 pour les tests |
| `scripts/avatar/tests/fake/avtr1_renderer/avatar_loader.py` | **supprimer** | 23 |  | Faux moteur AVTR-1 pour les tests |
| `scripts/avatar/tests/fake/avtr1_renderer/pipeline.py` | **supprimer** | 39 |  | Faux moteur AVTR-1 pour les tests |
| `scripts/avatar/tests/fake/avtr1_renderer/types.py` | **supprimer** | 24 | 1 | Faux moteur AVTR-1 pour les tests |
| `scripts/avatar/tests/test_avatar_server.py` | **supprimer** | 242 | 4 | Tests du serveur d'avatar |
| `scripts/build-linux.sh` | adapter | 54 |  | Chaîne de construction à garder ; commentaire sur le verrou pnpm selon D6 |
| `scripts/build-sidecars.sh` | adapter | 174 | 2 | Retirer l'installation des scripts d'avatar |
| `scripts/doctor.sh` | adapter | 64 |  | `VS_MODELS_DIR` devient `REPERE_MODELS_DIR` |
| `scripts/set-version.mjs` | adapter | 28 | 1 | Nom de paquet dans le commentaire |
| `scripts/tts/chatterbox_server.py` | adapter | 270 | 1 | Chemins `virtual-story` renommés |
| `scripts/tts/setup-tts.sh` | adapter | 44 | 1 | `VS_TTS_VENV` et chemins renommés |
| `scripts/tts/test_chatterbox_server.py` | garder | 185 |  | Tests du serveur de voix |
| `scripts/tts/tts-server` | adapter | 18 | 2 | `VS_TTS_VENV` et chemins renommés |
| `scripts/verify-package.sh` | adapter | 47 | 1 | Nom de l'exécutable ; retirer le contrôle du serveur d'avatar |

## 4 bis. Écarts entre le plan et l'exécution

Les tableaux ci-dessus ont été suivis, avec ces écarts, tous voulus :

| Fichier | Plan | Fait | Pourquoi |
|---|---|---|---|
| `frontend/src/views/Live.vue` | retirer scène, avatar, médias, histoires | le panneau « source affichée » est conservé (image ou vidéo, légende, plein écran) | c'est ce qu'affiche `[[show: #id]]` ; la phase 2 y mettra la citation des sources |
| `frontend/vite.config.js` | retirer `/videos`, `/thumbnails`, `/parts`, `/photos`, `/backgrounds` | `/videos`, `/thumbnails` et `/photos` conservés ; `/parts` et `/backgrounds` retirés | mêmes médias que ci-dessus |
| `backend/server.js` | retirer les dossiers vidéo, miniatures et arrière-plans ; renommer le secret de session | `/videos`, `/thumbnails` et `/photos` conservés (statiques), `/parts` et `/backgrounds` retirés ; sans `SESSION_SECRET`, le secret est tiré au hasard à chaque démarrage | idem ; plus aucune clé connue de tous dans le dépôt |
| `backend/db.js` | garder les tables `users` et `settings` | seule `users` reste | `settings` ne servait qu'à l'arrière-plan du lecteur |
| `live-engine/src/director.rs` | retirer `voice`, `ambient`, `suggest`, `choose`, `state` | de plus, `SpokenFilter` est remplacé par `for_speech` | l'ancien filtre supprimait le texte entre astérisques (les gestes du jeu de rôle) : une phrase en italique n'aurait pas été lue |
| `live-engine/src/config.rs` | retirer la réserve VRAM de l'avatar, température ≈ 0,3 | de plus, le nombre de fils de `whisper-server` dépend des cœurs (la moitié, de 2 à 16) au lieu de 16 fixes | la valeur d'origine était réglée sur une machine précise |
| `package-lock.json` (racine) | « à régénérer » | régénéré en entier | l'ancien n'était qu'un squelette de 147 lignes |
| `build/icon.png`, `frontend/public/favicon.ico` | « à remplacer » (D7) | placeholders neutres (un réticule) | l'identité visuelle de la DSI n'existe pas encore |

## 5. Déroulement

Les sept étapes ont été exécutées dans cet ordre :

1. suppressions validées, puis corrections pour que tout compile ;
2. réécriture des fichiers à adapter, dans l'ordre des dépendances (`terms.rs` reçoit `fold`, puis `media.rs`, le thésaurus, `director.rs`, `persona.rs` et `prompt.rs`, `ws.rs`, `analyzer.rs`, `importer.rs`, le reste du moteur, `models.rs`), avec `cargo test` et clippy après chaque module ;
3. interface, backend, scripts, CI ;
4. renommage Virtual Story → Repère (D8) ;
5. README, README du moteur, ce document ;
6. vérifications (§6) ;
7. historique Git.

**Historique Git.** Le dépôt est public : la copie brute n'est jamais entrée dans l'historique. Rien n'a été commité avant la fin du nettoyage et du renommage, donc le premier commit contient déjà l'arbre propre. Les commits sont découpés par composant, chacun déjà renommé et propre au sens de la recherche du §6 :

1. moteur de conversation (`live-engine/`) ;
2. serveur Express (`backend/`) ;
3. interface Vue (`frontend/`) ;
4. application Electron, paquet Ubuntu, CI et scripts (racine, `build/`, `scripts/`, `.github/`) ;
5. documentation (README, inventaire).

Les modèles de texte vérifiés (D4) viendront dans un commit à part.

À la fin de la phase 1, l'assistant n'a **aucune source** tant que des images décrites n'ont pas été importées (la base de documents est la phase 2) : avec les règles de support, il répond « je ne sais pas » et propose un ticket. C'est voulu.

## 6. Vérifications finales

Résultats sur l'arbre final, avant le premier commit :

| Contrôle | Résultat |
|---|---|
| `cargo test --locked`, après recompilation du moteur | 140 tests, tous verts |
| `cargo clippy --locked --all-targets -- -D warnings`, après recompilation | sans avertissement |
| interface : `npm ci` depuis le verrou, `npm test`, `npm run build` | 15 tests verts ; build réussi |
| backend : `npm ci` depuis le verrou, `npm test` | 4 tests verts |
| racine : `npm ci --ignore-scripts` depuis le verrou | 627 paquets installés |
| `python3 -I -m unittest discover -s scripts/tts` | 10 tests verts |
| `bash -n` sur `run` et les scripts shell, `node --check` sur `main.js`, `preload.js` et `scripts/set-version.mjs` | sans erreur |
| parcours complet dans Chromium | 40 contrôles verts ; aucune erreur dans la console |

Le parcours complet démarre le vrai moteur, le vrai Express et l'interface compilée, avec un faux serveur compatible OpenAI à la place de `llama-server`. Il vérifie : l'accueil, le message d'accueil du profil, une question avec la source affichée (la capture est servie par Express), les boutons de réponse rapide, le « je ne sais pas » sans source, le contenu exact des prompts envoyés au modèle (règles de support, source proposée, prénom, température), la reprise de la conversation après rechargement, la connexion, les utilisateurs, le profil d'assistant, l'écran des modèles, et les accès refusés sans session. L'assistant n'a **pas** été essayé avec un vrai modèle.

### Recherche de la liste de contrôle

Insensible à la casse, sur le contenu puis sur les noms de fichiers, hors ce document :

```
grep -rniE 'sexe|sexualit|intime|explicit|intensit|gémiss|onomatop|heretic|abliter|sutra|avtr|lifeselector|virtual-story|virtual story' . --exclude-dir=.git --exclude=INVENTAIRE.md
find . -path ./.git -prune -o -print | grep -niE '<les mêmes motifs>'
```

Résultat : **aucun nom de fichier**. Dans le contenu, 11 lignes, toutes dans `package-lock.json` (racine et `backend/`), toutes le paquet npm `isexe` (« is executable », dépendance de `which`, donc de `cross-spawn`, d'electron-builder et de node-gyp) : le motif `sexe` en est une sous-chaîne. Sans `isexe`, la recherche ne renvoie rien. Pour qu'elle renvoie zéro tout court, écrire `\bsexe` ou exclure `package-lock.json`.

```
./backend/package-lock.json:1447:        "isexe": "^2.0.0"
./backend/package-lock.json:2116:    "node_modules/isexe": {
./backend/package-lock.json:2118:      "resolved": "https://registry.npmjs.org/isexe/-/isexe-2.0.0.tgz",
./package-lock.json:3034:    "node_modules/cross-spawn/node_modules/isexe": {
./package-lock.json:3036:      "resolved": "https://registry.npmjs.org/isexe/-/isexe-2.0.0.tgz",
./package-lock.json:3048:        "isexe": "^2.0.0"
./package-lock.json:4827:    "node_modules/isexe": {
./package-lock.json:4829:      "resolved": "https://registry.npmjs.org/isexe/-/isexe-3.1.5.tgz",
./package-lock.json:6019:    "node_modules/node-gyp/node_modules/isexe": {
./package-lock.json:6021:      "resolved": "https://registry.npmjs.org/isexe/-/isexe-4.0.0.tgz",
./package-lock.json:6049:        "isexe": "^4.0.0"
./package-lock.json:7993:        "isexe": "^3.1.1"
```

**Liste complémentaire** (mêmes règles, hors verrous), pour les cas que la liste de contrôle ne voit pas :

```
\bnu(e|es|s)?\b|nudit|\bseins?\b|lingerie|bikini|éroti|sensuel|orgasm|fellation|pénétr|sodomi|masturb|porn|nsfw|bdsm|fantasm|dirty talk|décensur|non[ -]censur|sans (aucune )?censure|uncensor|joycaption|jeu de rôle|roleplay|\brp\b|amant|son mari|mineur|ultra-?rp|nymphaea|hauhau|rocinante|cydonia
```

Elle ne renvoie rien dans les sources du projet. Avec le motif `érot` du premier jet, elle attrapait « numérotées » ; il est devenu `éroti`. Le verrou `Cargo.lock`, exclu de la recherche, contient la crate `nu-ansi-term` (mise en forme des journaux), que `\bnu\b` attrape sans rapport.

## 7. Points d'attention pour les phases 2 et 3

- **Sécurité à traiter avant tout déploiement serveur.** Le compte `admin` / `admin` est créé au premier lancement, et la fenêtre de connexion l'affiche. Le WebSocket `/api/live/ws` et toutes les lectures REST du moteur sont ouverts sans session : `liveProxy.js` ne protège que les écritures. Tout utilisateur connecté peut créer ou supprimer des comptes (pas de rôles). Les sessions sont gardées en mémoire (`MemoryStore`), et le serveur Express écoute sur `0.0.0.0` par défaut hors de l'application Electron (qui impose `127.0.0.1`). Le moteur n'accepte que des origines locales (`origin_allowed`). Rien de tout cela ne gêne une application de bureau locale ; tout cela doit changer avec l'OIDC et le serveur. Déjà traité en phase 1 : le secret de session n'est plus une valeur écrite dans le code.
- **Conversations** : une par profil (fichier JSON dont le nom est l'identifiant du profil), partagée par tous les utilisateurs du poste. Elles passent par utilisateur en phase 3.
- **Voix** (traité) : `SpokenFilter` supprimait ce qui est entre astérisques. `for_speech` ne retire que la mise en forme (gras, listes, émojis), et les marques d'étape « 1. » ne coupent plus une phrase.
- **Température** (traité) : 0,3 par défaut, dans la configuration et dans le profil « Repère », pour limiter l'invention.
- **Réserve de VRAM** (traité) : `hardware.vram_other_models_gb` vaut 0 par défaut (le micro et la voix sont comptés à part) ; le modèle et son contexte gagnent environ 4 Go sur une carte de 12 Go.
- **Réutilisé plus tard** : `autosave.js` et `mediaRows.js` restent sans écran jusqu'à la base de connaissances. `MediaLibrary` et `MediaItem` gardent leurs noms et leurs types `video` / `photo` jusqu'à la bascule vers documents et passages. `files.rs` et `webp.rs` sortiront avec l'import de médias. Le thésaurus attend d'être branché dans `relevant()` (phase 2).
