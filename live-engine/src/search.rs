//! Recherche des sources (extraits de documents, images) qui répondent à la conversation en cours.
//!
//! Chaque source est indexée par les racines de ses mots (voir `terms`), pesées selon l'endroit où elles figurent : étiquettes et
//! titres comptent plus que le texte. Le dictionnaire de synonymes (`thesaurus`) ajoute à l'index et à la question des « concepts » :
//! « mdp » dans la question rejoint « mot de passe » dans un document. Le classement est un BM25 ; une source n'est proposée que si
//! elle partage assez de mots utiles avec la question (voir `Index::search`), de sorte qu'une question hors sujet ne ramène rien.

use crate::kb::Source;
use crate::terms::{is_generic, terms};
use crate::thesaurus::Thesaurus;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::SystemTime;

/// Saturation de la fréquence d'un mot et prise en compte de la longueur du texte (BM25).
const K1: f64 = 1.2;
const B: f64 = 0.6;
/// Poids d'un mot selon l'endroit où il figure.
const W_TAG: f32 = 3.0;
const W_TITLE: f32 = 2.5;
const W_BODY: f32 = 1.0;
/// Poids, dans la question, d'un concept de même sens que ses mots, et d'un concept seulement lié.
const SYNONYM: f64 = 0.8;
const RELATED: f64 = 0.25;
/// Poids d'un mot courant de la langue (« joindre », « donner »), qui n'est pas dans le dictionnaire : dans une petite base, un mot
/// courant qui ne figure que dans un ou deux extraits paraît très rare, donc très parlant, alors qu'il ne l'est pas.
const GENERIC: f64 = 0.35;
/// Une source proposée atteint au moins cette part du score de la meilleure.
const RELATIVE_FLOOR: f64 = 0.3;
/// Extraits d'un même document proposés au plus : un guide qui parle de VPN partout ne doit pas évincer les autres.
const PER_DOCUMENT: usize = 3;

/// Une source dans l'index, avec la longueur de son texte.
struct Entry {
    source: Source,
    len: f64,
}

/// Une occurrence : numéro de la source, fréquence pesée du mot, présence dans un titre ou une étiquette.
type Posting = (u32, f32, bool);

pub struct Index {
    pub generation: u64,
    /// Version du fichier de synonymes de l'utilisateur avec lequel l'index a été construit.
    pub stamp: Option<SystemTime>,
    thesaurus: Arc<Thesaurus>,
    entries: Vec<Entry>,
    /// Racine (ou concept : `@n` pour des mots de même sens, `~n` pour des mots liés) → sources qui la contiennent.
    postings: HashMap<String, Vec<Posting>>,
    avg_len: f64,
}

/// Ce que la recherche a trouvé, avec de quoi expliquer pourquoi (écran d'essai de la base de connaissances).
#[derive(Debug, Default, Clone)]
pub struct Report {
    /// Racines de la question et leur poids.
    pub terms: Vec<(String, f64)>,
    /// Concepts du dictionnaire évoqués par la question.
    pub concepts: Vec<String>,
    /// Racines de la question qu'aucune source ne contient (ni aucun mot de même sens).
    pub unknown: Vec<String>,
    /// Sources retenues, la plus pertinente d'abord.
    pub hits: Vec<Hit>,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub source: Source,
    pub score: f64,
    /// Nombre de mots (ou concepts) de la question trouvés dans la source.
    pub matched: usize,
}

impl Index {
    pub fn build(generation: u64, stamp: Option<SystemTime>, thesaurus: Arc<Thesaurus>, sources: Vec<Source>) -> Self {
        let mut entries = Vec::with_capacity(sources.len());
        let mut postings: HashMap<String, Vec<Posting>> = HashMap::new();
        let mut total_len = 0.0;
        for source in sources {
            let tag_seqs: Vec<Vec<String>> = source.tags.iter().map(|t| terms(t)).filter(|s| !s.is_empty()).collect();
            let title_seqs = vec![terms(&source.title), terms(&source.section)];
            let body = terms(&source.description);

            // fréquence pesée de chaque racine, et présence dans un titre ou une étiquette
            let mut tf: HashMap<String, (f32, bool)> = HashMap::new();
            let mut add = |stems: &[String], weight: f32, head: bool| {
                for w in stems {
                    let e = tf.entry(w.clone()).or_insert((0.0, false));
                    e.0 += weight;
                    e.1 |= head;
                }
            };
            for t in &tag_seqs {
                add(t, W_TAG, true);
            }
            for t in &title_seqs {
                add(t, W_TITLE, true);
            }
            add(&body, W_BODY, false);
            // concepts du dictionnaire présents dans le texte : mots de même sens (`@n`) ou seulement liés (`~n`)
            let fields: [(f32, &[Vec<String>]); 3] = [(W_TAG, &tag_seqs), (W_TITLE, &title_seqs), (W_BODY, std::slice::from_ref(&body))];
            for (c, (strong, weak)) in thesaurus.doc_concepts(&fields) {
                if strong > 0.0 {
                    tf.insert(format!("@{c}"), (strong, strong >= W_TITLE));
                }
                if weak > 0.0 {
                    tf.insert(format!("~{c}"), (weak, weak >= W_TITLE));
                }
            }
            let len = body.len() as f64 + 0.5 * (tag_seqs.iter().map(Vec::len).sum::<usize>() + title_seqs.iter().map(Vec::len).sum::<usize>()) as f64;
            total_len += len;
            let n = entries.len() as u32;
            for (w, (f, head)) in tf {
                postings.entry(w).or_default().push((n, f, head));
            }
            entries.push(Entry { source, len });
        }
        let avg_len = if entries.is_empty() { 1.0 } else { (total_len / entries.len() as f64).max(1.0) };
        Self { generation, stamp, thesaurus, entries, postings, avg_len }
    }

