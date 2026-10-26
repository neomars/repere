//! Tickets : quand l'assistant ne sait pas répondre, l'utilisateur ouvre un ticket prérempli avec le résumé de la conversation. Ce
//! module compose le brouillon et garde les tickets enregistrés dans une base locale. Aucun outil de suivi n'est branché pour
//! l'instant : le point d'entrée de la suite (GLPI, Teams…) est `TicketStore::create`, appelé quand l'utilisateur valide.

use crate::kb::Source;
use crate::llm::ChatMsg;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

const MAX_TITLE: usize = 200;
const MAX_DESCRIPTION: usize = 8000;
const MAX_NAME: usize = 100;
/// Statuts d'un ticket enregistré.
pub const STATUSES: [&str; 3] = ["nouveau", "pris en charge", "clos"];

/// Une source consultée pendant la conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsultedSource {
    pub id: i64,
    pub title: String,
    pub section: String,
    pub link: Option<String>,
}

impl ConsultedSource {
    pub fn of(s: &Source) -> Self {
        Self { id: s.id, title: s.title.clone(), section: s.section.clone(), link: s.link.clone() }
    }

    fn label(&self) -> String {
        let name = if self.section.is_empty() { self.title.clone() } else { format!("{} › {}", self.title, self.section) };
        match &self.link {
            Some(l) => format!("{name} ({l})"),
            None => name,
        }
    }
}

/// Le ticket tel qu'il est proposé à l'utilisateur, avant qu'il le relise et le valide.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Draft {
    pub title: String,
    pub description: String,
    pub sources: Vec<ConsultedSource>,
    /// Le résumé vient du modèle (sinon : les messages de l'utilisateur, tels quels).
    pub summarized: bool,
}

/// Une phrase d'un résumé ou d'un message, comme titre : « L'utilisateur n'arrive pas à se connecter au VPN (erreur 809). »
/// devient « N'arrive pas à se connecter au VPN (erreur 809) ».
fn title_from(text: &str) -> String {
    let first = text
        .split_inclusive(['.', '!', '?', '\n'])
        .map(crate::extract::clean_line)
        .find(|s| s.chars().filter(|c| c.is_alphanumeric()).count() >= 3)
        .unwrap_or_default();
    let mut t = first.trim_end_matches(['.', '!', ':', ';', ' ']).to_string();
    for prefix in ["Résumé :", "Résumé:", "Problème :", "Problème:", "L'utilisateur ", "L’utilisateur ", "Utilisateur "] {
        if let Some(rest) = t.strip_prefix(prefix) {
            t = rest.trim().to_string();
            break;
        }
    }
    if t.chars().filter(|c| c.is_alphanumeric()).count() < 3 {
        return "Demande d'assistance informatique".into();
    }
    let mut cs = t.chars();
    let mut t = cs.next().map(|c| c.to_uppercase().collect::<String>() + cs.as_str()).unwrap_or_default();
    if t.chars().count() > 90 {
        let head: String = t.chars().take(90).collect();
        let cut = head.rfind(' ').filter(|i| *i > 45).unwrap_or(head.len());
        t = format!("{}…", head[..cut].trim_end_matches([',', ';', ':', ' ']));
    }
    t
}

