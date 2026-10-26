//! Mots de passe et clés oubliés dans un document. Un document de la base peut contenir « mot de passe : Printemps2024! » ; l'assistant
//! ne doit jamais le répéter. À l'import, on signale les passages concernés à la personne qui importe ; quand un passage est proposé
//! à l'IA, les valeurs repérées sont remplacées par « [masqué] ».
//!
//! La détection est volontairement prudente : elle cherche une valeur qui ressemble à un secret après un libellé qui en annonce un
//! (« mot de passe : », « token = »), des formats de clés connus, des clés privées et des adresses avec identifiants. Elle manquera un
//! mot de passe écrit au milieu d'une phrase : c'est un filet de sécurité, pas une garantie.

use regex::Regex;
use std::borrow::Cow;
use std::sync::OnceLock;

/// Ce qui remplace une valeur secrète.
pub const MASK: &str = "[masqué]";

/// Début de l'avertissement d'import qui signale des mots de passe dans un document : `Document::secrets` s'y reconnaît.
pub const WARNING: &str = "ce document semble contenir";

struct Rules {
    /// Libellé annonçant un secret, séparateur, valeur (groupe 2). La valeur est examinée ensuite (`looks_secret`).
    labeled: Regex,
    /// Formats de clés ou de jetons reconnaissables à eux seuls.
    tokens: Vec<(&'static str, Regex)>,
    private_key: Regex,
    /// `https://utilisateur:mot-de-passe@hôte`
    credentials_in_url: Regex,
}

fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| Rules {
        labeled: Regex::new(
            r#"(?i)\b(mot\s+de\s+passe|mots\s+de\s+passe|mdp|password|passwd|pwd|passphrase|code\s+secret|secret|jeton|token|api[\s_-]?key|cl[ée]\s+(?:d['’]api|secr[èe]te|priv[ée]e))\b[^\n:=]{0,25}?\s*[:=]\s*["'«`]?\s*([^\s"'»`<>\[\]{}()]{4,})"#,
        )
        .expect("expression constante"),
        tokens: vec![
            ("clé d'accès AWS", Regex::new(r"\bAKIA[0-9A-Z]{16}\b").expect("expression constante")),
            ("jeton GitHub", Regex::new(r"\bgh[pousr]_[A-Za-z0-9]{30,}\b").expect("expression constante")),
            ("jeton Slack", Regex::new(r"\bxox[abprs]-[A-Za-z0-9-]{10,}\b").expect("expression constante")),
            ("clé d'API", Regex::new(r"\bsk-[A-Za-z0-9]{20,}\b").expect("expression constante")),
            ("jeton JWT", Regex::new(r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b").expect("expression constante")),
        ],
        private_key: Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").expect("expression constante"),
        credentials_in_url: Regex::new(r"(?i)\b[a-z][a-z0-9+.-]*://[^/\s:@]+:([^/\s:@]+)@").expect("expression constante"),
    })
}

/// La valeur ressemble-t-elle à un secret, ou à un mot ordinaire, un exemple, un espace réservé ?
/// « doit » et « ******** » n'en sont pas ; « Printemps2024! » et « s3cr3t » en sont.
fn looks_secret(value: &str) -> bool {
    let v = value.trim_matches(|c: char| matches!(c, '.' | ',' | ';' | '"' | '\''));
    if v.chars().count() < 4 {
        return false;
    }
    let lower = v.to_lowercase();
    let placeholder = v.chars().all(|c| matches!(c, '*' | 'x' | 'X' | '•' | '·' | '.' | '_' | '-' | '#' | '?'))
        || ["votre", "exemple", "example", "secret", "motdepasse", "password", "xxxx", "à définir", "a-definir", "changeme", "monmotdepasse"].iter().any(|p| lower.starts_with(p));
    if placeholder {
        return false;
    }
    let digit = v.chars().any(|c| c.is_ascii_digit());
    let symbol = v.chars().any(|c| !c.is_alphanumeric());
    let mixed_case = v.chars().any(char::is_uppercase) && v.chars().any(char::is_lowercase) && v.chars().skip(1).any(char::is_uppercase);
    digit || symbol || mixed_case
}

/// Les sortes de secrets que contient un texte (sans jamais citer leur valeur).
pub fn kinds(text: &str) -> Vec<&'static str> {
    let r = rules();
    let mut out: Vec<&'static str> = vec![];
    let mut add = |k: &'static str| {
        if !out.contains(&k) {
            out.push(k);
        }
    };
    if r.labeled.captures_iter(text).any(|c| c.get(2).is_some_and(|m| looks_secret(m.as_str()))) {
        add("mot de passe");
    }
    for (name, re) in &r.tokens {
        if re.is_match(text) {
            add(name);
        }
    }
    if r.private_key.is_match(text) {
        add("clé privée");
    }
    if r.credentials_in_url.captures_iter(text).any(|c| c.get(1).is_some_and(|m| looks_secret(m.as_str()) || m.as_str().chars().count() >= 6)) {
        add("adresse avec identifiants");
    }
    out
}

