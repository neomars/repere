//! Mots-clés d'un texte, pour rapprocher une conversation des textes de la base : accents et ponctuation repliés, mots vides
//! retirés, racines légères (« imprimantes » et « imprimante », « redémarre » et « redémarrer » se rejoignent). Volontairement
//! simple et sans dictionnaire : un faux rapprochement ne fait que proposer une source de plus à l'IA, qui choisit ensuite.

use std::collections::HashSet;
use std::sync::OnceLock;

/// Minuscules, sans accents, tout ce qui n'est pas lettre ou chiffre devient une espace (« Wi-Fi, é » → « wi fi  e »).
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        for l in c.to_lowercase() {
            let l = match l {
                'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => 'a',
                'ç' => 'c',
                'é' | 'è' | 'ê' | 'ë' => 'e',
                'î' | 'ï' | 'í' | 'ì' => 'i',
                'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
                'ù' | 'û' | 'ü' | 'ú' => 'u',
                'ÿ' | 'ý' => 'y',
                'ñ' => 'n',
                '’' | '\'' => ' ',
                c if c.is_alphanumeric() => c,
                _ => ' ',
            };
            out.push(l);
        }
    }
    out
}

/// Mots sans valeur pour départager deux textes (déjà repliés : minuscules, sans accents).
const STOP: &[&str] = &[
    // français
    "a", "ai", "aie", "aient", "ait", "as", "au", "aux", "avec", "avait", "avais", "avoir", "avons", "avez", "ont", "ayant", "ce", "ceci", "cela",
    "celle", "celles", "celui", "ces", "cet", "cette", "ceux", "chaque", "chez", "comme", "comment", "contre", "dans", "de", "des", "donc", "dont",
    "du", "elle", "elles", "en", "encore", "entre", "es", "est", "etais", "etait", "etant", "ete", "etre", "eux", "faire", "fait", "fais", "faut",
    "il", "ils", "je", "les", "leur", "leurs", "lui", "ma", "mais", "me", "meme", "mes", "moi", "mon", "ne", "ni", "nos", "notre", "nous", "on", "ou",
    "par", "pas", "peu", "peut", "plus", "pour", "pourquoi", "quand", "que", "quel", "quelle", "quels", "quelles", "qui", "quoi", "sa", "sans",
    "se", "ses", "si", "son", "sont", "sous", "sur", "ta", "te", "tes", "toi", "ton", "tous", "tout", "toute", "toutes", "tres", "trop", "tu",
    "un", "une", "vers", "vos", "votre", "vous", "ca", "qu", "suis", "sommes", "etes", "serait", "sera", "vais", "va", "vas", "vont", "allez",
    "allons", "veux", "veut", "voudrais", "peux", "viens", "dis", "dit", "oui", "non", "ok", "bon", "bien", "alors", "puis", "aussi", "deja",
    "jamais", "toujours", "rien", "beaucoup", "moins", "autre", "autres", "aucun", "quelque", "quelques", "ici", "voila", "voici", "avant",
    "apres", "pendant", "depuis", "jusque", "jusqu", "vraiment", "juste", "aime", "pense", "sais", "sait", "veux", "dois", "doit",
    // anglais
    "the", "an", "and", "or", "of", "to", "in", "on", "at", "by", "for", "with", "from", "is", "are", "was", "were", "be", "been", "it", "its",
    "this", "that", "these", "those", "he", "she", "they", "his", "her", "their", "him", "them", "as", "into", "over", "under", "up", "down",
    "out", "off", "then", "than", "so", "not", "no", "yes", "you", "your", "my", "we", "our", "us", "what", "who", "how", "why", "when", "where",
    "do", "does", "did", "have", "has", "had", "will", "would", "can", "could", "just", "there", "here",
];

/// Sigles informatiques de deux lettres : le filtre « trois lettres au moins » les supprimerait (« mon PC », « adresse IP »).
/// « wi » et « fi » restent pour que « Wi-Fi » (replié en deux mots) ne disparaisse pas.
const SHORT_TECH: &[&str] = &["pc", "ip", "os", "ad", "wi", "fi"];

fn stop_words() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| STOP.iter().copied().collect())
}

