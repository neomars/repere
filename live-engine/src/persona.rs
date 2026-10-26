//! Profils d'assistant (« personas » dans le code) : fiches éditables stockées en JSON dans `data/personas/`. Un profil
//! règle le ton de l'assistant, son message d'accueil, sa voix et ses réglages d'écriture ; les règles de fond (répondre
//! d'après les sources, ne jamais demander de mot de passe…) viennent du moteur et ne se modifient pas ici.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Champ libre défini par l'utilisateur (ex. « Service » → « Informatique », « Horaires » → « 8 h – 18 h »).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CustomField {
    pub label: String,
    pub value: String,
}

const MAX_CUSTOM_FIELDS: usize = 30;

/// Identifiant du profil créé au premier lancement.
const DEFAULT_ID: &str = "repere";
/// Fichier témoin posé à côté des profils une fois le profil par défaut créé : supprimé volontairement, il ne revient pas.
const DEFAULT_MARKER: &str = ".profil-par-defaut";

/// Nom valide pour un fichier de voix ou une clé de conversation (lettres, chiffres, - et _).
pub fn valid_asset_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Un f32 sérialisé tel quel donne 0.8500000238418579 dans le JSON (conversion en f64) : on arrondit au millième.
fn ser_opt_f32<S: serde::Serializer>(v: &Option<f32>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(x) => s.serialize_some(&((*x as f64 * 1000.0).round() / 1000.0)),
        None => s.serialize_none(),
    }
}

/// Réglages d'écriture du modèle (échantillonnage), propres à chaque profil. Tout est facultatif : ce qui manque prend la
/// valeur par défaut de l'app. Les noms sont ceux de llama-server (`top_p`, `min_p`, `repeat_penalty`, `presence_penalty`,
/// `mirostat*`, `dry_*`, `xtc_*`, `max_tokens`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Sampling {
    #[serde(serialize_with = "ser_opt_f32")]
    pub top_p: Option<f32>,
    #[serde(serialize_with = "ser_opt_f32")]
    pub min_p: Option<f32>,
    #[serde(serialize_with = "ser_opt_f32")]
    pub repeat_penalty: Option<f32>,
    #[serde(serialize_with = "ser_opt_f32")]
    pub presence_penalty: Option<f32>,
    /// Longueur maximale d'une réponse, en jetons (coupée net au-delà).
    pub max_tokens: Option<u32>,
    /// Mirostat 2 : règle lui-même la diversité (top_p et min_p sont alors ignorés par llama.cpp, on ne les envoie pas).
    pub mirostat: bool,
    #[serde(serialize_with = "ser_opt_f32")]
    pub mirostat_tau: Option<f32>,
    #[serde(serialize_with = "ser_opt_f32")]
    pub mirostat_eta: Option<f32>,
    /// DRY (pénalise les séquences répétées) et XTC (écarte parfois le choix le plus probable), en version légère.
    pub dry: bool,
    pub xtc: bool,
}

impl Sampling {
    /// Bornes sûres : une fiche éditée à la main ne peut pas envoyer de valeurs absurdes au serveur.
    pub fn normalized(&self) -> Sampling {
        let c = |v: Option<f32>, lo: f32, hi: f32| v.filter(|x| x.is_finite()).map(|x| x.clamp(lo, hi));
        Sampling {
            top_p: c(self.top_p, 0.5, 1.0),
            min_p: c(self.min_p, 0.0, 0.5),
            repeat_penalty: c(self.repeat_penalty, 1.0, 1.5),
            presence_penalty: c(self.presence_penalty, 0.0, 1.0),
            max_tokens: self.max_tokens.map(|m| m.clamp(64, 2048)),
            mirostat: self.mirostat,
            mirostat_tau: c(self.mirostat_tau, 1.0, 10.0),
            mirostat_eta: c(self.mirostat_eta, 0.01, 1.0),
            dry: self.dry,
            xtc: self.xtc,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Persona {
    pub id: String,
    /// Nom de l'assistant, tel que l'utilisateur le voit.
    pub name: String,
    /// Contexte : organisation, périmètre, outils en place (texte libre).
    pub background: String,
    /// Posture générale de l'assistant (le champ s'appelle `personality` pour garder les identifiants de code).
    pub personality: String,
    /// Champs libres ajoutés par l'utilisateur.
    pub custom_fields: Vec<CustomField>,
    pub summary: String,
    /// Style de réponse : longueur, ton, mise en forme.
    pub speaking_style: String,
    /// Consignes supplémentaires, propres à ce profil.
    pub scenario: String,
    /// Limites supplémentaires (en plus des règles du moteur) : sujets que ce profil ne traite pas.
    pub boundaries: String,
    /// Message d'accueil affiché à l'ouverture d'une conversation.
    pub first_message: String,
    pub language: String,
    /// Nom d'une voix de référence (fichier dans le dossier des voix) ou voix du serveur TTS.
    pub voice: Option<String>,
    /// Expressivité de la voix (Chatterbox, 0.25-2.0 ; 0.5 = neutre).
    #[serde(serialize_with = "ser_opt_f32")]
    pub tts_exaggeration: Option<f32>,
    /// Rythme/guidage (Chatterbox, 0-1 ; plus bas = débit plus lent et posé).
    #[serde(serialize_with = "ser_opt_f32")]
    pub tts_cfg_weight: Option<f32>,
    #[serde(serialize_with = "ser_opt_f32")]
    pub temperature: Option<f32>,
    pub sampling: Sampling,
}

impl Default for Persona {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            background: String::new(),
            personality: String::new(),
            custom_fields: vec![],
            summary: String::new(),
            speaking_style: String::new(),
            scenario: String::new(),
            boundaries: String::new(),
            first_message: String::new(),
            language: "fr".into(),
            voice: None,
            tts_exaggeration: None,
            tts_cfg_weight: None,
            temperature: None,
            sampling: Sampling::default(),
        }
    }
}

impl Persona {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty()
            || !self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("id invalide (lettres, chiffres, - et _ uniquement)".into());
        }
        if self.name.trim().is_empty() {
            return Err("le nom est obligatoire".into());
        }
        if self.custom_fields.len() > MAX_CUSTOM_FIELDS {
            return Err(format!("trop de champs libres (maximum {MAX_CUSTOM_FIELDS})"));
        }
        Ok(())
    }

    /// Nettoie la fiche avant l'enregistrement : espaces superflus, champs libres vides ou sans titre.
    pub fn normalized(&self) -> Persona {
        let mut p = self.clone();
        p.name = p.name.trim().to_string();
        p.custom_fields = p
            .custom_fields
            .iter()
            .map(|f| CustomField { label: f.label.trim().chars().take(80).collect(), value: f.value.trim().to_string() })
            .filter(|f| !f.label.is_empty() && !f.value.is_empty())
            .collect();
        p.temperature = p.temperature.filter(|t| t.is_finite()).map(|t| t.clamp(0.1, 1.5));
        p.sampling = p.sampling.normalized();
        p
    }
}

