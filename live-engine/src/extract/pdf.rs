//! PDF : le texte est extrait page par page (une page illisible n'empêche pas de lire les autres), les en-têtes et pieds de page
//! qui se répètent sont retirés, puis les lignes sont remises en titres, paragraphes et listes comme pour un texte brut. Un PDF
//! scanné (une image par page) n'a pas de texte : la reconnaissance de caractères n'est pas prise en charge.

use super::{text, Block, Extracted};
use anyhow::{anyhow, bail, Result};
use pdf_extract::{output_doc_page, PlainTextOutput};
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Pages lues au plus : au-delà, c'est un livre, pas une fiche d'aide.
const MAX_PAGES: usize = 1500;

/// La forme d'une ligne répétée d'une page à l'autre : « Page 3 sur 12 » et « Page 4 sur 12 » ont la même.
fn shape(line: &str) -> String {
    let digits_masked: String = line.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
    digits_masked.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Retire les en-têtes et pieds de page : une ligne courte, en haut ou en bas de la page, qui revient sur la plupart des pages.
fn strip_running_lines(pages: &mut [(u32, Vec<String>)]) {
    if pages.len() < 3 {
        return;
    }
    // positions (dans la page) des trois premières et des trois dernières lignes non vides
    let edges = |lines: &[String]| -> Vec<usize> {
        let filled: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| !l.trim().is_empty()).map(|(i, _)| i).collect();
        let mut e: Vec<usize> = filled.iter().take(3).chain(filled.iter().rev().take(3)).copied().collect();
        e.sort_unstable();
        e.dedup();
        e
    };
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, lines) in pages.iter() {
        let shapes: HashSet<String> = edges(lines).into_iter().map(|i| shape(&lines[i])).collect();
        for s in shapes {
            *counts.entry(s).or_default() += 1;
        }
    }
    let threshold = (pages.len() * 6 / 10).max(3);
    let running: HashSet<String> = counts.into_iter().filter(|(s, n)| *n >= threshold && !s.is_empty() && s.chars().count() <= 100).map(|(s, _)| s).collect();
    if running.is_empty() {
        return;
    }
    for (_, lines) in pages.iter_mut() {
        let drop: HashSet<usize> = edges(lines).into_iter().filter(|i| running.contains(&shape(&lines[*i]))).collect();
        *lines = lines.iter().enumerate().filter(|(i, _)| !drop.contains(i)).map(|(_, l)| l.clone()).collect();
    }
}

pub fn extract(bytes: &[u8]) -> Result<Extracted> {
    // Un fichier piégé ou malformé peut faire échouer la bibliothèque par un arrêt brutal : on en fait une erreur lisible.
    catch_unwind(AssertUnwindSafe(|| read(bytes))).map_err(|_| anyhow!("ce PDF n'a pas pu être lu (fichier endommagé ou format non pris en charge)"))?
}

