//! Découpage d'un document en passages : des morceaux d'une demi-page environ, chacun avec le chemin de titres qui l'amène (« Guide VPN ›
//! Erreurs courantes › Erreur 809 »). Un passage ne coupe pas une liste d'étapes en deux tant qu'elle tient, garde une phrase
//! d'introduction (« Marche à suivre : ») avec ce qu'elle annonce, et ne mélange des sections voisines que si elles sont minuscules.

use super::Block;

/// Sous cette taille, un passage est trop pauvre pour être cité seul : on lui rattache la suite.
pub const MIN_CHARS: usize = 300;
/// Taille visée d'un passage.
pub const TARGET_CHARS: usize = 900;
/// Taille qu'un passage ne dépasse que s'il ne peut pas être coupé (rare).
pub const MAX_CHARS: usize = 1400;
/// Une liste d'étapes ou un tableau plus court que ça n'est jamais coupé.
const MAX_WHOLE: usize = 1600;

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Titres de section, du plus général au plus précis (sans le titre du document lui-même).
    pub section: Vec<String>,
    /// Titres des sections voisines, minuscules, rattachées à ce passage (leur texte suit celui de `section`).
    pub merged: Vec<String>,
    pub text: String,
    /// Page du début du passage (PDF).
    pub page: Option<u32>,
}

impl Chunk {
    /// Le chemin du passage tel qu'on l'affiche : « Erreurs courantes › Erreur 691 + Erreur 720 ».
    pub fn label(&self) -> String {
        let mut label = self.section.join(" › ");
        for m in &self.merged {
            if !label.is_empty() {
                label.push_str(" + ");
            }
            label.push_str(m);
        }
        label
    }
}

fn len(s: &str) -> usize {
    s.chars().count()
}

/// Un élément à placer : une ou plusieurs lignes qu'on préfère ne pas séparer.
#[derive(Debug, Clone)]
struct Unit {
    /// Numéro de la section d'où vient l'élément.
    section: usize,
    lines: Vec<String>,
    page: Option<u32>,
    /// Colle cet élément au suivant (phrase d'introduction d'une liste).
    glue: bool,
}

impl Unit {
    fn size(&self) -> usize {
        self.lines.iter().map(|l| len(l) + 1).sum::<usize>().saturating_sub(1)
    }
}

/// Une ligne par élément de liste, en retrait selon la profondeur.
fn render_item(depth: u8, number: Option<u32>, text: &str) -> String {
    let marker = number.map_or_else(|| "- ".to_string(), |n| format!("{n}. "));
    format!("{}{marker}{text}", "  ".repeat(depth.min(4) as usize))
}

/// Une ligne par ligne de tableau. Avec un en-tête : « Code : 809 ; Cause : Pare-feu », ce qui garde le lien entre la valeur et sa
/// colonne quand le passage est lu hors de son tableau.
fn render_table(header: bool, rows: &[Vec<String>]) -> Vec<String> {
    let cells = |r: &Vec<String>| r.iter().map(|c| c.trim().to_string()).collect::<Vec<_>>();
    if header && rows.len() >= 2 && rows[0].iter().all(|h| !h.trim().is_empty()) {
        let head = cells(&rows[0]);
        return rows[1..]
            .iter()
            .map(|r| {
                cells(r).iter().enumerate().filter(|(_, c)| !c.is_empty()).map(|(i, c)| match head.get(i) {
                    Some(h) => format!("{h} : {c}"),
                    None => c.clone(),
                }).collect::<Vec<_>>().join(" ; ")
            })
            .filter(|l| !l.is_empty())
            .collect();
    }
    rows.iter().map(|r| cells(r).into_iter().filter(|c| !c.is_empty()).collect::<Vec<_>>().join(" | ")).filter(|l| !l.is_empty()).collect()
}

