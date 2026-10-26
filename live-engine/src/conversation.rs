//! Persistance des conversations : un fichier JSON par profil d'assistant, repris automatiquement ; « Nouvelle conversation »
//! le supprime. (Une conversation par utilisateur viendra avec la connexion des personnes : la clé portera alors son identifiant.)

use crate::llm::ChatMsg;
use crate::persona::valid_asset_name;
use crate::prompt::OPENING_PREFIX;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Les extraits qu'un message de l'assistant a cités.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Citation {
    /// Rang du message dans l'historique.
    pub at: usize,
    /// Numéros des extraits cités.
    pub ids: Vec<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub persona: String,
    pub history: Vec<ChatMsg>,
    pub summary: String,
    pub shown: Vec<i64>,
    pub user_name: String,
    /// Secondes depuis l'epoch Unix.
    pub updated: u64,
    /// Extraits cités par l'assistant, message par message (affichés à nouveau quand la conversation est reprise).
    pub citations: Vec<Citation>,
}

/// Clé de sauvegarde : le profil d'assistant.
pub fn key_for(persona: &str) -> Option<String> {
    valid_asset_name(persona).then(|| persona.to_string())
}

/// Ce message s'affiche-t-il ? La consigne interne d'ouverture (« (Début de la conversation… ») non.
pub fn is_visible(m: &ChatMsg) -> bool {
    (m.role == "user" || m.role == "assistant") && !(m.role == "user" && m.content.starts_with(OPENING_PREFIX))
}

/// Messages à afficher.
pub fn visible_messages(history: &[ChatMsg]) -> Vec<(String, String)> {
    history.iter().filter(|m| is_visible(m)).map(|m| (m.role.clone(), m.content.clone())).collect()
}

pub struct ConversationStore {
    dir: PathBuf,
}

impl ConversationStore {
    pub fn new(dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self { dir: dir.to_path_buf() })
    }

    fn path(&self, key: &str) -> Option<PathBuf> {
        valid_asset_name(key).then(|| self.dir.join(format!("{key}.json")))
    }

    pub fn load(&self, key: &str) -> Option<Saved> {
        serde_json::from_str(&std::fs::read_to_string(self.path(key)?).ok()?).ok()
    }

    /// Écriture atomique (fichier temporaire puis renommage) : une coupure ne laisse jamais un fichier tronqué.
    pub fn save(&self, key: &str, saved: &Saved) -> std::io::Result<()> {
        let path = self.path(key).ok_or_else(|| std::io::Error::other("clé invalide"))?;
        std::fs::create_dir_all(&self.dir)?; // le dossier a pu être supprimé depuis le démarrage
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(saved)?)?;
        std::fs::rename(&tmp, &path)
    }

    pub fn delete(&self, key: &str) -> bool {
        self.path(key).is_some_and(|p| std::fs::remove_file(p).is_ok())
    }

    /// (clé, profil, nombre de messages visibles, date) de chaque conversation sauvegardée.
    pub fn list(&self) -> Vec<(String, String, usize, u64)> {
        let mut v: Vec<_> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let key = p.file_stem()?.to_str()?.to_string();
                (p.extension()? == "json").then_some(())?;
                let s = self.load(&key)?;
                Some((key, s.persona.clone(), visible_messages(&s.history).len(), s.updated))
            })
            .collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.3));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sauvegarde_reprise_et_remise_a_zero() {
        let dir = std::env::temp_dir().join(format!("repere-conv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let st = ConversationStore::new(&dir).unwrap();
        assert!(st.load("repere").is_none());
        std::fs::remove_dir_all(&dir).unwrap(); // supprimé après le démarrage : la sauvegarde le recrée
        let saved = Saved {
            persona: "repere".into(),
            history: vec![
                ChatMsg::new("user", format!("{OPENING_PREFIX}. Salue l'utilisateur.)")),
                ChatMsg::new("assistant", "Bonjour, que puis-je faire pour vous ?"),
                ChatMsg::new("user", "Mon imprimante ne répond plus"),
            ],
            summary: "résumé".into(),
            shown: vec![3, 7],
            user_name: "Marc".into(),
            updated: 42,
            citations: vec![Citation { at: 1, ids: vec![12, 15] }],
        };
        st.save("repere", &saved).unwrap();
        let back = st.load("repere").unwrap();
        assert_eq!((back.history.len(), back.summary.as_str(), back.shown.clone(), back.user_name.as_str()), (3, "résumé", vec![3, 7], "Marc"));
        assert_eq!(back.citations, vec![Citation { at: 1, ids: vec![12, 15] }]);
        assert_eq!(
            visible_messages(&back.history),
            vec![
                ("assistant".into(), "Bonjour, que puis-je faire pour vous ?".into()),
                ("user".into(), "Mon imprimante ne répond plus".into()),
            ]
        );
        assert_eq!(st.list(), vec![("repere".to_string(), "repere".to_string(), 2, 42)]);
        assert!(st.delete("repere") && st.load("repere").is_none() && !st.delete("repere"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn cles_valides_seulement_et_fichier_corrompu_ignore() {
        assert_eq!(key_for("repere").as_deref(), Some("repere"));
        assert_eq!(key_for("../etc"), None);
        let dir = std::env::temp_dir().join(format!("repere-conv-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let st = ConversationStore::new(&dir).unwrap();
        assert!(st.save("../x", &Saved::default()).is_err());
        std::fs::write(dir.join("casse.json"), "{pas du json").unwrap();
        assert!(st.load("casse").is_none() && st.list().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn une_ancienne_sauvegarde_avec_des_champs_retires_reste_lisible() {
        let dir = std::env::temp_dir().join(format!("repere-conv-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let st = ConversationStore::new(&dir).unwrap();
        std::fs::write(dir.join("ancien.json"), r#"{"persona":"ancien","history":[{"role":"user","content":"Bonjour"}],"story_mode":true,"plot":null}"#).unwrap();
        let back = st.load("ancien").unwrap();
        assert_eq!((back.persona.as_str(), back.history.len()), ("ancien", 1));
        std::fs::remove_dir_all(dir).ok();
    }
}