fn read(bytes: &[u8]) -> Result<Extracted> {
    let mut doc = lopdf::Document::load_mem(bytes).map_err(|e| anyhow!("ce PDF est illisible ({e})"))?;
    if doc.is_encrypted() && doc.decrypt("").is_err() {
        bail!("ce PDF est protégé par un mot de passe : retirez la protection avant de l'importer");
    }
    let numbers: Vec<u32> = doc.get_pages().keys().copied().collect();
    if numbers.is_empty() {
        bail!("ce PDF ne contient aucune page");
    }
    let mut out = Extracted::default();
    if numbers.len() > MAX_PAGES {
        out.warnings.push(format!("seules les {MAX_PAGES} premières pages sur {} sont lues", numbers.len()));
    }

    let mut pages: Vec<(u32, Vec<String>)> = vec![];
    let mut unreadable: Vec<u32> = vec![];
    for &n in numbers.iter().take(MAX_PAGES) {
        let mut raw = String::new();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let mut output = PlainTextOutput::new(&mut raw);
            output_doc_page(&doc, &mut output, n)
        }));
        match outcome {
            Ok(Ok(())) => pages.push((n, raw.replace('\r', "").lines().map(str::to_string).collect())),
            _ => unreadable.push(n),
        }
    }
    if !unreadable.is_empty() {
        let list: Vec<String> = unreadable.iter().take(10).map(u32::to_string).collect();
        out.warnings.push(format!("{} page(s) n'ont pas pu être lues : {}{}", unreadable.len(), list.join(", "), if unreadable.len() > 10 { "…" } else { "" }));
    }
    strip_running_lines(&mut pages);

    for (n, lines) in &pages {
        let blocks = text::blocks_from_lines(&lines.join("\n"));
        if !blocks.is_empty() {
            out.blocks.push(Block::Page(*n));
            out.blocks.extend(blocks);
        }
    }
    out.title = text::promote_title(&mut out.blocks);
    if !out.has_text() {
        out.warnings.push("aucun texte dans ce PDF : s'il s'agit d'un document scanné (une image par page), la reconnaissance de caractères n'est pas prise en charge".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TELETRAVAIL: &[u8] = include_bytes!("../../../eval/corpus/04-teletravail-bonnes-pratiques.pdf");

    #[test]
    fn un_pdf_de_word_redevient_titres_listes_et_paragraphes() {
        let e = extract(TELETRAVAIL).unwrap();
        assert_eq!(e.title.as_deref(), Some("Télétravail : la check-list du poste nomade"));
        assert!(e.warnings.is_empty(), "{:?}", e.warnings);
        assert!(matches!(e.blocks[0], Block::Page(1)));
        let titres: Vec<&str> = e.blocks.iter().filter_map(|b| if let Block::Heading { text, .. } = b { Some(text.as_str()) } else { None }).collect();
        assert_eq!(titres, vec!["Télétravail : la check-list du poste nomade", "Le matériel", "La connexion", "La sécurité du poste", "Les fichiers", "Le support à distance"]);
        // les puces sont reconnues malgré le caractère de la police de symboles
        let puces: Vec<&str> = e.blocks.iter().filter_map(|b| if let Block::Item { number: None, text, .. } = b { Some(text.as_str()) } else { None }).collect();
        assert_eq!(puces.len(), 7, "{puces:?}");
        assert!(puces[0].starts_with("Connectez-vous à un Wi-Fi domestique"), "{puces:?}");
        // les lignes coupées par la mise en page forment un seul paragraphe
        assert!(e.blocks.iter().any(|b| matches!(b, Block::Para(t) if t.contains("poste 4242. En dehors des horaires, envoyez un courriel à support@exemple.fr"))), "{:#?}", e.blocks);
    }

    #[test]
    fn des_octets_quelconques_ne_font_pas_un_pdf() {
        assert!(extract(b"%PDF-1.4 pas vraiment un pdf").is_err());
        assert!(extract(b"n'importe quoi").is_err());
        assert!(extract(b"").is_err());
    }

    #[test]
    fn les_en_tetes_et_pieds_de_page_repetes_sont_retires() {
        let page = |n: u32, corps: &str| (n, format!("Confidentiel - DSI\n{corps}\nPage {n} sur 4").lines().map(str::to_string).collect::<Vec<_>>());
        let mut pages = vec![page(1, "Texte un."), page(2, "Texte deux."), page(3, "Texte trois."), page(4, "Texte quatre.")];
        strip_running_lines(&mut pages);
        for (i, (_, lines)) in pages.iter().enumerate() {
            assert_eq!(lines.len(), 1, "page {}: {lines:?}", i + 1);
        }
        // avec deux pages seulement, rien n'est retiré (on ne peut pas savoir ce qui se répète)
        let mut deux = vec![page(1, "A"), page(2, "B")];
        strip_running_lines(&mut deux);
        assert_eq!(deux[0].1.len(), 3);
        // une ligne qui revient au milieu de la page n'est pas un en-tête
        let mots = ["un", "deux", "trois", "quatre"];
        let mut milieu: Vec<(u32, Vec<String>)> = (0..4)
            .map(|n| (n as u32 + 1, ["a", "b", "c", "d"].iter().map(|x| format!("{} {x}", mots[n])).chain(["Répété".to_string()]).chain(["e", "f", "g", "h"].iter().map(|x| format!("{} {x}", mots[n]))).collect()))
            .collect();
        strip_running_lines(&mut milieu);
        assert_eq!(milieu[0].1.len(), 9);
    }

    #[test]
    fn la_forme_d_une_ligne_ignore_les_chiffres_et_la_casse() {
        assert_eq!(shape("Page 3 sur 12"), shape("PAGE 4  sur 99"));
        assert_ne!(shape("Page 3 sur 12"), shape("Annexe 3"));
    }
}
