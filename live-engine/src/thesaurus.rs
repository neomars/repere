//! Dictionnaire de synonymes : rapproche ce qui se dit (« mdp », « password ») de ce qu'écrivent les textes de la base
//! (« mot de passe »). Un « concept » regroupe des mots de même sens, en français et en anglais ; dès qu'un texte en contient un,
//! tout texte dont le titre, les tags ou la description en contiennent un autre est candidat. Des mots seulement « liés » (le
//! signal pour le Wi-Fi) ne déclenchent rien, mais classent mieux les textes qui les contiennent.
//!
//! Le dictionnaire de base est `synonyms.txt` (compilé dans le moteur) ; l'utilisateur le complète avec son propre fichier
//! `synonyms.txt`, dans le dossier de données, au même format (voir l'en-tête du fichier). Les mots sont réduits à leur racine
//! comme partout ailleurs (`terms.rs`) : accents, majuscules, pluriels et conjugaisons courantes ne comptent pas.
//!
//! Ces concepts servent à la recherche (`search.rs`) : la question évoque des concepts, et chaque texte de la base est indexé avec
//! ceux qu'il contient.

use crate::terms::{norm_tag, terms};
use std::collections::HashMap;

const BUILTIN: &str = include_str!("synonyms.txt");

/// Des mots de même sens (« forts ») et des mots liés (« faibles »), réduits à leurs racines.
#[derive(Debug, Clone)]
struct Concept {
    /// Le premier mot de la ligne, tel qu'écrit : c'est le nom du concept (« mot de passe »).
    name: String,
    strong: Vec<Vec<String>>,
    weak: Vec<Vec<String>>,
}

/// Mots ou expressions → concepts. Un mot isolé se retrouve par sa racine ; une expression (« réseau sans fil »), par sa première
/// racine puis par la suite des suivantes.
#[derive(Debug, Default)]
pub struct Thesaurus {
    concepts: Vec<Concept>,
    single_strong: HashMap<String, Vec<u32>>,
    single_weak: HashMap<String, Vec<u32>>,
    multi_strong: HashMap<String, Vec<(u32, Vec<String>)>>,
    multi_weak: HashMap<String, Vec<(u32, Vec<String>)>>,
}

/// Une ligne du fichier : « mot, mot, mot ~ mot lié, mot lié ».
fn parse_line(line: &str) -> Option<(String, Vec<String>, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (strong, weak) = line.split_once('~').unwrap_or((line, ""));
    let members = |s: &str| s.split(',').map(|m| m.trim().to_lowercase()).filter(|m| !m.is_empty()).collect::<Vec<_>>();
    let strong = members(strong);
    let name = strong.first()?.clone();
    Some((name, strong, members(weak)))
}

impl Thesaurus {
    /// Le dictionnaire de base, complété (ou non) par le texte d'un fichier de l'utilisateur.
    pub fn with_extra(extra: &str) -> Self {
        let mut concepts: Vec<Concept> = vec![];
        let mut by_name: HashMap<String, usize> = HashMap::new();
        for line in BUILTIN.lines().chain(extra.lines()) {
            let Some((name, strong, weak)) = parse_line(line) else { continue };
            let stems = |list: &[String]| list.iter().map(|m| terms(m)).filter(|s| !s.is_empty()).collect::<Vec<_>>();
            // une ligne qui commence par le nom d'un concept existant s'y ajoute
            let i = *by_name.entry(norm_tag(&name)).or_insert_with(|| {
                concepts.push(Concept { name: name.clone(), strong: vec![], weak: vec![] });
                concepts.len() - 1
            });
            let c = &mut concepts[i];
            for m in stems(&strong) {
                if !c.strong.contains(&m) {
                    c.strong.push(m);
                }
            }
            for m in stems(&weak) {
                if !c.weak.contains(&m) && !c.strong.contains(&m) {
                    c.weak.push(m);
                }
            }
        }
        concepts.retain(|c| !c.strong.is_empty());
        let mut th = Thesaurus { concepts, ..Default::default() };
        for (i, c) in th.concepts.iter().enumerate() {
            let i = i as u32;
            for (list, single, multi) in [(&c.strong, &mut th.single_strong, &mut th.multi_strong), (&c.weak, &mut th.single_weak, &mut th.multi_weak)] {
                for m in list {
                    match m.as_slice() {
                        [w] => single.entry(w.clone()).or_default().push(i),
                        [first, ..] => multi.entry(first.clone()).or_default().push((i, m.clone())),
                        [] => {}
                    }
                }
            }
        }
        th
    }