    /// Rareté d'un mot : forte s'il figure dans peu de sources, faible (mais jamais nulle) s'il est partout. Sert à décider si une
    /// source est assez proche de la question pour être proposée.
    fn rarity(&self, df: usize) -> f64 {
        (((self.entries.len() + 1) as f64) / (df as f64 + 0.5)).ln().max(0.1)
    }

    /// Poids du mot dans le classement (BM25).
    fn idf(&self, df: usize) -> f64 {
        let n = self.entries.len() as f64;
        (1.0 + (n - df as f64 + 0.5) / (df as f64 + 0.5)).ln()
    }

    /// Les sources qui répondent à la conversation, de la plus pertinente à la moins pertinente. `parts` = textes de la
    /// conversation avec leur poids (le dernier message compte plus que le résumé). Une source doit répondre à au moins deux
    /// mots de la question (ou à un seul, qui la désigne : rare, ou dans son titre, et dit dans le dernier message), pesés par
    /// leur rareté. Un mot est « retrouvé » dans une source quand elle le contient, ou contient un mot de même sens
    /// (« mdp » et « mot de passe »). Les sources de `exclude` (images déjà montrées) sont écartées.
    pub fn search(&self, parts: &[(String, f64)], exclude: &HashSet<i64>, limit: usize) -> Report {
        let mut report = Report::default();
        if self.entries.is_empty() || limit == 0 {
            return report;
        }
        // 1. la question : chaque mot (racine) et les concepts qu'il évoque ; chacun garde le poids du message d'où il vient
        struct Word {
            weight: f64,
            keys: Vec<String>, // la racine, puis `@n` pour chaque concept de même sens qu'elle évoque
            /// Mot courant que le dictionnaire ne connaît pas : il compte moins, dans le classement comme dans la pertinence.
            generic: bool,
        }
        let mut words: HashMap<String, Word> = HashMap::new();
        let mut weights: HashMap<String, f64> = HashMap::new(); // poids de chaque clé dans le classement
        let mut concepts: Vec<u32> = vec![];
        for (text, w) in parts {
            let stems = terms(text);
            let spans = self.thesaurus.spans(&stems);
            for (i, stem) in stems.iter().enumerate() {
                let word = words.entry(stem.clone()).or_insert_with(|| Word { weight: 0.0, keys: vec![stem.clone()], generic: false });
                word.weight = word.weight.max(*w);
                for (c, start, len) in &spans {
                    let key = format!("@{c}");
                    if (*start..start + len).contains(&i) && !word.keys.contains(&key) {
                        word.keys.push(key);
                    }
                }
                let e = weights.entry(stem.clone()).or_insert(0.0);
                *e = e.max(*w);
            }
            for (c, _, _) in &spans {
                if !concepts.contains(c) {
                    concepts.push(*c);
                }
                for (key, weight) in [(format!("@{c}"), SYNONYM * w), (format!("~{c}"), RELATED * w)] {
                    let e = weights.entry(key).or_insert(0.0);
                    *e = e.max(weight);
                }
            }
        }
        for (stem, word) in words.iter_mut() {
            word.generic = word.keys.len() == 1 && is_generic(stem);
            if word.generic {
                if let Some(w) = weights.get_mut(stem) {
                    *w *= GENERIC;
                }
            }
        }
        // un mot que personne n'a écrit nulle part ne départage rien
        let mut unknown: Vec<String> = words.iter().filter(|(_, w)| w.keys.iter().all(|k| !self.postings.contains_key(k))).map(|(s, _)| s.clone()).collect();
        unknown.sort();
        words.retain(|_, w| w.keys.iter().any(|k| self.postings.contains_key(k)));
        weights.retain(|k, _| self.postings.contains_key(k));
        report.unknown = unknown;
        report.concepts = concepts.iter().map(|c| self.thesaurus.name(*c).to_string()).collect();
        report.terms = {
            let mut t: Vec<(String, f64)> = words.iter().map(|(s, w)| (s.clone(), w.weight)).collect();
            t.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
            t
        };
        if words.is_empty() {
            return report;
        }

        // 2. classement : BM25 sur toutes les clés (mots, concepts de même sens, concepts liés)
        let mut score: HashMap<u32, f64> = HashMap::new();
        for (key, weight) in &weights {
            let post = &self.postings[key];
            let idf = self.idf(post.len());
            for &(n, tf, _) in post {
                let tf = tf as f64;
                let norm = K1 * (1.0 - B + B * self.entries[n as usize].len / self.avg_len);
                *score.entry(n).or_default() += weight * idf * (tf * (K1 + 1.0)) / (tf + norm);
            }
        }

        // 3. pertinence : à combien de mots de la question la source répond-elle ?
        #[derive(Default)]
        struct Fit {
            matched: usize,
            rarity_sum: f64,
            designated: bool,
        }
        let mut fit: HashMap<u32, Fit> = HashMap::new();
        let few = (self.entries.len() / 20).max(1);
        for word in words.values() {
            // un mot courant de la langue (« jour », « donner ») ne désigne rien à lui seul, sauf si le dictionnaire le connaît
            let speaks = word.weight >= 0.9 && !word.generic;
            let factor = if word.generic { GENERIC } else { 1.0 };
            // pour chaque source, le meilleur des moyens qu'elle a de répondre à ce mot
            let mut best: HashMap<u32, (f64, bool)> = HashMap::new();
            for key in &word.keys {
                let Some(post) = self.postings.get(key) else { continue };
                let rarity = word.weight * factor * self.rarity(post.len());
                let rare = post.len() <= few;
                for &(n, _, head) in post {
                    let e = best.entry(n).or_insert((0.0, false));
                    e.0 = e.0.max(rarity);
                    e.1 |= speaks && (rare || head);
                }
            }
            for (n, (rarity, designated)) in best {
                let f = fit.entry(n).or_default();
                f.matched += 1;
                f.rarity_sum += rarity;
                f.designated |= designated;
            }
        }
        let threshold = (0.9 * ((self.entries.len() + 1) as f64).ln()).min(3.0);
        let mut ranked: Vec<(f64, usize, &Entry)> = fit
            .into_iter()
            .filter_map(|(n, f)| {
                let e = &self.entries[n as usize];
                if e.source.kind != crate::kb::PASSAGE && exclude.contains(&e.source.id) {
                    return None;
                }
                // deux mots dont le poids cumulé compte, ou un seul qui désigne la source
                if (f.matched >= 2 && f.rarity_sum < threshold) || (f.matched < 2 && !f.designated) {
                    return None;
                }
                Some((score.get(&n).copied().unwrap_or(0.0), f.matched, e))
            })
            .collect();
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.2.source.id.cmp(&b.2.source.id)));

        // 4. le haut du classement, sans noyer la réponse sous un seul document ni sous des sources bien plus faibles que la première
        let best = ranked.first().map_or(0.0, |r| r.0);
        let mut per_document: HashMap<i64, usize> = HashMap::new();
        for (score, matched, e) in ranked {
            if report.hits.len() >= limit || score < RELATIVE_FLOOR * best {
                break;
            }
            if let Some(d) = e.source.doc_id {
                let n = per_document.entry(d).or_default();
                if *n >= PER_DOCUMENT {
                    continue;
                }
                *n += 1;
            }
            report.hits.push(Hit { source: e.source.clone(), score, matched });
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passage(id: i64, doc: i64, title: &str, section: &str, text: &str) -> Source {
        Source { id, kind: crate::kb::PASSAGE.into(), title: title.into(), section: section.into(), description: text.into(), doc_id: Some(doc), ..Default::default() }
    }

    fn index() -> Index {
        let sources = vec![
            passage(1, 1, "Client VPN", "Erreur 809", "L'erreur 809 signifie que le pare-feu bloque les ports UDP 500 et 4500 du client VPN."),
            passage(2, 1, "Client VPN", "Erreur 691", "L'erreur 691 : identifiant ou mot de passe refusé par le client VPN."),
            passage(3, 2, "Imprimantes", "File bloquée", "Si l'imprimante est bloquée, videz la file d'attente d'impression."),
        ];
        Index::build(1, None, Arc::new(Thesaurus::with_extra("")), sources)
    }

    fn ids(ix: &Index, parts: &[(&str, f64)]) -> Vec<i64> {
        let parts: Vec<(String, f64)> = parts.iter().map(|(t, w)| (t.to_string(), *w)).collect();
        ix.search(&parts, &HashSet::new(), 4).hits.iter().map(|h| h.source.id).collect()
    }

    #[test]
    fn un_message_de_suite_garde_le_contexte() {
        let ix = index();
        assert_eq!(ids(&ix, &[("Mon VPN affiche l'erreur 809", 1.0)]), vec![1]);
        // « ça ne marche toujours pas » n'a que des mots courants : le sujet reste le VPN
        assert_eq!(ids(&ix, &[("J'ai essayé mais ça ne marche toujours pas", 1.0), ("Mon VPN affiche l'erreur 809", 0.7)]), vec![1]);
    }

    #[test]
    fn une_question_hors_sujet_ne_ramene_rien_sans_contexte() {
        let ix = index();
        assert!(ids(&ix, &[("Quelle est la capitale de la Mongolie ?", 1.0)]).is_empty());
        assert!(ids(&ix, &[("Bonjour, merci pour votre aide", 1.0)]).is_empty());
    }
}
