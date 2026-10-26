//! Texte brut : décodage (UTF-8, UTF-16, Windows-1252 des vieux exports) et découpage en blocs. Ces règles servent aussi aux PDF,
//! dont l'extraction ne rend que des lignes de texte.

use super::{clean_line, Block, Extracted};

/// Décode un fichier texte : BOM UTF-8 ou UTF-16 reconnus ; sinon UTF-8, et à défaut Windows-1252 (les exports de vieux logiciels
/// français), jamais une erreur.
pub fn decode(bytes: &[u8]) -> String {
    let (decoded, _) = if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        encoding_rs::UTF_16LE.decode_without_bom_handling(rest)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        encoding_rs::UTF_16BE.decode_without_bom_handling(rest)
    } else {
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
        match std::str::from_utf8(bytes) {
            Ok(s) => return s.to_string(),
            Err(_) => encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes),
        }
    };
    decoded.into_owned()
}

pub fn extract(text: &str) -> Extracted {
    let mut blocks = blocks_from_lines(text);
    let title = promote_title(&mut blocks);
    Extracted { title, blocks, ..Default::default() }
}

/// La première ligne d'un texte ou d'un PDF, si elle ressemble à un titre (courte, sans point final, en capitales ou suivie
/// d'un texte), en est le titre : le bloc devient un titre de niveau 1.
pub(super) fn promote_title(blocks: &mut [Block]) -> Option<String> {
    let i = blocks.iter().position(|b| !matches!(b, Block::Page(_)))?;
    let followed = blocks.get(i + 1).is_some_and(|b| matches!(b, Block::Para(_) | Block::Heading { .. } | Block::Item { .. }));
    let title = match &blocks[i] {
        Block::Para(t) if followed && t.chars().count() <= 90 && !t.ends_with(['.', ':', ';', ',', '?', '!']) => t.clone(),
        // un titre en capitales (« MON POSTE EST LENT : QUE FAIRE ? ») est déjà reconnu comme titre de section
        Block::Heading { text, .. } if text.chars().any(char::is_alphabetic) && !text.chars().any(char::is_lowercase) => text.clone(),
        _ => return None,
    };
    blocks[i] = Block::Heading { level: 1, text: title.clone() };
    Some(title)
}

const BULLETS: [char; 9] = ['-', '*', '•', '–', '●', '▪', '◦', '‣', '○'];

/// « 3. Cliquez », « 12) Fermez » → (3, « Cliquez »). Trois chiffres au plus : « 2024. Année record » n'est pas une étape, et
/// « 2.1 Titre » (pas d'espace après le point) non plus.
fn numbered(line: &str) -> Option<(u32, &str)> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if !(1..=3).contains(&digits) {
        return None;
    }
    let rest = &line[digits..];
    let rest = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'))?;
    let first = rest.chars().next()?;
    if !first.is_whitespace() {
        return None;
    }
    let text = rest.trim_start();
    (!text.is_empty()).then(|| (line[..digits].parse().unwrap_or(0), text))
}

fn bulleted(line: &str) -> Option<&str> {
    let mut cs = line.chars();
    let first = cs.next()?;
    // les puces des polices de symboles (Symbol, Wingdings) sortent des PDF en caractères d'usage privé
    if !BULLETS.contains(&first) && !('\u{e000}'..='\u{f8ff}').contains(&first) {
        return None;
    }
    let rest = cs.as_str();
    // « -- » et « ** » ne sont pas des puces ; une puce est suivie d'une espace
    (rest.starts_with(char::is_whitespace) && !rest.trim().is_empty()).then(|| rest.trim_start())
}