/// Le texte sans ses valeurs secrètes : « mot de passe : Printemps2024! » devient « mot de passe : [masqué] ».
pub fn mask(text: &str) -> Cow<'_, str> {
    if kinds(text).is_empty() {
        return Cow::Borrowed(text);
    }
    let r = rules();
    let mut out = r
        .labeled
        .replace_all(text, |c: &regex::Captures| {
            let whole = c.get(0).map_or("", |m| m.as_str());
            match c.get(2) {
                Some(v) if looks_secret(v.as_str()) => whole.replacen(v.as_str(), MASK, 1),
                _ => whole.to_string(),
            }
        })
        .into_owned();
    for (_, re) in &r.tokens {
        out = re.replace_all(&out, MASK).into_owned();
    }
    out = r.credentials_in_url.replace_all(&out, |c: &regex::Captures| c[0].replacen(&c[1], MASK, 1)).into_owned();
    // une clé privée tient sur plusieurs lignes : tout ce qui suit l'en-tête est retiré
    if let Some(m) = r.private_key.find(&out) {
        out.truncate(m.start());
        out.push_str(MASK);
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_mot_de_passe_ecrit_apres_son_libelle_est_repere_et_masque() {
        for (texte, masque) in [
            ("Le mot de passe : Printemps2024! est valable un mois.", "Le mot de passe : [masqué] est valable un mois."),
            ("MDP = Azerty123", "MDP = [masqué]"),
            ("password: s3cr3t-value", "password: [masqué]"),
            ("Mot de passe initial : « Bienvenue-2024 »", "Mot de passe initial : « [masqué] »"),
            ("wifi invité, pwd=Ab3dEf", "wifi invité, pwd=[masqué]"),
        ] {
            assert_eq!(kinds(texte), vec!["mot de passe"], "{texte}");
            assert_eq!(mask(texte), masque);
        }
    }

    #[test]
    fn des_phrases_ordinaires_ne_sont_pas_des_secrets() {
        for texte in [
            "Le mot de passe : doit comporter 12 caractères au minimum.",
            "Changez votre mot de passe : cliquez sur Modifier.",
            "Mot de passe : ******** (masqué par le système)",
            "mot de passe : xxxxxxxx",
            "Votre mot de passe : <à choisir>",
            "Le mot de passe expire tous les 90 jours.",
            "Tapez le mot de passe puis validez",
            "password: votre-mot-de-passe",
            "token : exemple",
        ] {
            assert!(kinds(texte).is_empty(), "{texte} → {:?}", kinds(texte));
            assert_eq!(mask(texte), texte);
        }
    }

    #[test]
    fn cles_jetons_et_adresses_avec_identifiants() {
        let aws = "clé AKIAIOSFODNN7EXAMPLE dans le fichier";
        assert_eq!(kinds(aws), vec!["clé d'accès AWS"]);
        assert_eq!(mask(aws), "clé [masqué] dans le fichier");
        let gh = "jeton ghp_abcdefghijklmnopqrstuvwxyz0123456789 (ancien)";
        assert!(kinds(gh).contains(&"jeton GitHub"));
        assert!(!mask(gh).contains("ghp_"));
        let url = "Se connecter à ftp://admin:Hunter2xx@serveur.exemple.fr/dossier";
        assert_eq!(kinds(url), vec!["adresse avec identifiants"]);
        assert_eq!(mask(url), "Se connecter à ftp://admin:[masqué]@serveur.exemple.fr/dossier");
        assert!(kinds("https://intranet.exemple.fr/aide:80/page").is_empty());
        let key = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA…\n-----END RSA PRIVATE KEY-----";
        assert_eq!(kinds(key), vec!["clé privée"]);
        assert_eq!(mask(key), MASK);
        let jwt = "Authorization: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        assert!(kinds(jwt).contains(&"jeton JWT") && !mask(jwt).contains("eyJ"));
    }

    #[test]
    fn un_texte_sans_secret_est_rendu_tel_quel() {
        let t = "Redémarrez le poste puis ouvrez https://mdp.exemple.fr pour réinitialiser votre accès.";
        assert!(matches!(mask(t), Cow::Borrowed(_)));
        assert!(kinds("").is_empty());
    }
}