/// Mots dont le point ne termine pas une phrase.
const ABBREVIATIONS: [&str; 16] = ["m", "mm", "mme", "mlle", "dr", "pr", "prof", "st", "ste", "cf", "vs", "etc", "env", "tel", "ex", "fig"];

/// Découpe un texte en phrases (la ponctuation reste à sa phrase). Un point suivi d'une minuscule, d'un chiffre collé ou d'une
/// abréviation connue ne termine rien.
fn sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![];
    let mut start = 0;
    for i in 0..chars.len() {
        if !matches!(chars[i], '.' | '!' | '?' | '…') {
            continue;
        }
        if chars.get(i + 1).is_some_and(|c| matches!(c, '.' | '!' | '?' | '…')) {
            continue; // « ... » : on coupe après le dernier point
        }
        let Some(next) = chars[i + 1..].iter().find(|c| !c.is_whitespace()) else { break };
        let spaced = chars.get(i + 1).is_some_and(|c| c.is_whitespace());
        if !spaced || !(next.is_uppercase() || next.is_ascii_digit() || matches!(next, '«' | '"' | '(' | '-' | '•')) {
            continue;
        }
        let word: String = chars[start..i].iter().rev().take_while(|c| c.is_alphanumeric()).collect::<Vec<_>>().into_iter().rev().collect();
        let is_initial = word.chars().count() == 1 && word.chars().all(char::is_uppercase);
        if chars[i] == '.' && (is_initial || ABBREVIATIONS.contains(&word.to_lowercase().as_str())) {
            continue;
        }
        out.push(chars[start..=i].iter().collect::<String>().trim().to_string());
        start = i + 1;
    }
    let rest: String = chars[start..].iter().collect();
    if !rest.trim().is_empty() {
        out.push(rest.trim().to_string());
    }
    out
}