    #[cfg(test)]
    pub fn builtin() -> Self {
        Self::with_extra("")
    }

    /// Nombre de concepts.
    pub fn len(&self) -> usize {
        self.concepts.len()
    }

    /// Le texte du dictionnaire de base, tel qu'il est livré (l'écran d'administration l'affiche en lecture seule).
    pub fn builtin_text() -> &'static str {
        BUILTIN
    }

    /// Ce qui ne va pas dans un fichier de synonymes, ligne par ligne, pour que la personne qui l'écrit le corrige. Une remarque par
    /// ligne en cause ; un fichier sans remarque est sans reproche (ce qui ne dit pas qu'il est utile).
    pub fn problems(text: &str) -> Vec<String> {
        let mut out = vec![];
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let n = i + 1;
            let Some((_, strong, weak)) = parse_line(line) else {
                out.push(format!("ligne {n} : aucun mot avant le « ~ » ; la ligne est ignorée"));
                continue;
            };
            for (m, is_strong) in strong.iter().map(|m| (m, true)).chain(weak.iter().map(|m| (m, false))) {
                let stems = terms(m);
                if stems.is_empty() {
                    out.push(format!("ligne {n} : « {m} » ne contient aucun mot utile (mot vide ou trop court) ; il est ignoré"));
                } else if is_strong && stems.len() == 1 && crate::terms::is_generic(&stems[0]) {
                    // un mot lié (après le « ~ ») ne déclenche rien : il peut être courant
                    out.push(format!("ligne {n} : « {m} » est un mot très courant de la langue ; il rapprocherait des textes sans rapport"));
                }
            }
            if strong.len() == 1 && !line.contains('~') && !text.lines().take(i).any(|l| parse_line(l).is_some_and(|(name, _, _)| norm_tag(&name) == norm_tag(&strong[0]))) {
                out.push(format!("ligne {n} : « {} » est seul sur sa ligne ; ajoutez des mots de même sens (séparés par des virgules), ou le nom d'un concept existant pour compléter celui-ci", strong[0]));
            }
        }
        out
    }

    pub fn name(&self, concept: u32) -> &str {
        &self.concepts[concept as usize].name
    }

    /// Les concepts qu'évoque un texte, avec les mots qui les évoquent : (concept, rang de la première racine, nombre de racines).
    /// Une expression l'emporte sur ses mots isolés (« réseau sans fil » évoque le Wi-Fi, pas aussi le réseau).
    pub fn spans(&self, stems: &[String]) -> Vec<(u32, usize, usize)> {
        let mut out: Vec<(u32, usize, usize)> = vec![];
        let mut i = 0;
        while i < stems.len() {
            // la plus longue expression qui commence ici
            let mut longest: Vec<u32> = vec![];
            let mut len = 0;
            for (c, m) in self.multi_strong.get(&stems[i]).into_iter().flatten() {
                if stems[i..].starts_with(m) && m.len() >= len {
                    if m.len() > len {
                        longest.clear();
                        len = m.len();
                    }
                    longest.push(*c);
                }
            }
            if len > 0 {
                out.extend(longest.into_iter().map(|c| (c, i, len)));
                i += len;
                continue;
            }
            if let Some(cs) = self.single_strong.get(&stems[i]) {
                out.extend(cs.iter().map(|c| (*c, i, 1)));
            }
            i += 1;
        }
        out
    }

    /// Les concepts qu'évoque un texte (racines dans l'ordre, voir `terms`), et pour chaque racine si elle a été « consommée » par
    /// un concept : « réseau sans fil » évoque le Wi-Fi, pas aussi le réseau. Une expression l'emporte sur ses mots isolés.
    #[cfg(test)]
    pub fn triggers(&self, stems: &[String]) -> (Vec<u32>, Vec<bool>) {
        let mut consumed = vec![false; stems.len()];
        let mut out: Vec<u32> = vec![];
        for (c, start, len) in self.spans(stems) {
            consumed[start..start + len].iter_mut().for_each(|x| *x = true);
            if !out.contains(&c) {
                out.push(c);
            }
        }
        (out, consumed)
    }

    /// Ce qu'un texte de la base évoque : pour chaque concept, la force du meilleur mot de même sens et celle du meilleur mot lié (0
    /// s'il n'y en a pas). Un mot pèse selon l'endroit où il figure : `fields` = (poids, suites de racines) — tags, titre, description…
    pub fn doc_concepts(&self, fields: &[(f32, &[Vec<String>])]) -> HashMap<u32, (f32, f32)> {
        let mut out: HashMap<u32, (f32, f32)> = HashMap::new();
        for (weight, seqs) in fields {
            for seq in *seqs {
                for (i, w) in seq.iter().enumerate() {
                    for c in self.single_strong.get(w).into_iter().flatten() {
                        let e = out.entry(*c).or_default();
                        e.0 = e.0.max(*weight);
                    }
                    for c in self.single_weak.get(w).into_iter().flatten() {
                        let e = out.entry(*c).or_default();
                        e.1 = e.1.max(*weight);
                    }
                    for (c, m) in self.multi_strong.get(w).into_iter().flatten() {
                        if seq[i..].starts_with(m) {
                            let e = out.entry(*c).or_default();
                            e.0 = e.0.max(*weight);
                        }
                    }
                    for (c, m) in self.multi_weak.get(w).into_iter().flatten() {
                        if seq[i..].starts_with(m) {
                            let e = out.entry(*c).or_default();
                            e.1 = e.1.max(*weight);
                        }
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(th: &Thesaurus, text: &str) -> Vec<String> {
        th.triggers(&terms(text)).0.into_iter().map(|c| th.name(c).to_string()).collect()
    }

    #[test]
    fn le_dictionnaire_de_base_est_lisible() {
        let th = Thesaurus::builtin();
        assert!(th.len() >= 50, "{} concepts", th.len());
        // Une ligne par concept : un nom écrit deux fois fusionnerait deux lignes, une ligne sans mot utile disparaîtrait.
        let lignes = BUILTIN.lines().filter(|l| parse_line(l).is_some()).count();
        assert_eq!(lignes, th.len(), "chaque ligne du fichier est un concept distinct");
        // chaque concept sait se retrouver lui-même
        for c in &th.concepts {
            assert!(c.strong.iter().all(|m| !m.is_empty()), "{}", c.name);
            assert!(names(&th, &c.name).contains(&c.name), "« {} » ne se déclenche pas lui-même", c.name);
        }
    }

    #[test]
    fn le_dictionnaire_de_base_ne_merite_aucun_reproche() {
        assert_eq!(Thesaurus::problems(BUILTIN), Vec::<String>::new());
    }

    #[test]
    fn les_lignes_douteuses_d_un_fichier_sont_signalees() {
        assert!(Thesaurus::problems("# commentaire\n\nglpi, ticket, incident ~ demande\nmot de passe, sésame\n").is_empty());
        let p = Thesaurus::problems("~ juste des liens\nle, la\nproblème, souci, pb\nseul\n");
        let on = |ligne: usize, morceau: &str| p.iter().any(|r| r.starts_with(&format!("ligne {ligne} :")) && r.contains(morceau));
        assert!(on(1, "aucun mot avant"), "{p:?}");
        assert!(on(2, "« le »") && on(2, "« la »") && on(2, "aucun mot utile"), "{p:?}");
        assert!(on(3, "« problème » est un mot très courant") && on(3, "« souci » est un mot très courant") && on(3, "« pb » ne contient aucun mot utile"), "{p:?}");
        assert!(on(4, "seul sur sa ligne"), "{p:?}");
        // un mot seul qui complète un concept existant n'est pas un reproche
        assert!(Thesaurus::problems("imprimante, photocopieur\nimprimante, copieuse\n").is_empty());
        assert_eq!(Thesaurus::with_extra("glpi, ticket glpi\n").len(), Thesaurus::builtin().len() + 1);
    }

    #[test]
    fn des_mots_de_meme_sens_evoquent_le_meme_concept() {
        let th = Thesaurus::builtin();
        for t in ["J'ai oublié mon mot de passe", "Mon MDP ne marche plus", "my password expired", "tous mes mots de passe", "pwd ?"] {
            assert_eq!(names(&th, t), vec!["mot de passe"], "{t}");
        }
        assert_eq!(names(&th, "Il faut une mise à jour"), vec!["mise à jour"]);
        assert_eq!(names(&th, "Windows update bloqué"), vec!["mise à jour"], "l'expression consomme « windows »");
        assert_eq!(names(&th, "L'imprimante du couloir et le scanner"), vec!["imprimante", "scanner"]);
        // plusieurs concepts dans un texte, dans l'ordre, sans doublon
        assert_eq!(names(&th, "VPN coupé, puis encore le VPN"), vec!["vpn"]);
        assert_eq!(names(&th, "Le VPN ne passe pas sur le Wi-Fi"), vec!["vpn", "wifi"]);
        // les abréviations de deux lettres comptent
        assert_eq!(names(&th, "Mon PC n'a plus d'adresse IP"), vec!["ordinateur", "adresse ip"]);
    }

    #[test]
    fn une_expression_consomme_ses_mots() {
        let th = Thesaurus::builtin();
        // « réseau sans fil » = le Wi-Fi, pas aussi « réseau » ; « réseau privé virtuel » = le VPN
        assert_eq!(names(&th, "réseau sans fil"), vec!["wifi"]);
        assert_eq!(names(&th, "un réseau privé virtuel"), vec!["vpn"]);
        // mais « réseau » seul reste le réseau
        assert_eq!(names(&th, "le réseau"), vec!["réseau"]);
        let (_, consumed) = th.triggers(&terms("un réseau sans fil coupe"));
        assert_eq!(consumed, vec![true, true, false]);
        // « écran bleu » n'est pas « écran » ; « capture d'écran » non plus
        assert_eq!(names(&th, "un écran bleu"), vec!["écran bleu"]);
        assert_eq!(names(&th, "une capture d'écran"), vec!["capture d'écran"]);
        assert_eq!(names(&th, "un écran"), vec!["écran"]);
    }

    #[test]
    fn les_mots_courants_ne_declenchent_rien() {
        // Garde-fou du dictionnaire : les formules de politesse et de contexte ne doivent évoquer aucun concept.
        let th = Thesaurus::builtin();
        for t in [
            "Bonjour, merci pour votre aide", "Cordialement", "Salut, j'ai un souci", "C'est urgent, j'ai une question",
            "Pouvez-vous m'aider svp", "Depuis hier matin", "Mon collègue du deuxième étage", "J'ai besoin d'une réponse rapide",
            "Je ne comprends pas", "Ça ne fonctionne plus depuis lundi", "Quelle est la procédure ?", "Dites-moi ce qu'il faut faire",
        ] {
            assert!(names(&th, t).is_empty(), "{t} → {:?}", names(&th, t));
        }
    }

    #[test]
    fn ce_qu_evoque_un_texte_depend_de_l_endroit_ou_figure_le_mot() {
        let th = Thesaurus::builtin();
        let c = th.triggers(&terms("wifi")).0[0];
        let seqs = |l: &[&str]| l.iter().map(|t| terms(t)).collect::<Vec<_>>();
        let tags = seqs(&["réseau sans fil", "signal"]);
        let desc = seqs(&["Le poste perd la connexion au Wi-Fi du bâtiment."]);
        let ev = th.doc_concepts(&[(3.0, &tags), (1.5, &desc)]);
        // « réseau sans fil » est dans les tags (3) ; « signal » est un mot lié, lui aussi dans les tags (3)
        assert_eq!(ev[&c], (3.0, 3.0));
        // seulement dans la description : plus faible (« connexion » y est un mot lié)
        let ev = th.doc_concepts(&[(1.5, &desc)]);
        assert_eq!(ev[&c], (1.5, 1.5));
        // un mot lié seul n'évoque rien de fort
        let seul = seqs(&["Le signal est faible."]);
        let ev = th.doc_concepts(&[(1.5, &seul)]);
        assert_eq!(ev[&c], (0.0, 1.5), "mot lié seulement : pas de correspondance forte");
    }

    #[test]
    fn le_fichier_de_l_utilisateur_complete_le_dictionnaire() {
        let th = Thesaurus::with_extra("# mon dictionnaire\nmot de passe, sésame, open sesame\nvpn, tunnel chiffré\nascenseur, lift ~ étage\n\n   \n~ rien\n");
        // il s'ajoute aux concepts existants…
        assert_eq!(names(&th, "mon sésame"), vec!["mot de passe"]);
        assert_eq!(names(&th, "un open sesame"), vec!["mot de passe"]);
        assert_eq!(names(&th, "le tunnel chiffré"), vec!["vpn"]);
        assert_eq!(names(&th, "le mdp"), vec!["mot de passe"], "le dictionnaire de base reste actif");
        // … ou crée le sien
        assert_eq!(names(&th, "un lift"), vec!["ascenseur"]);
        assert_eq!(Thesaurus::builtin().len() + 1, th.len());
        // une ligne sans mot fort est ignorée
        assert!(names(&th, "rien").is_empty());
    }

    #[test]
    fn le_vocabulaire_de_tous_les_jours_ne_declenche_rien() {
        // Garde-fou du dictionnaire : aucun de ces mots courants (noms, verbes, adjectifs, formules…) ne doit évoquer un concept.
        // Ajouter au dictionnaire un mot qui figure ici rapprocherait des textes à tout propos.
        let th = Thesaurus::builtin();
        let courants = "jour nuit temps vie monde homme femme enfant maison pays travail main tête oeil yeux bras coeur corps voix mot nom fois chose part côté façon moment soir matin midi année mois semaine heure minute seconde demain hier aujourd'hui maintenant toujours jamais souvent parfois vraiment très trop assez plus moins bien mal mieux pire grand petit beau joli bon mauvais nouveau vieux jeune long court haut bas gros fort doux chaud froid rouge bleu vert noir blanc jaune rose gris argent or eau feu terre air vent ciel soleil étoile arbre chat chien oiseau train avion route porte fenêtre mur sol toit table chaise livre papier stylo film musique chanson jeu match repas pain thé bouteille tasse assiette fruit légume viande poisson gâteau sucre sel prix patron collègue ami famille mari frère soeur père mère fils fille voisin voisine docteur médecin école classe cours étude examen question réponse idée problème solution histoire souvenir rêve peur joie colère tristesse bonheur plaisir envie besoin amour confiance respect secret mensonge vérité promesse cadeau surprise sourire rire regard silence bruit odeur parfum goût chaleur lumière ombre couleur forme taille poids force douceur beauté charme peau cheveux visage bouche dents langue cou dos genou doigt ongle sang souffle respiration battement parler dire écouter entendre voir regarder toucher sentir goûter manger boire dormir rêver réveiller lever coucher asseoir marcher courir sauter chanter jouer travailler étudier lire écrire dessiner conduire voyager partir venir arriver rester revenir attendre chercher trouver perdre donner prendre mettre ouvrir fermer pousser tirer porter tenir garder laisser quitter oublier penser croire savoir comprendre aimer détester vouloir pouvoir devoir falloir faire être avoir aller bonjour bonsoir salut merci pardon désolé urgent aide svp bureau service réunion étage couloir lundi mardi vendredi";
        for w in courants.split_whitespace() {
            assert!(names(&th, w).is_empty(), "« {w} » déclenche {:?}", names(&th, w));
        }
        // et ceux-ci, qui disent vraiment quelque chose du domaine, déclenchent bien
        for w in ["vpn", "imprimante", "wifi", "mdp", "outlook", "teams", "windows", "sauvegarde", "scanner", "phishing"] {
            assert!(!names(&th, w).is_empty(), "{w}");
        }
    }
}
