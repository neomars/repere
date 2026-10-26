//! Évaluation de la recherche sur le jeu de `eval/` : le corpus est importé comme le ferait un administrateur (Markdown, HTML, Word,
//! PDF, texte), puis chacune des 30 questions est posée à la recherche, avec les échanges qui la précèdent. On vérifie que les
//! passages proposés à l'IA contiennent le bon document et que les questions sans rapport avec la base ne ramènent rien.
//!
//! Ce test ne dit rien de la qualité des réponses du modèle : pour cela, `eval/run.mjs` pose les mêmes questions à un moteur démarré
//! avec un vrai modèle. `cargo test eval -- --nocapture` affiche le détail question par question.

use crate::ingest::{ingest, Outcome, Upload};
use crate::kb::KnowledgeBase;
use crate::llm::ChatMsg;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Deserialize)]
struct Fichier {
    questions: Vec<Question>,
}

#[derive(Deserialize)]
struct Question {
    id: String,
    category: String,
    #[serde(default)]
    turns: Vec<Echange>,
    question: String,
    expect: Attendu,
}

#[derive(Deserialize)]
struct Echange {
    role: String,
    text: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Attendu {
    documents: Vec<String>,
    sections: Vec<String>,
    no_sources: bool,
}

fn racine() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../eval")
}

/// Le corpus d'évaluation, importé dans une base en mémoire.
async fn base() -> KnowledgeBase {
    let kb = KnowledgeBase::open_in_memory().unwrap();
    let copies = std::env::temp_dir().join(format!("repere-eval-{}", std::process::id()));
    let mut fichiers: Vec<_> = std::fs::read_dir(racine().join("corpus")).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    fichiers.sort();
    for f in fichiers {
        let nom = f.file_name().unwrap().to_string_lossy().into_owned();
        let o = ingest(&kb, &copies, Upload { name: nom.clone(), bytes: std::fs::read(&f).unwrap(), ..Default::default() }).await;
        assert!(matches!(o, Outcome::Added { .. }), "{nom} : {o:?}");
    }
    std::fs::remove_dir_all(copies).ok();
    kb
}

/// Ce que la recherche reçoit à ce tour : la question, précédée des messages de l'utilisateur (comme dans une vraie conversation).
fn textes(q: &Question) -> Vec<(String, f64)> {
    let mut histoire: Vec<ChatMsg> = q.turns.iter().map(|t| ChatMsg::new(&t.role, &t.text)).collect();
    histoire.push(ChatMsg::new("user", &q.question));
    crate::ws::scene_parts(&histoire, "")
}

fn plie(s: &str) -> String {
    crate::terms::fold(s).split_whitespace().collect::<Vec<_>>().join(" ")
}

#[tokio::test]
async fn la_recherche_propose_les_bons_passages_pour_les_30_questions() {
    let kb = base().await;
    let fichier: Fichier = serde_json::from_str(&std::fs::read_to_string(racine().join("questions.json")).unwrap()).unwrap();
    assert_eq!(fichier.questions.len(), 30, "le jeu compte 30 questions");

    let (mut a_trouver, mut trouvees, mut somme_rangs) = (0usize, 0usize, 0.0f64);
    let (mut sans_rapport, mut vides) = (0usize, 0usize);
    let mut echecs: Vec<String> = vec![];
    println!("\n{:<4} {:<22} {:<4} {:>3}  question", "id", "catégorie", "rang", "nb");
    for q in &fichier.questions {
        let rapport = kb.search(&textes(q), &HashSet::new(), crate::ws::MAX_PICKS);
        if std::env::var_os("DETAIL").is_some() {
            println!("  {} mots : {:?} ; concepts : {:?} ; inconnus : {:?}", q.id, rapport.terms, rapport.concepts, rapport.unknown);
            for h in &rapport.hits {
                println!("    {:.2} ({} mots) {} › {}", h.score, h.matched, h.source.title, h.source.section);
            }
        }
        let propose: Vec<_> = rapport.hits.into_iter().map(|h| h.source).collect();
        let noms: Vec<String> = propose.iter().map(|s| s.doc_id.and_then(|d| kb.document(d)).map(|d| d.name).unwrap_or_default()).collect();
        let attendu = &q.expect;
        let rang = noms.iter().position(|n| attendu.documents.contains(n)).map(|i| i + 1);
        println!("{:<4} {:<22} {:<4} {:>3}  {}", q.id, q.category, rang.map_or("-".into(), |r| r.to_string()), propose.len(), q.question);
        if !attendu.documents.is_empty() {
            a_trouver += 1;
            match rang {
                Some(r) => {
                    trouvees += 1;
                    somme_rangs += 1.0 / r as f64;
                }
                None => echecs.push(format!("{} : aucun passage de {:?} proposé (reçu : {:?})", q.id, attendu.documents, noms)),
            }
            if !attendu.sections.is_empty() {
                let bon = propose.iter().zip(&noms).any(|(s, n)| attendu.documents.contains(n) && attendu.sections.iter().any(|sec| plie(&s.section).contains(&plie(sec))));
                if !bon {
                    echecs.push(format!("{} : aucun passage proposé ne porte la section {:?} (reçu : {:?})", q.id, attendu.sections, propose.iter().map(|s| s.section.clone()).collect::<Vec<_>>()));
                }
            }
        }
        if attendu.no_sources {
            sans_rapport += 1;
            if propose.is_empty() {
                vides += 1;
            } else {
                echecs.push(format!("{} : question sans rapport avec la base, mais {} passage(s) proposés : {:?}", q.id, propose.len(), propose.iter().map(|s| format!("{} › {}", s.title, s.section)).collect::<Vec<_>>()));
            }
        }
    }
    println!("\nrappel : {trouvees}/{a_trouver} questions dont le bon document est proposé ; rang moyen inverse {:.2}", somme_rangs / a_trouver as f64);
    println!("sans rapport : {vides}/{sans_rapport} questions pour lesquelles rien n'est proposé");
    assert!(echecs.is_empty(), "\n{}", echecs.join("\n"));
    assert!(somme_rangs / a_trouver as f64 >= 0.85, "le bon document doit le plus souvent venir en premier");
}