/// « 2.1 Réinitialisation », « Chapitre 3 », « ÉTAPES À SUIVRE » : un titre qu'aucune mise en forme ne signale (texte brut, PDF).
/// Le niveau vient du nombre de numéros (« 2.1.3 » : niveau 3) ; un titre en capitales est de niveau 2.
fn loose_heading(line: &str) -> Option<(u8, String)> {
    let len = line.chars().count();
    if !(3..=90).contains(&len) || line.ends_with(['.', ';', ',', ':']) {
        return None;
    }
    // numérotation à plusieurs niveaux : « 2.1 Titre » (un seul niveau est ambigu avec une étape)
    let num: String = line.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let rest = line[num.len()..].trim_start();
    let num = num.trim_end_matches('.');
    if num.contains('.')
        && num.split('.').all(|p| !p.is_empty() && p.len() <= 3)
        && line[num.len()..].starts_with([' ', '.'])
        && rest.chars().next().is_some_and(char::is_alphabetic)
    {
        return Some(((num.split('.').count() as u8).min(6), rest.to_string()));
    }
    // « Chapitre 3 », « Partie 2 : Réseau », « Annexe A »
    let lower = line.to_lowercase();
    for word in ["chapitre", "section", "partie", "annexe"] {
        if let Some(after) = lower.strip_prefix(word) {
            if after.starts_with(' ') && after.trim().chars().next().is_some_and(|c| c.is_alphanumeric()) && len <= 60 {
                return Some((1, line.to_string()));
            }
        }
    }
    // tout en capitales
    let letters: Vec<char> = line.chars().filter(|c| c.is_alphabetic()).collect();
    let upper = letters.iter().filter(|c| c.is_uppercase()).count();
    if letters.len() >= 4 && upper * 100 >= letters.len() * 92 && len <= 70 && line.split_whitespace().any(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 3) {
        return Some((2, line.to_string()));
    }
    None
}

fn is_underline(line: &str, ch: char) -> bool {
    let t = line.trim();
    t.chars().count() >= 3 && t.chars().all(|c| c == ch)
}