/// Compose le brouillon. `summary` : le résumé de la conversation par le modèle (`None` s'il n'a pas pu en faire un) ; `history` : les
/// messages visibles de la conversation. Les mots de passe qui auraient été écrits sont masqués.
pub fn compose(summary: Option<&str>, history: &[ChatMsg], user: &str, sources: &[ConsultedSource]) -> Draft {
    let said: Vec<&str> = history.iter().filter(|m| m.role == "user").map(|m| m.content.trim()).filter(|m| !m.is_empty()).collect();
    let summary = summary.map(|s| crate::secrets::mask(s.trim()).into_owned()).filter(|s| !s.is_empty());
    let mut description = match &summary {
        Some(s) => s.clone(),
        None => {
            // pas de résumé : ce que l'utilisateur a écrit, tel quel, pour que le ticket ne parte pas vide
            let mut s = String::from("Messages de l'utilisateur (résumé automatique indisponible) :");
            for m in said.iter().rev().take(6).rev() {
                s.push_str(&format!("\n- {}", crate::prompt::short(&crate::secrets::mask(m), 300)));
            }
            s
        }
    };
    if !user.trim().is_empty() {
        description.push_str(&format!("\n\nDemandeur : {}", user.trim()));
    }
    if !sources.is_empty() {
        description.push_str("\n\nSources consultées dans la base de connaissances (sans résultat suffisant) :");
        for s in sources.iter().take(6) {
            description.push_str(&format!("\n- {}", s.label()));
        }
    }
    description.push_str(&format!("\n\nTicket préparé avec l'assistant Repère ({} message(s) de l'utilisateur).", said.len()));
    let basis = summary.as_deref().or_else(|| said.first().copied()).unwrap_or("");
    Draft { title: title_from(basis), description: description.chars().take(MAX_DESCRIPTION).collect(), sources: sources.to_vec(), summarized: summary.is_some() }
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewTicket {
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub user_name: String,
    /// Profil d'assistant de la conversation d'où vient le ticket.
    #[serde(default)]
    pub conversation: String,
    #[serde(default)]
    pub sources: Vec<ConsultedSource>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ticket {
    pub id: i64,
    /// « T-0012 » : le numéro donné à l'utilisateur.
    pub reference: String,
    /// Secondes depuis l'epoch Unix.
    pub created: i64,
    pub user_name: String,
    pub title: String,
    pub description: String,
    pub conversation: String,
    pub sources: Vec<ConsultedSource>,
    pub status: String,
}

pub struct TicketStore {
    conn: Mutex<Connection>,
}

const COLS: &str = "id,created,user_name,title,description,conversation,sources,status";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Ticket> {
    let id: i64 = r.get(0)?;
    Ok(Ticket {
        id,
        reference: format!("T-{id:04}"),
        created: r.get(1)?,
        user_name: r.get(2)?,
        title: r.get(3)?,
        description: r.get(4)?,
        conversation: r.get(5)?,
        sources: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
        status: r.get(7)?,
    })
}

impl TicketStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> anyhow::Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tickets (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               created INTEGER NOT NULL,
               user_name TEXT NOT NULL DEFAULT '',
               title TEXT NOT NULL,
               description TEXT NOT NULL,
               conversation TEXT NOT NULL DEFAULT '',
               sources TEXT NOT NULL DEFAULT '[]',
               status TEXT NOT NULL DEFAULT 'nouveau'
             );",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Enregistre un ticket. Erreur lisible si le titre ou la description est vide.
    pub fn create(&self, t: &NewTicket) -> Result<Ticket, String> {
        let title = crate::extract::clean_line(&t.title);
        let description = t.description.trim().to_string();
        if title.is_empty() {
            return Err("le titre du ticket est vide".into());
        }
        if description.is_empty() {
            return Err("la description du ticket est vide".into());
        }
        if title.chars().count() > MAX_TITLE || description.chars().count() > MAX_DESCRIPTION {
            return Err(format!("ticket trop long (titre {MAX_TITLE} caractères, description {MAX_DESCRIPTION} au plus)"));
        }
        let name: String = crate::extract::clean_line(&t.user_name).chars().take(MAX_NAME).collect();
        let conversation: String = t.conversation.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).take(64).collect();
        let sources = serde_json::to_string(&t.sources.iter().take(20).collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into());
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO tickets (created,user_name,title,description,conversation,sources) VALUES (?1,?2,?3,?4,?5,?6)",
            params![now, name, title, description, conversation, sources],
        )
        .map_err(|e| format!("enregistrement impossible : {e}"))?;
        let id = c.last_insert_rowid();
        c.query_row(&format!("SELECT {COLS} FROM tickets WHERE id=?1"), [id], from_row).map_err(|e| e.to_string())
    }

    /// Les tickets, du plus récent au plus ancien.
    pub fn list(&self, limit: usize) -> Vec<Ticket> {
        let c = self.conn.lock().unwrap();
        let Ok(mut st) = c.prepare(&format!("SELECT {COLS} FROM tickets ORDER BY id DESC LIMIT ?1")) else { return vec![] };
        st.query_map([limit.clamp(1, 1000) as i64], from_row).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    pub fn set_status(&self, id: i64, status: &str) -> Result<Option<Ticket>, String> {
        if !STATUSES.contains(&status) {
            return Err(format!("statut inconnu (attendu : {})", STATUSES.join(", ")));
        }
        let c = self.conn.lock().unwrap();
        let n = c.execute("UPDATE tickets SET status=?2 WHERE id=?1", params![id, status]).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(None);
        }
        Ok(c.query_row(&format!("SELECT {COLS} FROM tickets WHERE id=?1"), [id], from_row).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, text: &str) -> ChatMsg {
        ChatMsg::new(role, text)
    }

    fn source(id: i64, section: &str, link: Option<&str>) -> ConsultedSource {
        ConsultedSource { id, title: "Guide VPN".into(), section: section.into(), link: link.map(String::from) }
    }

    #[test]
    fn le_titre_vient_de_la_premiere_phrase() {
        assert_eq!(title_from("L'utilisateur n'arrive pas à se connecter au VPN (erreur 809). Il a essayé de redémarrer."), "N'arrive pas à se connecter au VPN (erreur 809)");
        assert_eq!(title_from("Résumé : imprimante bloquée au 2e étage."), "Imprimante bloquée au 2e étage");
        assert_eq!(title_from("mon wifi ne marche plus\nmerci"), "Mon wifi ne marche plus");
        assert_eq!(title_from("   ...   "), "Demande d'assistance informatique");
        assert_eq!(title_from(""), "Demande d'assistance informatique");
        let long = title_from("Le client VPN refuse de se connecter depuis le domicile de l'agent malgré plusieurs redémarrages successifs du poste de travail");
        assert!(long.chars().count() <= 91 && long.ends_with('…'), "{long}");
    }

    #[test]
    fn le_brouillon_reprend_le_resume_les_sources_et_le_demandeur() {
        let history = vec![msg("assistant", "Bonjour !"), msg("user", "Mon VPN affiche l'erreur 809"), msg("assistant", "Autorisez les ports UDP."), msg("user", "Ça ne marche toujours pas")];
        let sources = vec![source(12, "Erreur 809", Some("https://intranet.exemple.fr/vpn")), source(15, "", None)];
        let d = compose(Some("L'utilisateur ne peut pas se connecter au VPN (erreur 809). Il a ouvert les ports UDP 500 et 4500 sans succès."), &history, "Marc", &sources);
        assert_eq!(d.title, "Ne peut pas se connecter au VPN (erreur 809)");
        assert!(d.summarized);
        assert!(d.description.starts_with("L'utilisateur ne peut pas se connecter au VPN (erreur 809)."));
        assert!(d.description.contains("Demandeur : Marc"));
        assert!(d.description.contains("- Guide VPN › Erreur 809 (https://intranet.exemple.fr/vpn)\n- Guide VPN"));
        assert!(d.description.contains("2 message(s) de l'utilisateur"));
        assert_eq!(d.sources, sources);
    }

    #[test]
    fn sans_resume_le_ticket_reprend_les_messages_de_l_utilisateur() {
        let history = vec![msg("user", "Mon VPN affiche l'erreur 809"), msg("assistant", "…"), msg("user", "Mon mot de passe : Printemps2024! ne marche pas")];
        let d = compose(None, &history, "", &[]);
        assert!(!d.summarized);
        assert_eq!(d.title, "Mon VPN affiche l'erreur 809");
        assert!(d.description.starts_with("Messages de l'utilisateur (résumé automatique indisponible) :\n- Mon VPN affiche l'erreur 809\n- Mon mot de passe : [masqué]"), "{}", d.description);
        assert!(!d.description.contains("Printemps2024") && !d.description.contains("Demandeur"));
        // une conversation sans rien : un ticket tout de même utilisable
        let vide = compose(None, &[], "", &[]);
        assert_eq!(vide.title, "Demande d'assistance informatique");
        let secret = compose(Some("Mot de passe : Azerty123 refusé."), &[], "", &[]);
        assert!(!secret.description.contains("Azerty123"));
    }

    #[test]
    fn les_tickets_sont_enregistres_listes_et_suivis() {
        let store = TicketStore::open_in_memory().unwrap();
        let nouveau = |title: &str| NewTicket { title: title.into(), description: "Détails.".into(), user_name: "  Marc   Dupont ".into(), conversation: "repere/../x".into(), sources: vec![source(1, "", None)] };
        let a = store.create(&nouveau("  VPN   erreur 809 ")).unwrap();
        let b = store.create(&nouveau("Imprimante")).unwrap();
        assert_eq!((a.reference.as_str(), a.title.as_str(), a.user_name.as_str(), a.status.as_str()), ("T-0001", "VPN erreur 809", "Marc Dupont", "nouveau"));
        assert_eq!(a.conversation, "reperex", "seuls les caractères d'un nom de profil sont gardés");
        assert_eq!(store.list(10).iter().map(|t| t.id).collect::<Vec<_>>(), vec![b.id, a.id], "le plus récent d'abord");
        assert_eq!(store.list(1).len(), 1);
        assert_eq!(store.set_status(a.id, "clos").unwrap().unwrap().status, "clos");
        assert!(store.set_status(a.id, "n'importe quoi").is_err());
        assert_eq!(store.set_status(999, "clos").unwrap(), None);
        // refus lisibles
        assert!(store.create(&nouveau("  ")).unwrap_err().contains("titre"));
        let sans = NewTicket { description: " ".into(), ..nouveau("Titre") };
        assert!(store.create(&sans).unwrap_err().contains("description"));
        let long = NewTicket { description: "x".repeat(MAX_DESCRIPTION + 1), ..nouveau("Titre") };
        assert!(store.create(&long).unwrap_err().contains("trop long"));
    }
}