/// Le profil créé au premier lancement : l'assistant de support de premier niveau, prêt à l'emploi.
fn default_profile() -> Persona {
    Persona {
        id: DEFAULT_ID.into(),
        name: "Repère".into(),
        summary: "Assistant de support informatique de premier niveau : répond d'après la base de connaissances de la DSI, ou propose d'ouvrir un ticket.".into(),
        personality: "Courtois, calme et précis. Va droit au but, sans jargon inutile.".into(),
        speaking_style: "Vouvoiement. Réponses courtes ; des étapes numérotées quand il y a une procédure à suivre.".into(),
        first_message: "Bonjour, je suis Repère, l'assistant de la DSI. Décrivez-moi votre problème : je cherche la réponse dans la base de connaissances et, si je ne la trouve pas, je vous propose d'ouvrir un ticket.".into(),
        temperature: Some(0.3),
        ..Default::default()
    }
}

pub struct PersonaStore {
    dir: PathBuf,
}

impl PersonaStore {
    pub fn new(dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self { dir: dir.to_path_buf() })
    }

    /// Au premier lancement, crée le profil « Repère » : sans lui l'écran de départ est vide. Il n'est pas recréé quand
    /// d'autres profils existent déjà, ni après avoir été supprimé (un fichier témoin le rappelle). `true` s'il vient d'être créé.
    pub fn ensure_default(&self) -> bool {
        let marker = self.dir.join(DEFAULT_MARKER);
        if marker.exists() || !self.list().is_empty() {
            return false;
        }
        self.save(&default_profile()).is_ok() && std::fs::write(marker, "").is_ok()
    }

    pub fn list(&self) -> Vec<Persona> {
        let mut v: Vec<Persona> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|s| serde_json::from_str::<Persona>(&s).ok())
            .filter(|p| p.validate().is_ok())
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn get(&self, id: &str) -> Option<Persona> {
        // L'id est validé à l'écriture ; on le revalide ici pour éviter tout chemin forgé.
        if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return None;
        }
        let s = std::fs::read_to_string(self.dir.join(format!("{id}.json"))).ok()?;
        serde_json::from_str::<Persona>(&s).ok().filter(|p| p.validate().is_ok())
    }

    pub fn save(&self, p: &Persona) -> Result<(), String> {
        let p = p.normalized();
        p.validate()?;
        let s = serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?;
        std::fs::write(self.dir.join(format!("{}.json", p.id)), s).map_err(|e| e.to_string())
    }

    pub fn delete(&self, id: &str) -> bool {
        id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            && std::fs::remove_file(self.dir.join(format!("{id}.json"))).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("repere-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn le_profil_par_defaut_est_valide_et_cree_une_seule_fois() {
        assert!(default_profile().validate().is_ok());
        let dir = temp_dir("profils");
        let store = PersonaStore::new(&dir).unwrap();
        assert!(store.ensure_default());
        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].id.as_str(), list[0].name.as_str()), (DEFAULT_ID, "Repère"));
        assert!(list[0].first_message.contains("Repère") && list[0].temperature == Some(0.3));
        assert!(!store.ensure_default(), "déjà créé");
        // supprimé volontairement : il ne revient pas
        assert!(store.delete(DEFAULT_ID));
        assert!(!store.ensure_default());
        assert!(store.list().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn pas_de_profil_par_defaut_quand_il_en_existe_deja() {
        let dir = temp_dir("profils-existants");
        let store = PersonaStore::new(&dir).unwrap();
        store.save(&Persona { id: "service-rh".into(), name: "Assistant RH".into(), ..Default::default() }).unwrap();
        assert!(!store.ensure_default());
        let ids: Vec<_> = store.list().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["service-rh"]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reglages_d_ecriture_bornes_et_anciennes_fiches_inchangees() {
        let old: Persona = serde_json::from_str(r#"{"id":"v","name":"V","temperature":0.9}"#).unwrap();
        assert_eq!(old.sampling, Sampling::default());
        let p = Persona {
            id: "x".into(), name: "X".into(), temperature: Some(9.0),
            sampling: Sampling {
                top_p: Some(0.1), min_p: Some(f32::NAN), repeat_penalty: Some(5.0), presence_penalty: Some(-3.0),
                max_tokens: Some(5), mirostat_tau: Some(99.0), mirostat_eta: Some(0.0), ..Default::default()
            },
            ..Default::default()
        }
        .normalized();
        assert_eq!(p.temperature, Some(1.5));
        let s = p.sampling;
        assert_eq!((s.top_p, s.min_p, s.repeat_penalty, s.presence_penalty), (Some(0.5), None, Some(1.5), Some(0.0)));
        assert_eq!((s.max_tokens, s.mirostat_tau, s.mirostat_eta), (Some(64), Some(10.0), Some(0.01)));
    }

    #[test]
    fn nombres_decimaux_propres_dans_le_json() {
        let p = Persona {
            id: "x".into(), name: "X".into(), temperature: Some(0.85), tts_exaggeration: Some(0.7),
            sampling: Sampling { min_p: Some(0.04), repeat_penalty: Some(1.1), ..Default::default() }, ..Default::default()
        };
        let j = serde_json::to_string(&p).unwrap();
        assert!(j.contains("\"temperature\":0.85") && j.contains("\"tts_exaggeration\":0.7"), "{j}");
        assert!(j.contains("\"min_p\":0.04") && j.contains("\"repeat_penalty\":1.1"), "{j}");
        let back: Persona = serde_json::from_str(&j).unwrap();
        assert_eq!(back.sampling.min_p, Some(0.04));
        assert!(serde_json::to_string(&json!({"p": &p})).is_ok());
    }

    #[test]
    fn fiches_incompletes_ou_avec_des_champs_inconnus() {
        let p: Persona = serde_json::from_str(r#"{"id":"vieux","name":"Vieux","personality":"calme"}"#).unwrap();
        assert!(p.validate().is_ok());
        assert!(p.custom_fields.is_empty());
        assert_eq!((p.personality.as_str(), p.language.as_str()), ("calme", "fr"));
        // un champ qui n'existe plus est ignoré, la fiche reste lisible
        let p: Persona = serde_json::from_str(r#"{"id":"ancien","name":"Ancien","age":40,"face":"x"}"#).unwrap();
        assert!(p.validate().is_ok());
    }

    #[test]
    fn enregistre_les_champs_de_la_fiche_et_nettoie_les_champs_libres() {
        let dir = temp_dir("profils-champs");
        let store = PersonaStore::new(&dir).unwrap();
        let p = Persona {
            id: "service".into(), name: "  Assistant  ".into(), background: "DSI d'une collectivité".into(),
            custom_fields: vec![
                CustomField { label: " Service ".into(), value: " Informatique ".into() },
                CustomField { label: "".into(), value: "sans titre".into() },
                CustomField { label: "Vide".into(), value: "  ".into() },
            ],
            ..Default::default()
        };
        store.save(&p).unwrap();
        let back = store.get("service").unwrap();
        assert_eq!(back.name, "Assistant");
        assert_eq!(back.custom_fields, vec![CustomField { label: "Service".into(), value: "Informatique".into() }]);
        assert_eq!(back.background, "DSI d'une collectivité");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn refuse_trop_de_champs_libres() {
        let f = CustomField { label: "a".into(), value: "b".into() };
        let p = Persona { id: "x".into(), name: "X".into(), custom_fields: vec![f; 31], ..Default::default() };
        assert!(p.validate().is_err());
    }

    #[test]
    fn refuse_un_id_avec_chemin_et_un_nom_vide() {
        let p = Persona { id: "../etc".into(), name: "X".into(), ..Default::default() };
        assert!(p.validate().is_err());
        let p = Persona { id: "x".into(), name: "  ".into(), ..Default::default() };
        assert!(p.validate().is_err());
        assert!(valid_asset_name("voix-1_a") && !valid_asset_name("../x") && !valid_asset_name(""));
    }
}