/// Découpe un texte en blocs : lignes consécutives réunies en paragraphes (les retours à la ligne d'un PDF ou d'un fichier mis en
/// page à 80 colonnes ne séparent rien), lignes vides entre paragraphes, étapes numérotées et puces, titres soulignés ou
/// numérotés.
pub(super) fn blocks_from_lines(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks: Vec<Block> = vec![];
    let mut para: Vec<String> = vec![];
    let mut item: Option<(u8, Option<u32>, String)> = None;

    fn flush_para(para: &mut Vec<String>, blocks: &mut Vec<Block>) {
        if !para.is_empty() {
            let t = clean_line(&para.join(" "));
            if !t.is_empty() {
                blocks.push(Block::Para(t));
            }
            para.clear();
        }
    }
    fn flush_item(item: &mut Option<(u8, Option<u32>, String)>, blocks: &mut Vec<Block>) {
        if let Some((depth, number, text)) = item.take() {
            let t = clean_line(&text);
            if !t.is_empty() {
                blocks.push(Block::Item { depth, number, text: t });
            }
        }
    }

    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let line = raw.trim();
        i += 1;
        if line.is_empty() {
            flush_para(&mut para, &mut blocks);
            flush_item(&mut item, &mut blocks);
            continue;
        }
        // titre souligné par des « === » ou des « --- »
        if let Some(next) = lines.get(i) {
            let underline = if is_underline(next, '=') { Some(1) } else if is_underline(next, '-') { Some(2) } else { None };
            if let Some(level) = underline.filter(|_| para.is_empty() && item.is_none() && line.chars().count() <= 100) {
                flush_item(&mut item, &mut blocks);
                blocks.push(Block::Heading { level, text: clean_line(line) });
                i += 1;
                continue;
            }
        }
        let indent = raw.len() - raw.trim_start().len();
        if let Some((n, rest)) = numbered(line) {
            // « 1. Installation », court, sans ponctuation finale, seul en début de bloc et que ne suit pas « 2. … » : un titre
            let next_number = lines[i..].iter().map(|l| l.trim()).find(|l| !l.is_empty()).and_then(numbered).map(|(m, _)| m);
            if para.is_empty() && item.is_none() && n <= 30 && rest.chars().count() <= 60 && !rest.ends_with(['.', ';', ':', ',', '?', '!']) && next_number != Some(n + 1) {
                blocks.push(Block::Heading { level: 1, text: clean_line(rest) });
                continue;
            }
            flush_para(&mut para, &mut blocks);
            flush_item(&mut item, &mut blocks);
            item = Some(((indent / 4).min(3) as u8, Some(n), rest.to_string()));
            continue;
        }
        if let Some(rest) = bulleted(line) {
            flush_para(&mut para, &mut blocks);
            flush_item(&mut item, &mut blocks);
            item = Some(((indent / 4).min(3) as u8, None, rest.to_string()));
            continue;
        }
        if para.is_empty() {
            if let Some((level, text)) = loose_heading(line) {
                flush_item(&mut item, &mut blocks);
                blocks.push(Block::Heading { level, text: clean_line(&text) });
                continue;
            }
        }
        // suite de la ligne précédente : l'élément de liste en cours, sinon le paragraphe
        if let Some((_, _, text)) = item.as_mut() {
            text.push(' ');
            text.push_str(line);
        } else {
            para.push(line.to_string());
        }
    }
    flush_para(&mut para, &mut blocks);
    flush_item(&mut item, &mut blocks);
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(t: &str) -> Block {
        Block::Para(t.into())
    }

    #[test]
    fn decodage_utf8_utf16_et_windows_1252() {
        assert_eq!(decode("Café à l'œuf".as_bytes()), "Café à l'œuf");
        assert_eq!(decode(b"\xEF\xBB\xBFBonjour"), "Bonjour", "BOM UTF-8 retiré");
        // Windows-1252 : « é » = 0xE9, « € » = 0x80, « œ » = 0x9C
        assert_eq!(decode(b"caf\xE9 \x80 \x9C"), "café € œ");
        // UTF-16 avec BOM
        assert_eq!(decode(&[0xFF, 0xFE, b'H', 0, 0xE9, 0]), "Hé");
        assert_eq!(decode(&[0xFE, 0xFF, 0, b'H', 0, 0xE9]), "Hé");
        assert_eq!(decode(b""), "");
    }

    #[test]
    fn paragraphes_et_lignes_coupees() {
        let b = blocks_from_lines("Le poste ne démarre plus\net affiche un écran noir.\n\nAppelez le support.\n");
        assert_eq!(b, vec![para("Le poste ne démarre plus et affiche un écran noir."), para("Appelez le support.")]);
        assert!(blocks_from_lines("  \n\n\t\n").is_empty());
    }

    #[test]
    fn etapes_numerotees_et_puces_avec_suite_de_ligne() {
        let b = blocks_from_lines("Marche à suivre :\n1. Ouvrez le menu Démarrer\n   puis tapez « cmd ».\n2) Validez.\n\n- un point\n• un autre\n");
        assert_eq!(
            b,
            vec![
                para("Marche à suivre :"),
                Block::Item { depth: 0, number: Some(1), text: "Ouvrez le menu Démarrer puis tapez « cmd ».".into() },
                Block::Item { depth: 0, number: Some(2), text: "Validez.".into() },
                Block::Item { depth: 0, number: None, text: "un point".into() },
                Block::Item { depth: 0, number: None, text: "un autre".into() },
            ]
        );
    }

    #[test]
    fn un_titre_numerote_se_distingue_d_une_liste_d_etapes() {
        // « 1. Installation » suivi d'un paragraphe : un titre ; les étapes, elles, se suivent
        let b = blocks_from_lines("1. Installation\nLe client est dans le Centre logiciel.\n\n1. Ouvrez le menu\n2. Cliquez sur OK\n\n2. Erreurs courantes\nVoir plus bas.\n");
        assert_eq!(
            b,
            vec![
                Block::Heading { level: 1, text: "Installation".into() },
                para("Le client est dans le Centre logiciel."),
                Block::Item { depth: 0, number: Some(1), text: "Ouvrez le menu".into() },
                Block::Item { depth: 0, number: Some(2), text: "Cliquez sur OK".into() },
                Block::Heading { level: 1, text: "Erreurs courantes".into() },
                para("Voir plus bas."),
            ]
        );
        // une table des matières reste une liste ; une étape qui se termine par un point aussi
        let b = blocks_from_lines("1. Installation\n2. Erreurs\n3. Bonnes pratiques\n");
        assert!(b.iter().all(|x| matches!(x, Block::Item { .. })), "{b:?}");
        assert_eq!(blocks_from_lines("1. Redémarrez.\nPuis attendez."), vec![Block::Item { depth: 0, number: Some(1), text: "Redémarrez. Puis attendez.".into() }]);
    }

    #[test]
    fn puces_des_polices_de_symboles() {
        assert_eq!(bulleted("\u{f0b7} Ne communiquez jamais votre mot de passe."), Some("Ne communiquez jamais votre mot de passe."));
        let b = blocks_from_lines("\u{f0b7} Premier\n\u{f0b7} Second\n");
        assert_eq!(b, vec![Block::Item { depth: 0, number: None, text: "Premier".into() }, Block::Item { depth: 0, number: None, text: "Second".into() }]);
    }

    #[test]
    fn la_premiere_ligne_devient_le_titre_du_document() {
        let e = extract("MON POSTE EST LENT : QUE FAIRE ?\n\nCette fiche explique quoi vérifier.\n\n1. Redémarrer\nTexte.\n");
        assert_eq!(e.title.as_deref(), Some("MON POSTE EST LENT : QUE FAIRE ?"));
        assert_eq!(e.blocks[0], Block::Heading { level: 1, text: "MON POSTE EST LENT : QUE FAIRE ?".into() });
        // une courte ligne suivie du texte
        assert_eq!(extract("Guide du client\n\nUn paragraphe.").title.as_deref(), Some("Guide du client"));
        // pas de titre : une phrase, un seul bloc, une étape numérotée, un titre en minuscules et majuscules mêlées
        assert_eq!(extract("Redémarrez le poste.\n\nPuis attendez.").title, None);
        assert_eq!(extract("Seulement une ligne").title, None);
        assert_eq!(extract("1. Installation\nTexte.").title, None);
        assert_eq!(extract("Chapitre 3\nTexte.").title, None);
    }

    #[test]
    fn une_annee_ou_un_titre_numerote_n_est_pas_une_etape() {
        assert_eq!(numbered("3. Cliquez"), Some((3, "Cliquez")));
        assert_eq!(numbered("12) Fermez"), Some((12, "Fermez")));
        assert_eq!(numbered("2024. Année record"), None);
        assert_eq!(numbered("2.1 Titre"), None);
        assert_eq!(numbered("3."), None);
        assert_eq!(bulleted("- texte"), Some("texte"));
        assert_eq!(bulleted("-- texte"), None);
        assert_eq!(bulleted("-texte"), None);
    }

    #[test]
    fn titres_soulignes_numerotes_et_en_capitales() {
        let b = blocks_from_lines("Guide VPN\n=========\nIntroduction.\n\nInstallation\n------------\nÉtape.\n\n2.1 Erreur 809\nCause : pare-feu.\n\nSOLUTIONS POSSIBLES\nAutoriser le port.\n");
        assert_eq!(
            b,
            vec![
                Block::Heading { level: 1, text: "Guide VPN".into() },
                para("Introduction."),
                Block::Heading { level: 2, text: "Installation".into() },
                para("Étape."),
                Block::Heading { level: 2, text: "Erreur 809".into() },
                para("Cause : pare-feu."),
                Block::Heading { level: 2, text: "SOLUTIONS POSSIBLES".into() },
                para("Autoriser le port."),
            ]
        );
        // une phrase en capitales qui se termine par un point, ou un sigle seul, n'est pas un titre
        assert!(loose_heading("ATTENTION : NE PAS ÉTEINDRE.").is_none());
        assert!(loose_heading("VPN").is_none());
        assert_eq!(loose_heading("Chapitre 3"), Some((1, "Chapitre 3".into())));
        assert_eq!(loose_heading("2.1.3 Détails").map(|(l, _)| l), Some(3));
        assert!(loose_heading("Le port 8.8 est ouvert").is_none());
    }

    #[test]
    fn les_puces_indentees_ont_une_profondeur() {
        let b = blocks_from_lines("- parent\n    - enfant\n");
        assert_eq!(
            b,
            vec![
                Block::Item { depth: 0, number: None, text: "parent".into() },
                Block::Item { depth: 1, number: None, text: "enfant".into() },
            ]
        );
    }
}
