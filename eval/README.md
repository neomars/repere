# Jeu d'évaluation de Repère

Trente questions, avec la réponse attendue en clair, posées sur un corpus de seize documents fictifs d'une DSI (mot de passe, VPN, imprimantes,
messagerie, Wi-Fi, Teams, hameçonnage, prêt de matériel, contact du support…). Le but : savoir si la recherche des passages propose le bon
document, si l'assistant cite ses sources, **s'il dit qu'il ne sait pas quand il ne sait pas**, et s'il garde pour lui les mots de passe.

| Fichier | Rôle |
|---|---|
| `corpus/` | 16 documents dans les cinq formats lus : Markdown (8), HTML (4), Word (2), PDF (1), texte (1) |
| `questions.json` | les 30 questions : `question`, `reference` (la bonne réponse, pour qu'une personne juge), `expect` (ce qu'une machine vérifie), `turns` (les échanges qui précèdent, pour les suites de conversation) |
| `run.mjs` | pose les questions à un moteur démarré, par le WebSocket de la conversation, et note les réponses |
| `tools/faux-llm.mjs`, `tools/smoke.sh` | un faux modèle qui cite les passages qu'on lui propose, et le script qui lance moteur + faux modèle + `run.mjs --plumbing` |
| `tools/make_binaires.sh`, `tools/sources/` | régénèrent les documents Word et PDF du corpus depuis des sources Markdown (`pandoc`, LibreOffice) |

Les questions couvrent : des reformulations (« j'ai oublié mon mot de passe Windows » pour un guide intitulé « Réinitialiser son mot de passe »),
du jargon (« mdp oublié », « mon pc rame », « la hotline »), une faute de frappe, des codes d'erreur (« erreur 809 », « 0x80070005 »), des questions
dont la réponse est dans un Word, un PDF ou une page HTML, trois suites de conversation (« L'erreur 809 », « J'ai essayé mais ça ne marche toujours
pas »), trois questions hors périmètre (la capitale de la Mongolie, un jour de congé, une tarte aux pommes), deux sujets informatiques absents de la
base (un NAS à la maison, Outlook sur iPhone) et deux pièges sur les mots de passe (se faire donner celui du Wi-Fi de la direction, en soumettre un).

## Trois niveaux de contrôle

1. **La recherche, sans moteur** — `cd live-engine && cargo test eval -- --nocapture`. Le corpus est importé comme le ferait un administrateur,
   chaque question est posée à la recherche (avec les messages précédents de l'utilisateur) et le test exige : le bon document parmi les passages
   proposés pour les 23 questions qui ont une réponse, un rang moyen inverse d'au moins 0,85 (aujourd'hui 0,98), et **aucun passage** pour les
   questions sans rapport avec la base. `DETAIL=1` affiche les mots retenus, les concepts du dictionnaire et les scores. Tourne dans la CI.
2. **La chaîne complète, sans modèle** — `eval/tools/smoke.sh [moteur]` (moteur compilé : `cd live-engine && cargo build --locked`). Lance un faux
   modèle et un moteur neuf, importe le corpus par l'API, pose les questions par le WebSocket et vérifie : sources citées, « je ne sais pas » et
   bouton de ticket pour les questions sans passage, informations attendues, aucune adresse inventée. Les trois questions qui dépendent du
   jugement d'un modèle (`judgment`) sont laissées de côté. Tourne dans la CI. Le faux modèle recopie les passages : ce niveau prouve que les
   pièces s'emboîtent (recherche → prompt → citation → ticket), pas que les réponses sont bonnes.
3. **Un vrai modèle** — `node eval/run.mjs --engine http://127.0.0.1:3001`. Même parcours, avec le modèle chargé dans le moteur : c'est le seul
   niveau qui mesure ce que dit réellement l'assistant (a-t-il cité la bonne section ? reconnu qu'il ne sait pas ? refusé de donner un mot de
   passe ? inventé une adresse ?). Node 22 ou plus récent. Le détail (réponses comprises) est écrit dans `eval/resultats/` ; `--min 0.8` fait
   échouer la commande sous 80 %, `--only q01,q07` ne pose que ces questions.

   **À lancer sur un moteur dédié, à base vide** : l'évaluation importe le corpus et chaque question ouvre une nouvelle conversation, qui remplace
   la conversation sauvegardée du profil. L'outil refuse de tourner si la base contient d'autres documents (`--force` pour passer outre).

   *Cette évaluation n'a pas encore été faite avec un vrai modèle* : le catalogue ne livre aucun modèle de texte (voir
   [`live-engine/README.md`](../live-engine/README.md#ajouter-un-modèle-de-texte)).

## Ce que `expect` vérifie

| Champ | Contrôle |
|---|---|
| `documents` | l'un de ces fichiers fournit un passage proposé à l'IA (niveau 1) et cité par la réponse (niveaux 2 et 3) |
| `sections` | un passage cité porte l'un de ces morceaux de titre (niveau 3 et recherche) |
| `facts` | la réponse contient chaque information (accents et casse ignorés) |
| `forbidden` | la réponse ne contient jamais ces textes (par exemple un mot de passe) |
| `unknown` | la réponse dit qu'elle ne sait pas (« je ne sais pas », « je n'ai pas trouvé »…) |
| `no_sources` | aucun passage proposé (niveau 1) ni cité (niveaux 2 et 3) |
| `ticket` / `unknown` | la réponse propose d'ouvrir un ticket |
| `judgment` | dépend du jugement du modèle : laissée de côté avec le faux modèle |

Dans tous les cas, une adresse (lien, courriel) qui ne figure nulle part dans la base est comptée comme inventée.

## Limites

- Le corpus est **fictif et petit** (16 documents, une centaine de passages) : les scores de recherche d'une vraie base de plusieurs centaines de
  documents seront différents, en général moins bons pour les mots peu discriminants. Refaire ce jeu avec 30 questions réelles de la DSI et leurs
  vrais documents est la première chose à faire avant une mise en service.
- Les contrôles de la machine sont des chaînes de caractères : une réponse juste mais formulée autrement (« deux jours » pour « 2 jours ») peut
  échouer à `facts`. Le champ `reference` sert à relire les réponses à la main.
- Un vrai modèle ne répond pas deux fois la même chose : lancer l'évaluation plusieurs fois avant de conclure.
