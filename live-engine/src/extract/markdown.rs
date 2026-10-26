//! Markdown : titres, paragraphes, listes (numérotées ou à puces, imbriquées), tableaux et blocs de code. Un en-tête YAML simple
//! (`title`, `tags`, `url`) donne le titre, les étiquettes et l'adresse d'origine du document.

use super::{clean_line, is_web_address, Block, Extracted};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

#[derive(Default)]
struct Meta {
    title: Option<String>,
    tags: Vec<String>,
    url: Option<String>,
}

/// « Titre », 'Titre' → Titre.
fn unquote(v: &str) -> &str {
    let v = v.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return v[1..v.len() - 1].trim();
        }
    }
    v
}

/// Clé d'en-tête comparable : minuscules, sans accents (« Étiquettes » = « etiquettes »).
fn meta_key(k: &str) -> String {
    crate::terms::fold(k).split_whitespace().collect::<Vec<_>>().join("-")
}

/// Sépare l'en-tête `---` … `---` du reste. Sans en-tête, ou s'il n'est pas refermé (un simple filet horizontal), tout est du texte.
fn front_matter(src: &str) -> (Meta, &str) {
    let mut meta = Meta::default();
    let Some(rest) = src.strip_prefix("---").and_then(|r| r.strip_prefix("\r\n").or_else(|| r.strip_prefix('\n'))) else {
        return (meta, src);
    };
    let mut pos = 0;
    let mut end = None;
    for line in rest.split_inclusive('\n') {
        let t = line.trim_end();
        if t == "---" || t == "..." {
            end = Some((pos, pos + line.len()));
            break;
        }
        pos += line.len();
    }
    let Some((yaml_end, body_start)) = end else { return (meta, src) };
    let mut current: Option<String> = None;
    for line in rest[..yaml_end].lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        // élément de liste sous une clé : « - vpn »
        if let (Some(key), Some(item)) = (&current, line.trim_start().strip_prefix("- ")) {
            if line.starts_with(char::is_whitespace) || line.starts_with("- ") {
                if matches!(key.as_str(), "tags" | "tag" | "etiquettes" | "mots-cles" | "keywords" | "mots-cle") {
                    meta.tags.push(unquote(item).to_string());
                }
                continue;
            }
        }
        let Some((k, v)) = line.split_once(':') else { continue };
        if k.starts_with(char::is_whitespace) {
            continue; // clé imbriquée : ignorée
        }
        let key = meta_key(k);
        let value = unquote(v);
        current = Some(key.clone());
        match key.as_str() {
            "title" | "titre" => meta.title = Some(value.to_string()),
            "url" | "lien" | "link" | "source" | "canonical" => meta.url = Some(value.to_string()),
            "tags" | "tag" | "etiquettes" | "mots-cles" | "mots-cle" | "keywords" => {
                let v = value.trim_start_matches('[').trim_end_matches(']');
                meta.tags.extend(v.split(',').map(|t| unquote(t).to_string()).filter(|t| !t.is_empty()));
            }
            _ => {}
        }
    }
    (meta, &rest[body_start..])
}