/// Coupe un texte trop long en morceaux de `max` caractères au plus, aux phrases d'abord, puis aux espaces.
fn split_text(text: &str, max: usize) -> Vec<String> {
    let mut pieces: Vec<String> = vec![];
    let mut cur = String::new();
    let push = |cur: &mut String, pieces: &mut Vec<String>| {
        if !cur.trim().is_empty() {
            pieces.push(cur.trim().to_string());
        }
        cur.clear();
    };
    for s in sentences(text) {
        let mut sentence = s;
        while len(&sentence) > max {
            // phrase sans ponctuation : coupée à la dernière espace avant la limite
            let head: String = sentence.chars().take(max).collect();
            let cut = head.rfind(' ').filter(|i| *i > max / 2).unwrap_or(head.len());
            if len(&cur) > 0 {
                push(&mut cur, &mut pieces);
            }
            pieces.push(head[..cut].trim().to_string());
            sentence = sentence[cut..].trim_start().to_string();
        }
        if len(&cur) + 1 + len(&sentence) > max {
            push(&mut cur, &mut pieces);
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(&sentence);
    }
    push(&mut cur, &mut pieces);
    pieces
}

/// Les éléments qui dépassent la taille maximale sont coupés (une phrase d'abord ; une ligne de code ou de tableau à la ligne).
fn split_unit(u: Unit, max: usize) -> Vec<Unit> {
    if u.size() <= max {
        return vec![u];
    }
    let mut out = vec![];
    if u.lines.len() > 1 {
        // plusieurs lignes (liste, tableau, code) : coupé entre deux lignes, au plus près de la taille visée
        let mut cur: Vec<String> = vec![];
        let mut size = 0;
        for line in u.lines.iter().flat_map(|l| if len(l) > max { split_text(l, max) } else { vec![l.clone()] }) {
            if size > 0 && size + 1 + len(&line) > TARGET_CHARS.min(max) {
                out.push(Unit { lines: std::mem::take(&mut cur), glue: false, ..u.clone() });
                size = 0;
            }
            size += len(&line) + usize::from(size > 0);
            cur.push(line);
        }
        if !cur.is_empty() {
            out.push(Unit { lines: cur, ..u.clone() });
        }
        return out;
    }
    let pieces = split_text(&u.lines[0], max);
    let n = pieces.len();
    for (i, p) in pieces.into_iter().enumerate() {
        out.push(Unit { lines: vec![p], glue: u.glue && i + 1 == n, ..u.clone() });
    }
    out
}

struct Cur {
    /// Section où commence le passage (son chemin le nomme) et section du dernier élément ajouté.
    first: usize,
    last: usize,
    merged: Vec<String>,
    parts: Vec<String>,
    size: usize,
    page: Option<u32>,
}

/// Découpe `blocks` en passages. `doc_title` sert à ne pas répéter le titre du document dans le chemin des sections.
pub fn chunk(doc_title: &str, blocks: &[Block]) -> Vec<Chunk> {
    // 1. éléments, avec la section d'où ils viennent
    let mut headings: Vec<Vec<String>> = vec![vec![]]; // chemin de chaque section (la section 0 précède tout titre)
    let mut heading_text: Vec<String> = vec![String::new()];
    let mut stack: Vec<(u8, String)> = vec![];
    let mut units: Vec<Unit> = vec![];
    let mut page: Option<u32> = None;
    let mut run: Vec<String> = vec![]; // éléments de liste ou lignes de tableau consécutifs
    let mut run_section = 0;
    let mut run_page: Option<u32> = None;

    fn close_run(run: &mut Vec<String>, section: usize, page: Option<u32>, units: &mut Vec<Unit>) {
        if !run.is_empty() {
            units.push(Unit { section, lines: std::mem::take(run), page, glue: false });
        }
    }
    let title_key = crate::terms::fold(doc_title).trim().to_string();

    for b in blocks {
        let section = headings.len() - 1;
        match b {
            Block::Page(n) => page = Some(*n),
            Block::Heading { level, text } => {
                close_run(&mut run, run_section, run_page, &mut units);
                while stack.last().is_some_and(|(l, _)| *l >= *level) {
                    stack.pop();
                }
                stack.push((*level, text.clone()));
                let path: Vec<String> = stack.iter().map(|(_, t)| t.clone()).collect();
                // le titre du document, répété en tête, n'est pas une section
                let path = if path.first().is_some_and(|t| crate::terms::fold(t).trim() == title_key) { path[1..].to_vec() } else { path };
                headings.push(path);
                heading_text.push(text.clone());
            }
            Block::Item { depth, number, text } => {
                if run.is_empty() {
                    (run_section, run_page) = (section, page);
                }
                run.push(render_item(*depth, *number, text));
            }
            Block::Table { header, rows } => {
                close_run(&mut run, run_section, run_page, &mut units);
                let lines = render_table(*header, rows);
                if !lines.is_empty() {
                    units.push(Unit { section, lines, page, glue: false });
                }
            }
            Block::Para(t) => {
                close_run(&mut run, run_section, run_page, &mut units);
                // une phrase qui annonce ce qui suit (« Marche à suivre : ») reste avec la suite
                units.push(Unit { section, lines: vec![t.clone()], page, glue: t.trim_end().ends_with(':') });
            }
            Block::Code(c) => {
                close_run(&mut run, run_section, run_page, &mut units);
                units.push(Unit { section, lines: c.lines().map(|l| l.trim_end().to_string()).collect(), page, glue: false });
            }
        }
        // une liste est attachée à la phrase qui l'introduit, mais ne s'arrête pas à la fin de la section sans que le bloc suivant
        // la clôture : voir ci-dessous
        if matches!(b, Block::Heading { .. }) {
            run_section = headings.len() - 1;
        }
    }
    close_run(&mut run, run_section, run_page, &mut units);

    // Une suite d'éléments de liste qui dépasse la taille d'un passage est coupée entre deux éléments ; sinon elle reste entière.
    let units: Vec<Unit> = units
        .into_iter()
        .flat_map(|u| {
            let max = if u.lines.len() > 1 { MAX_WHOLE } else { MAX_CHARS };
            split_unit(u, max)
        })
        .collect();

    // 2. passages
    let mut out: Vec<Chunk> = vec![];
    let mut cur: Option<Cur> = None;
    let flush = |cur: &mut Option<Cur>, out: &mut Vec<Chunk>, headings: &[Vec<String>]| {
        if let Some(c) = cur.take() {
            let text = c.parts.join("\n");
            if text.chars().filter(|c| c.is_alphanumeric()).count() >= 3 {
                out.push(Chunk { section: headings[c.first].clone(), merged: c.merged, text, page: c.page });
            }
        }
    };
    let mut i = 0;
    while i < units.len() {
        // un élément qui en annonce un autre est placé avec lui, s'ils tiennent ensemble
        let mut group = vec![units[i].clone()];
        while group.last().is_some_and(|u| u.glue) && i + group.len() < units.len() && group.iter().map(Unit::size).sum::<usize>() + units[i + group.len()].size() <= MAX_CHARS {
            group.push(units[i + group.len()].clone());
        }
        i += group.len();
        let size: usize = group.iter().map(|u| u.size() + 1).sum::<usize>().saturating_sub(1);
        let section = group[0].section;
        let (fits, new_section) = match &cur {
            None => (false, true),
            Some(c) => {
                let same = c.last == section;
                let fits = c.size + 1 + size <= TARGET_CHARS || (c.size < MIN_CHARS && c.size + 1 + size <= MAX_CHARS);
                // une nouvelle section ne se rattache que si la précédente est trop courte pour exister seule
                (fits && (same || c.size < MIN_CHARS), !same)
            }
        };
        if !fits {
            flush(&mut cur, &mut out, &headings);
            cur = Some(Cur { first: section, last: section, merged: vec![], parts: vec![], size: 0, page: group[0].page });
        } else if new_section {
            // section minuscule rattachée à la précédente : son titre devient une ligne du passage
            if let (Some(c), true) = (cur.as_mut(), !heading_text[section].is_empty()) {
                c.size += len(&heading_text[section]) + 1;
                c.parts.push(heading_text[section].clone());
                c.merged.push(heading_text[section].clone());
            }
        }
        let c = cur.as_mut().expect("passage en cours");
        c.last = section;
        for u in &group {
            c.size += u.size() + usize::from(c.size > 0);
            c.parts.extend(u.lines.iter().cloned());
        }
    }
    flush(&mut cur, &mut out, &headings);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(level: u8, text: &str) -> Block {
        Block::Heading { level, text: text.into() }
    }
    fn p(text: &str) -> Block {
        Block::Para(text.into())
    }
    fn item(number: Option<u32>, text: &str) -> Block {
        Block::Item { depth: 0, number, text: text.into() }
    }
    /// Un paragraphe d'environ `n` caractères fait de phrases courtes.
    fn texte(n: usize, mot: &str) -> String {
        let mut s = String::new();
        let mut i = 0;
        while s.chars().count() < n {
            i += 1;
            s.push_str(&format!("Phrase {i} sur {mot} qui continue encore un peu. "));
        }
        s.trim().to_string()
    }

    #[test]
    fn un_document_court_tient_en_un_passage() {
        let c = chunk("Guide", &[h(1, "Guide"), p("Un paragraphe."), item(Some(1), "Première étape."), item(Some(2), "Seconde étape.")]);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].text, "Un paragraphe.\n1. Première étape.\n2. Seconde étape.");
        assert!(c[0].section.is_empty(), "le titre du document n'est pas une section : {:?}", c[0].section);
        assert!(chunk("x", &[]).is_empty());
        assert!(chunk("x", &[h(1, "Seul titre")]).is_empty());
    }

    #[test]
    fn chaque_section_assez_longue_a_son_passage_et_son_chemin() {
        let blocs = vec![
            h(1, "Guide VPN"),
            h(2, "Installation"),
            p(&texte(500, "l'installation")),
            h(2, "Erreurs courantes"),
            h(3, "Erreur 809"),
            p(&texte(500, "le pare-feu")),
            h(3, "Erreur 691"),
            p(&texte(500, "le mot de passe")),
        ];
        let c = chunk("Guide VPN", &blocs);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].section, vec!["Installation"]);
        assert_eq!(c[1].section, vec!["Erreurs courantes", "Erreur 809"]);
        assert_eq!(c[2].section, vec!["Erreurs courantes", "Erreur 691"]);
        assert!(c.iter().all(|c| c.text.chars().count() <= MAX_CHARS));
    }

    #[test]
    fn des_sections_minuscules_se_rejoignent_avec_leur_titre() {
        let blocs = vec![h(1, "FAQ"), h(2, "Wi-Fi"), p("Choisir le réseau ENTREPRISE."), h(2, "Imprimante"), p("Ajouter \\\\print01\\ETAGE2."), h(2, "Messagerie"), p(&texte(700, "la messagerie"))];
        let c = chunk("FAQ", &blocs);
        // les deux premières sections sont trop courtes pour exister seules : elles se suivent dans le même passage
        assert_eq!(c[0].section, vec!["Wi-Fi"]);
        assert!(c[0].text.starts_with("Choisir le réseau ENTREPRISE.\nImprimante\nAjouter"), "{}", c[0].text);
        assert_eq!(c[0].label(), "Wi-Fi + Imprimante + Messagerie", "le chemin nomme toutes les sections du passage");
        // le reste de la section « Messagerie » continue le même passage tant qu'il tient, puis un autre passage prend le relais
        assert!(c.iter().skip(1).all(|c| c.section == vec!["Messagerie"]), "{c:?}");
    }

    #[test]
    fn la_fin_d_une_section_rejoint_son_debut_meme_apres_un_rattachement() {
        // « Intro » (court) est rattachée à « Étapes » ; le paragraphe qui suit les étapes reste dans ce passage s'il tient
        let blocs = vec![p("Introduction courte."), h(2, "Étapes"), item(Some(1), "Première étape."), item(Some(2), "Seconde étape."), p("Remarque finale sur les étapes.")];
        let c = chunk("Doc", &blocs);
        assert_eq!(c.len(), 1, "{c:?}");
        assert!(c[0].text.ends_with("Remarque finale sur les étapes."));
    }

    #[test]
    fn une_liste_d_etapes_reste_entiere_avec_sa_phrase_d_introduction() {
        let etapes: Vec<Block> = (1..=8).map(|n| item(Some(n), &format!("Étape {n} : {}", texte(60, "x")))).collect();
        let mut blocs = vec![h(1, "Doc"), p(&texte(700, "le contexte")), p("Marche à suivre :")];
        blocs.extend(etapes);
        let c = chunk("Doc", &blocs);
        let avec_etapes: Vec<&Chunk> = c.iter().filter(|c| c.text.contains("1. Étape 1")).collect();
        assert_eq!(avec_etapes.len(), 1);
        let t = &avec_etapes[0].text;
        assert!(t.contains("Marche à suivre :\n1. Étape 1") && t.contains("8. Étape 8"), "liste coupée ou séparée de son introduction : {t}");
    }

    #[test]
    fn une_liste_trop_longue_est_coupee_entre_deux_elements() {
        let etapes: Vec<Block> = (1..=30).map(|n| item(Some(n), &format!("Étape {n} : {}", texte(120, "y")))).collect();
        let c = chunk("Doc", &etapes);
        assert!(c.len() >= 3, "{}", c.len());
        for ch in &c {
            assert!(ch.text.chars().count() <= MAX_WHOLE, "{}", ch.text.chars().count());
            // chaque passage commence par un élément entier
            assert!(ch.text.starts_with(|c: char| c.is_ascii_digit()), "{}", &ch.text[..30.min(ch.text.len())]);
        }
        // rien n'est perdu ni dupliqué
        let tout: String = c.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("\n");
        for n in 1..=30 {
            assert_eq!(tout.matches(&format!("\n{n}. Étape")).count() + usize::from(tout.starts_with(&format!("{n}. Étape"))), 1, "étape {n}");
        }
    }

    #[test]
    fn un_long_paragraphe_est_coupe_aux_phrases() {
        let c = chunk("Doc", &[p(&texte(4000, "un très long paragraphe"))]);
        assert!(c.len() >= 3);
        for ch in &c {
            assert!(ch.text.chars().count() <= MAX_CHARS);
            assert!(ch.text.ends_with('.'), "coupé au milieu d'une phrase : …{}", &ch.text[ch.text.len().saturating_sub(40)..]);
        }
        // sans ponctuation du tout, coupé aux espaces
        let mots = "mot ".repeat(1000);
        let c = chunk("Doc", &[p(mots.trim())]);
        assert!(c.len() >= 3 && c.iter().all(|c| c.text.chars().count() <= MAX_CHARS && !c.text.starts_with(' ')));
    }

    #[test]
    fn un_tableau_garde_le_lien_entre_valeur_et_colonne() {
        let t = Block::Table { header: true, rows: vec![vec!["Code".into(), "Cause".into(), "Solution".into()], vec!["809".into(), "Pare-feu".into(), "Ouvrir UDP 500 et 4500".into()], vec!["691".into(), "".into(), "Vérifier le mot de passe".into()]] };
        let c = chunk("Doc", &[p("Codes d'erreur :"), t]);
        assert_eq!(c[0].text, "Codes d'erreur :\nCode : 809 ; Cause : Pare-feu ; Solution : Ouvrir UDP 500 et 4500\nCode : 691 ; Solution : Vérifier le mot de passe");
        // sans en-tête : cellules séparées par une barre
        let t = Block::Table { header: false, rows: vec![vec!["a".into(), "b".into()], vec!["c".into(), "d".into()]] };
        assert_eq!(chunk("Doc", &[t])[0].text, "a | b\nc | d");
    }

    #[test]
    fn la_page_du_debut_du_passage_est_retenue() {
        let blocs = vec![Block::Page(1), p(&texte(1000, "page un")), Block::Page(2), p(&texte(1000, "page deux"))];
        let c = chunk("Doc", &blocs);
        assert_eq!(c.first().and_then(|c| c.page), Some(1));
        assert_eq!(c.last().and_then(|c| c.page), Some(2));
    }

    #[test]
    fn un_bloc_de_code_long_est_coupe_aux_lignes() {
        let code = (1..=120).map(|i| format!("ligne {i} de la commande")).collect::<Vec<_>>().join("\n");
        let c = chunk("Doc", &[Block::Code(code)]);
        assert!(c.len() >= 3);
        assert!(c.iter().all(|c| c.text.chars().count() <= MAX_WHOLE));
        assert!(c[0].text.starts_with("ligne 1 de"));
    }

    #[test]
    fn phrases_avec_abreviations_et_initiales() {
        let s = sentences("M. Dupont a appelé le support. Il a parlé à Mme Martin. J. K. Rowling n'y est pour rien ! Vraiment ? Oui... Peut-être.");
        assert_eq!(s.len(), 6, "{s:?}");
        assert_eq!(s[0], "M. Dupont a appelé le support.");
        assert_eq!(s[1], "Il a parlé à Mme Martin.");
        assert_eq!(s[2], "J. K. Rowling n'y est pour rien !");
        assert_eq!((s[3].as_str(), s[4].as_str()), ("Vraiment ?", "Oui..."));
        assert_eq!(sentences("Version 2.1 du client. Mettre à jour"), vec!["Version 2.1 du client.", "Mettre à jour"]);
    }
}