/// Racine d'un mot déjà replié : pluriels, féminin, terminaisons verbales courantes.
fn stem(w: &str) -> String {
    let mut s = w;
    // pluriel (« imprimantes », « réseaux ») ; pas « access », « bus », « virus »
    if s.len() >= 4 && (s.ends_with('s') || s.ends_with('x')) && !s.ends_with("ss") && !s.ends_with("us") {
        s = &s[..s.len() - 1];
    }
    // « e » muet et féminin (« imprimante », « verrouillée » → « verrouillee » → « verrouill »)
    for _ in 0..2 {
        if s.len() >= 5 && s.ends_with('e') {
            s = &s[..s.len() - 1];
        }
    }
    // verbes : « redémarrer », « redémarrent », « redémarrant » → « redemarr »
    for (suffix, min) in [("ent", 5), ("ant", 5), ("er", 4), ("ez", 4), ("ons", 4)] {
        if let Some(root) = s.strip_suffix(suffix) {
            if root.len() >= min {
                s = root;
                break;
            }
        }
    }
    s.to_string()
}

/// Les mots utiles d'un texte, dans l'ordre (avec répétitions) : repliés, sans mots vides, sans nombres, ramenés à leur racine.
pub fn terms(text: &str) -> Vec<String> {
    let stop = stop_words();
    fold(text)
        .split_whitespace()
        .filter(|w| !stop.contains(w) && !w.chars().all(|c| c.is_ascii_digit()))
        .map(stem)
        .filter(|t| t.chars().count() >= 3 || SHORT_TECH.contains(&t.as_str()))
        .collect()
}

/// Un tag comparable : « Wi-Fi  Invité » et « wi fi invite » sont le même.
pub fn norm_tag(t: &str) -> String {
    fold(t).split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pliage_des_accents_et_de_la_ponctuation() {
        assert_eq!(fold("Été à l'ÉCOLE, ça!"), "ete a l ecole  ca ");
        assert_eq!(fold("Wi-Fi, é"), "wi fi  e");
        assert_eq!(fold("Reçu : ñandú"), "recu   nandu");
    }

    #[test]
    fn racines_des_pluriels_du_feminin_et_des_verbes() {
        for (a, b) in [
            ("imprimante", "imprimantes"), ("clavier", "claviers"), ("réseau", "réseaux"), ("fichier", "fichiers"),
            ("verrouillé", "verrouillées"), ("redémarre", "redémarrer"), ("redémarre", "redémarrent"), ("redémarre", "redémarrant"),
            ("installe", "installer"), ("connexion", "connexions"), ("écran", "écrans"),
        ] {
            assert_eq!(terms(a), terms(b), "{a} / {b}");
        }
        // des mots différents restent différents
        assert_ne!(terms("disque"), terms("disquette"));
        assert_ne!(terms("lien"), terms("lieu"));
        assert_eq!(terms("access"), vec!["access"], "pas de « s » final retiré sur « ss »");
    }

    #[test]
    fn mots_vides_nombres_et_accents() {
        assert_eq!(
            terms("Il faut redémarrer le poste, puis vérifier la connexion."),
            vec!["redemarr", "post", "verifi", "connexion"]
        );
        assert_eq!(terms("Erreur 0x80070005 à l'ouverture de la session"), vec!["erreur", "0x80070005", "ouvertur", "session"]);
        assert_eq!(terms("Version 2041 — ÉCRANS partagés"), vec!["version", "ecran", "partag"]);
        assert!(terms("Je ne sais pas, et vous ?").is_empty(), "que des mots vides");
        assert_eq!(norm_tag("  Wi-Fi  Invité "), "wi fi invite");
        assert_eq!(norm_tag("Accès"), "acces");
    }

    #[test]
    fn sigles_informatiques_de_deux_lettres_conserves() {
        assert_eq!(terms("Mon PC n'a plus d'adresse IP"), vec!["pc", "adress", "ip"]);
        assert_eq!(terms("Le Wi-Fi coupe"), vec!["wi", "fi", "coup"]);
        // les autres mots de deux lettres restent écartés
        assert!(terms("xy zz qw").is_empty());
    }

    #[test]
    fn la_liste_de_mots_vides_est_repliee() {
        // Chaque mot de la liste doit être sous la forme produite par `fold` (sinon il ne serait jamais reconnu).
        for w in STOP {
            assert_eq!(fold(w).trim(), *w, "« {w} » n'est pas replié");
        }
    }
}