pub fn extract(src: &str) -> Extracted {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let (meta, body) = front_matter(src);
    let mut out = Extracted { title: meta.title, tags: meta.tags, url: meta.url, ..Default::default() };

    let mut text = String::new(); // texte en cours : paragraphe, titre, élément de liste ou cellule
    let mut lists: Vec<Option<u64>> = vec![]; // listes ouvertes : prochain numéro, ou `None` pour des puces
    let mut items: Vec<(u8, Option<u32>)> = vec![]; // éléments ouverts : profondeur et numéro
    let mut heading: Option<u8> = None;
    let mut code: Option<String> = None;
    let mut table: Option<(Vec<Vec<String>>, Vec<String>)> = None; // lignes terminées, ligne en cours
    let mut links: Vec<(String, usize)> = vec![]; // adresse et début du texte du lien
    let mut image = 0usize;

    let flush_item = |text: &mut String, items: &[(u8, Option<u32>)], blocks: &mut Vec<Block>| {
        let t = clean_line(text);
        if let (false, Some(&(depth, number))) = (t.is_empty(), items.last()) {
            blocks.push(Block::Item { depth, number, text: t });
        }
        text.clear();
    };

    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(body, options) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                text.clear();
                heading = Some(level as u8);
            }
            Event::End(TagEnd::Heading(_)) => {
                let t = clean_line(&text);
                if let (false, Some(level)) = (t.is_empty(), heading) {
                    out.blocks.push(Block::Heading { level, text: t });
                }
                text.clear();
                heading = None;
            }
            Event::End(TagEnd::Paragraph) => {
                if items.is_empty() && table.is_none() {
                    let t = clean_line(&text);
                    if !t.is_empty() {
                        out.blocks.push(Block::Para(t));
                    }
                    text.clear();
                } else {
                    text.push(' '); // plusieurs paragraphes dans un même élément de liste
                }
            }
            Event::Start(Tag::List(first)) => {
                if !items.is_empty() {
                    flush_item(&mut text, &items, &mut out.blocks); // l'élément parent avant ses sous-éléments
                }
                lists.push(first);
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                text.clear();
                let number = lists.last_mut().and_then(|n| n.as_mut()).map(|n| {
                    let current = *n as u32;
                    *n += 1;
                    current
                });
                items.push(((lists.len().saturating_sub(1)).min(5) as u8, number));
            }
            Event::End(TagEnd::Item) => {
                flush_item(&mut text, &items, &mut out.blocks);
                items.pop();
            }
            Event::Start(Tag::CodeBlock(_)) => code = Some(String::new()),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(c) = code.take() {
                    let c = c.trim_end().to_string();
                    if !c.trim().is_empty() {
                        out.blocks.push(Block::Code(c));
                    }
                }
            }
            Event::Start(Tag::Table(_)) => table = Some((vec![], vec![])),
            Event::Start(Tag::TableCell) => text.clear(),
            Event::End(TagEnd::TableCell) => {
                if let Some((_, row)) = table.as_mut() {
                    row.push(clean_line(&text));
                }
                text.clear();
            }
            // l'en-tête n'a pas de ligne : ses cellules sont directement dans `TableHead`
            Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                if let Some((rows, row)) = table.as_mut() {
                    rows.push(std::mem::take(row));
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some((rows, _)) = table.take() {
                    if rows.iter().flatten().any(|c| !c.is_empty()) {
                        out.blocks.push(Block::Table { header: true, rows });
                    }
                }
            }
            Event::Start(Tag::Link { dest_url, .. }) => links.push((dest_url.to_string(), text.len())),
            Event::End(TagEnd::Link) => {
                if let Some((url, start)) = links.pop() {
                    if is_web_address(&url) && text.get(start..).is_some_and(|label| label.trim() != url) {
                        text.push_str(&format!(" ({url})"));
                    }
                }
            }
            // les images n'apportent pas de texte (leur légende est celle de l'alternative, souvent vide ou le nom du fichier)
            Event::Start(Tag::Image { .. }) => image += 1,
            Event::End(TagEnd::Image) => image = image.saturating_sub(1),
            Event::Text(t) => {
                if image == 0 {
                    match code.as_mut() {
                        Some(c) => c.push_str(&t),
                        None => text.push_str(&t),
                    }
                }
            }
            Event::Code(c) => {
                if image == 0 {
                    text.push('`');
                    text.push_str(&c);
                    text.push('`');
                }
            }
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            Event::InlineHtml(h) if h.trim_start().to_lowercase().starts_with("<br") => text.push(' '),
            _ => {} // HTML brut, notes de bas de page, filets, cases à cocher…
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(src: &str) -> Vec<Block> {
        extract(src).blocks
    }

    fn item(depth: u8, number: Option<u32>, text: &str) -> Block {
        Block::Item { depth, number, text: text.into() }
    }

    #[test]
    fn en_tete_titre_etiquettes_et_adresse() {
        let e = extract("---\ntitre: \"Réinitialiser son mot de passe\"\nÉtiquettes: [mdp, compte]\nurl: https://intranet.exemple.fr/mdp\nautre: ignoré\n---\n# Titre\n");
        assert_eq!(e.title.as_deref(), Some("Réinitialiser son mot de passe"));
        assert_eq!(e.tags, vec!["mdp", "compte"]);
        assert_eq!(e.url.as_deref(), Some("https://intranet.exemple.fr/mdp"));
        assert_eq!(e.blocks, vec![Block::Heading { level: 1, text: "Titre".into() }]);
        // liste d'étiquettes sur plusieurs lignes
        let e = extract("---\ntags:\n  - vpn\n  - 'télétravail'\ntitle: VPN\n---\nTexte\n");
        assert_eq!((e.title.as_deref(), e.tags), (Some("VPN"), vec!["vpn".to_string(), "télétravail".into()]));
        // des filets horizontaux ne sont pas un en-tête
        let e = extract("---\nUn paragraphe entre deux filets\n\nsuite\n");
        assert!(e.title.is_none() && e.blocks.iter().any(|b| matches!(b, Block::Para(t) if t.contains("paragraphe"))));
    }

    #[test]
    fn titres_et_paragraphes() {
        assert_eq!(
            blocks("# Guide VPN\n\nLe VPN permet de **travailler** à *distance*.\nSuite de la phrase.\n\n## Erreur 809\n\nVoir ci-dessous.\n"),
            vec![
                Block::Heading { level: 1, text: "Guide VPN".into() },
                Block::Para("Le VPN permet de travailler à distance. Suite de la phrase.".into()),
                Block::Heading { level: 2, text: "Erreur 809".into() },
                Block::Para("Voir ci-dessous.".into()),
            ]
        );
    }

    #[test]
    fn listes_numerotees_a_puces_et_imbriquees() {
        let b = blocks("Étapes :\n\n1. Ouvrez le menu.\n2. Choisissez :\n   - Réseau\n   - VPN\n3. Validez.\n\n- A\n- B\n");
        assert_eq!(
            b,
            vec![
                Block::Para("Étapes :".into()),
                item(0, Some(1), "Ouvrez le menu."),
                item(0, Some(2), "Choisissez :"),
                item(1, None, "Réseau"),
                item(1, None, "VPN"),
                item(0, Some(3), "Validez."),
                item(0, None, "A"),
                item(0, None, "B"),
            ]
        );
        // une liste numérotée qui ne commence pas à 1
        assert_eq!(blocks("4. Quatre\n5. Cinq\n"), vec![item(0, Some(4), "Quatre"), item(0, Some(5), "Cinq")]);
        // plusieurs paragraphes dans un même élément
        assert_eq!(blocks("1. Premier paragraphe.\n\n   Deuxième paragraphe.\n2. Suite.\n"), vec![item(0, Some(1), "Premier paragraphe. Deuxième paragraphe."), item(0, Some(2), "Suite.")]);
    }

    #[test]
    fn tableaux_code_liens_et_images() {
        let b = blocks("| Code | Cause |\n|---|---|\n| 809 | Pare-feu |\n| 691 | Mot de passe |\n\n```\nipconfig /flushdns\n```\n\nVoir [le portail](https://portail.exemple.fr/aide) ou [https://x.fr](https://x.fr) et ![capture](img.png) la commande `gpupdate`.\n");
        assert_eq!(
            b,
            vec![
                Block::Table { header: true, rows: vec![vec!["Code".into(), "Cause".into()], vec!["809".into(), "Pare-feu".into()], vec!["691".into(), "Mot de passe".into()]] },
                Block::Code("ipconfig /flushdns".into()),
                Block::Para("Voir le portail (https://portail.exemple.fr/aide) ou https://x.fr et la commande `gpupdate`.".into()),
            ]
        );
    }

    #[test]
    fn html_brut_et_citations() {
        let b = blocks("<div class=\"x\">ignoré</div>\n\n> Attention : ne communiquez jamais votre mot de passe.\n\nAvant<br>après\n");
        assert_eq!(b, vec![Block::Para("Attention : ne communiquez jamais votre mot de passe.".into()), Block::Para("Avant après".into())]);
    }
}
