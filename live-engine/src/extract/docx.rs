//! Word (.docx) : une archive zip dont `word/document.xml` contient le texte. On en tire les titres (styles « Titre 1 », « Heading 2 »
//! ou niveau hiérarchique du plan), les paragraphes, les listes (numérotées ou à puces, par numérotation ou par style) et les
//! tableaux. Les modifications suivies supprimées, les en-têtes, les pieds de page et les notes sont ignorés.

use super::{clean_line, is_web_address, Block, Extracted};
use anyhow::{anyhow, bail, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::HashMap;
use std::io::{Cursor, Read};

/// Taille maximale d'une partie du document une fois décompressée (contre les archives piégées).
const MAX_PART_BYTES: u64 = 64 * 1024 * 1024;

type Archive<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

pub fn looks_like_docx(bytes: &[u8]) -> bool {
    zip::ZipArchive::new(Cursor::new(bytes)).is_ok_and(|mut z| z.by_name("word/document.xml").is_ok())
}

/// Une partie de l'archive, en texte ; `None` si elle n'existe pas.
fn part(zip: &mut Archive, name: &str) -> Result<Option<String>> {
    let Ok(file) = zip.by_name(name) else { return Ok(None) };
    let mut buf = Vec::new();
    let read = file.take(MAX_PART_BYTES + 1).read_to_end(&mut buf)?;
    if read as u64 > MAX_PART_BYTES {
        bail!("la partie « {name} » du document est trop volumineuse une fois décompressée");
    }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

fn attr(e: &BytesStart, key: &str) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.local_name().as_ref() == key).map(|a| a.value.into_owned())
}

/// Style d'un paragraphe : son nom lisible, son niveau dans le plan, sa numérotation éventuelle.
#[derive(Debug, Default, Clone)]
struct Style {
    name: String,
    based_on: Option<String>,
    outline: Option<u8>,
    num: Option<(u32, u32)>,
}

fn parse_styles(xml: &str) -> HashMap<String, Style> {
    let mut styles: HashMap<String, Style> = HashMap::new();
    let mut reader = Reader::from_str(xml);
    let (mut current, mut in_ppr): (Option<(String, Style)>, bool) = (None, false);
    let mut num_in_style: (Option<u32>, Option<u32>) = (None, None);
    loop {
        let ev = match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(ev) => ev,
        };
        let (start, empty) = match &ev {
            Event::Start(e) => (Some(e), false),
            Event::Empty(e) => (Some(e), true),
            _ => (None, false),
        };
        if let Some(e) = start {
            match e.local_name().as_ref() {
                "style" => {
                    if let Some(id) = attr(e, "styleId") {
                        current = Some((id, Style::default()));
                        num_in_style = (None, None);
                    }
                }
                "name" => {
                    if let (Some((_, s)), Some(v)) = (current.as_mut(), attr(e, "val")) {
                        s.name = v;
                    }
                }
                "basedOn" => {
                    if let (Some((_, s)), Some(v)) = (current.as_mut(), attr(e, "val")) {
                        s.based_on = Some(v);
                    }
                }
                "pPr" => in_ppr = !empty,
                "outlineLvl" if in_ppr => {
                    if let (Some((_, s)), Some(v)) = (current.as_mut(), attr(e, "val").and_then(|v| v.parse::<u8>().ok())) {
                        s.outline = Some(v + 1);
                    }
                }
                "numId" if in_ppr => num_in_style.0 = attr(e, "val").and_then(|v| v.parse().ok()),
                "ilvl" if in_ppr => num_in_style.1 = attr(e, "val").and_then(|v| v.parse().ok()),
                _ => {}
            }
        }
        let end = match &ev {
            Event::End(e) => Some(e.local_name().as_ref().to_string()),
            Event::Empty(e) => Some(e.local_name().as_ref().to_string()),
            _ => None,
        };
        match end.as_deref() {
            Some("pPr") => in_ppr = false,
            Some("style") => {
                if let Some((id, mut s)) = current.take() {
                    if let (Some(n), l) = num_in_style {
                        s.num = Some((n, l.unwrap_or(0)));
                    }
                    styles.insert(id, s);
                }
            }
            _ => {}
        }
    }
    styles
}

/// Niveau de titre et numérotation d'un style, en remontant ses styles parents.
fn resolve_style(styles: &HashMap<String, Style>, id: &str) -> Style {
    let mut out = Style::default();
    let mut next = Some(id.to_string());
    for _ in 0..8 {
        let Some(cur) = next.take().and_then(|i| styles.get(&i)) else { break };
        if out.name.is_empty() {
            out.name = cur.name.clone();
        }
        out.outline = out.outline.or(cur.outline);
        out.num = out.num.or(cur.num);
        next = cur.based_on.clone();
    }
    if out.name.is_empty() {
        out.name = id.to_string();
    }
    out
}

/// (numId, niveau) → liste numérotée ? (`false` : à puces ou sans marque)
fn parse_numbering(xml: &str) -> HashMap<(u32, u32), bool> {
    let mut abstracts: HashMap<u32, HashMap<u32, bool>> = HashMap::new();
    let mut nums: HashMap<u32, u32> = HashMap::new();
    let mut reader = Reader::from_str(xml);
    let (mut abstract_id, mut lvl, mut num_id): (Option<u32>, Option<u32>, Option<u32>) = (None, None, None);
    loop {
        let ev = match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(ev) => ev,
        };
        match &ev {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                "abstractNum" => abstract_id = attr(e, "abstractNumId").and_then(|v| v.parse().ok()),
                "lvl" => lvl = attr(e, "ilvl").and_then(|v| v.parse().ok()),
                "numFmt" => {
                    if let (Some(a), Some(l), Some(f)) = (abstract_id, lvl, attr(e, "val")) {
                        abstracts.entry(a).or_default().insert(l, !matches!(f.as_str(), "bullet" | "none"));
                    }
                }
                "num" => num_id = attr(e, "numId").and_then(|v| v.parse().ok()),
                "abstractNumId" if num_id.is_some() => {
                    if let (Some(n), Some(a)) = (num_id, attr(e, "val").and_then(|v| v.parse().ok())) {
                        nums.insert(n, a);
                    }
                }
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                "abstractNum" => abstract_id = None,
                "lvl" => lvl = None,
                "num" => num_id = None,
                _ => {}
            },
            _ => {}
        }
    }
    let mut out = HashMap::new();
    for (n, a) in nums {
        for (l, ordered) in abstracts.get(&a).into_iter().flatten() {
            out.insert((n, *l), *ordered);
        }
    }
    out
}

/// Adresses des liens du document (`r:id` → URL).
fn parse_relationships(xml: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if e.local_name().as_ref() == "Relationship" => {
                if let (Some(id), Some(target)) = (attr(&e, "Id"), attr(&e, "Target")) {
                    if attr(&e, "TargetMode").as_deref() == Some("External") {
                        out.insert(id, target.replace("&amp;", "&"));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Texte d'un élément simple (`<dc:title>…</dc:title>`) des propriétés du document.
fn core_property(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start = xml.find(&open)?;
    let after = &xml[start..];
    let body_start = after.find('>')? + 1;
    let body = &after[body_start..after.find(&format!("</{tag}>"))?.max(body_start)];
    let text = body.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'");
    Some(clean_line(&text)).filter(|t| !t.is_empty())
}

#[derive(Default)]
struct Para {
    style: Option<String>,
    outline: Option<u8>,
    num: Option<(u32, u32)>,
    text: String,
}

/// « Titre 1 », « Heading 2 », « heading3 » → 1, 2, 3. « Title » et « Titre » → 1 ; « Subtitle » et « Sous-titre » → 2.
fn heading_level(style_name: &str) -> Option<u8> {
    let n = style_name.trim().to_lowercase().replace(['-', '_'], " ");
    let n = n.trim();
    for prefix in ["heading", "titre"] {
        if let Some(rest) = n.strip_prefix(prefix) {
            let rest = rest.trim();
            if rest.is_empty() {
                return (prefix == "titre").then_some(1);
            }
            if let Ok(l) = rest.parse::<u8>() {
                return (1..=9).contains(&l).then_some(l.min(6));
            }
        }
    }
    match n {
        "title" => Some(1),
        "subtitle" | "sous titre" => Some(2),
        _ => None,
    }
}

struct Reading<'a> {
    styles: &'a HashMap<String, Style>,
    numbering: &'a HashMap<(u32, u32), bool>,
    blocks: Vec<Block>,
    title: Option<String>,
    /// Compteurs des listes numérotées en cours : (numId, niveau) → dernier numéro.
    counters: HashMap<(u32, u32), u32>,
}

impl Reading<'_> {
    /// Un paragraphe terminé hors tableau.
    fn paragraph(&mut self, p: Para) {
        let text = clean_line(&p.text);
        if text.is_empty() {
            self.counters.clear(); // un paragraphe vide interrompt la liste
            return;
        }
        let style = p.style.as_deref().map(|id| resolve_style(self.styles, id)).unwrap_or_default();
        let lower = style.name.to_lowercase();
        if let Some(level) = heading_level(&style.name).or(p.outline).or(style.outline).filter(|_| text.chars().count() <= 200) {
            self.counters.clear();
            if level == 1 && lower.starts_with("title") || lower == "titre" {
                self.title.get_or_insert_with(|| text.clone());
            }
            self.blocks.push(Block::Heading { level: level.min(6), text });
            return;
        }
        let num = p.num.or(style.num).filter(|(id, _)| *id != 0);
        let named_list = lower.starts_with("list ") || lower.starts_with("liste ") || lower.starts_with("list-");
        if num.is_some() || (named_list && !lower.contains("paragraph") && !lower.contains("paragraphe")) {
            let (id, lvl) = num.unwrap_or((0, 0));
            let ordered = match num {
                Some(key) => self.numbering.get(&key).copied().unwrap_or(false),
                None => lower.contains("number") || lower.contains("numér"),
            };
            let number = ordered.then(|| {
                self.counters.retain(|(i, l), _| *i != id || *l <= lvl); // les niveaux plus profonds repartent de 1
                let n = self.counters.entry((id, lvl)).or_insert(0);
                *n += 1;
                *n
            });
            self.blocks.push(Block::Item { depth: lvl.min(5) as u8, number, text });
            return;
        }
        self.counters.clear();
        self.blocks.push(Block::Para(text));
    }
}

pub fn extract(bytes: &[u8]) -> Result<Extracted> {
    let mut zip: Archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| anyhow!("ce fichier n'est pas un document Word (.docx) lisible"))?;
    let body = part(&mut zip, "word/document.xml")?.ok_or_else(|| anyhow!("ce fichier n'est pas un document Word : « word/document.xml » est absent"))?;
    let styles = part(&mut zip, "word/styles.xml")?.map(|x| parse_styles(&x)).unwrap_or_default();
    let numbering = part(&mut zip, "word/numbering.xml")?.map(|x| parse_numbering(&x)).unwrap_or_default();
    let links = part(&mut zip, "word/_rels/document.xml.rels")?.map(|x| parse_relationships(&x)).unwrap_or_default();
    let core = part(&mut zip, "docProps/core.xml")?.unwrap_or_default();

    let mut r = Reading { styles: &styles, numbering: &numbering, blocks: vec![], title: None, counters: HashMap::new() };
    let mut reader = Reader::from_str(&body);
    let mut para: Option<Para> = None;
    let (mut in_t, mut in_ppr, mut in_del) = (false, false, 0usize);
    let (mut tbl_depth, mut table_rows, mut row, mut cell): (usize, Vec<Vec<String>>, Vec<String>, String) = (0, vec![], vec![], String::new());
    let mut header_row = false;
    let mut link: Option<(String, usize)> = None;

    loop {
        let ev = match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(ev) => ev,
            Err(_) => break, // fin de fichier tronquée : on garde ce qui a été lu
        };
        let (start, empty) = match &ev {
            Event::Start(e) => (Some(e), false),
            Event::Empty(e) => (Some(e), true),
            _ => (None, false),
        };
        if let Some(e) = start {
            match e.local_name().as_ref() {
                "p" => para = Some(Para::default()),
                "pPr" => in_ppr = !empty,
                "pStyle" if in_ppr => {
                    if let Some(p) = para.as_mut() {
                        p.style = attr(e, "val");
                    }
                }
                "outlineLvl" if in_ppr => {
                    if let Some(p) = para.as_mut() {
                        p.outline = attr(e, "val").and_then(|v| v.parse::<u8>().ok()).filter(|l| *l < 9).map(|l| l + 1);
                    }
                }
                "numId" if in_ppr => {
                    if let Some(p) = para.as_mut() {
                        let n = attr(e, "val").and_then(|v| v.parse().ok()).unwrap_or(0);
                        p.num = Some((n, p.num.map_or(0, |(_, l)| l)));
                    }
                }
                "ilvl" if in_ppr => {
                    if let Some(p) = para.as_mut() {
                        let l = attr(e, "val").and_then(|v| v.parse().ok()).unwrap_or(0);
                        p.num = Some((p.num.map_or(0, |(n, _)| n), l));
                    }
                }
                "t" => in_t = !empty,
                "tab" | "br" | "cr" => {
                    if let Some(p) = para.as_mut() {
                        p.text.push(' ');
                    }
                }
                "noBreakHyphen" => {
                    if let Some(p) = para.as_mut() {
                        p.text.push('-');
                    }
                }
                "del" => in_del += 1,
                "hyperlink" => {
                    if let (Some(p), Some(url)) = (para.as_ref(), attr(e, "id").and_then(|id| links.get(&id)).cloned()) {
                        link = Some((url, p.text.len()));
                    }
                }
                "tbl" => {
                    tbl_depth += 1;
                    if tbl_depth == 1 {
                        (table_rows, row, cell, header_row) = (vec![], vec![], String::new(), false);
                    }
                }
                "tr" if tbl_depth == 1 => row = vec![],
                "tblHeader" if tbl_depth == 1 && table_rows.is_empty() => header_row = true,
                "tc" if tbl_depth == 1 => cell = String::new(),
                _ => {}
            }
        }
        // fin d'élément (ou élément vide)
        let end_name = match &ev {
            Event::End(e) => Some(e.local_name().as_ref().to_string()),
            Event::Empty(e) => Some(e.local_name().as_ref().to_string()),
            _ => None,
        };
        if let Some(name) = end_name {
            match name.as_str() {
                "t" => in_t = false,
                "pPr" => in_ppr = false,
                "del" => in_del = in_del.saturating_sub(1),
                "hyperlink" => {
                    if let (Some((url, start)), Some(p)) = (link.take(), para.as_mut()) {
                        if is_web_address(&url) && p.text.get(start..).is_some_and(|l| l.trim() != url) {
                            p.text.push_str(&format!(" ({url})"));
                        }
                    }
                }
                "p" => {
                    if let Some(p) = para.take() {
                        if tbl_depth > 0 {
                            if !cell.is_empty() {
                                cell.push(' ');
                            }
                            cell.push_str(&p.text);
                        } else {
                            r.paragraph(p);
                        }
                    }
                }
                "tc" if tbl_depth == 1 => row.push(clean_line(&std::mem::take(&mut cell))),
                "tr" if tbl_depth == 1 => table_rows.push(std::mem::take(&mut row)),
                "tbl" => {
                    tbl_depth = tbl_depth.saturating_sub(1);
                    if tbl_depth == 0 {
                        r.counters.clear();
                        let rows = std::mem::take(&mut table_rows);
                        if rows.iter().flatten().any(|c| !c.is_empty()) {
                            // sans marque d'en-tête, une première ligne de cellules courtes en fait office
                            let header = header_row || (rows.len() > 1 && rows[0].iter().all(|c| !c.is_empty() && c.chars().count() <= 40));
                            r.blocks.push(Block::Table { header, rows });
                        }
                    }
                }
                _ => {}
            }
        }
        // texte et références (&amp;…) dans une balise <w:t>
        match &ev {
            Event::Text(t) if in_t && in_del == 0 => {
                if let Some(p) = para.as_mut() {
                    p.text.push_str(t);
                }
            }
            Event::GeneralRef(g) if in_t && in_del == 0 => {
                let c = match g.resolve_char_ref() {
                    Ok(Some(c)) => Some(c),
                    _ => match &**g {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => None,
                    },
                };
                if let (Some(c), Some(p)) = (c, para.as_mut()) {
                    p.text.push(c);
                }
            }
            _ => {}
        }
    }

    let mut out = Extracted { blocks: r.blocks, ..Default::default() };
    out.title = core_property(&core, "dc:title").or(r.title);
    if let Some(k) = core_property(&core, "cp:keywords") {
        out.tags = k.split([',', ';']).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
    }
    if !out.has_text() {
        out.warnings.push("le document Word ne contient aucun texte".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Un .docx minimal fabriqué à la main : (nom de la partie, contenu).
    fn docx(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            for (name, content) in parts {
                z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
                z.write_all(content.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    const NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";

    fn p(style: Option<&str>, text: &str) -> String {
        let ppr = style.map(|s| format!("<w:pPr><w:pStyle w:val=\"{s}\"/></w:pPr>")).unwrap_or_default();
        format!("<w:p>{ppr}<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>")
    }

    fn numbered(num_id: u32, lvl: u32, text: &str) -> String {
        format!("<w:p><w:pPr><w:pStyle w:val=\"Paragraphedeliste\"/><w:numPr><w:ilvl w:val=\"{lvl}\"/><w:numId w:val=\"{num_id}\"/></w:numPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>")
    }

    const STYLES: &str = "<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
        <w:style w:type=\"paragraph\" w:styleId=\"Titre1\"><w:name w:val=\"heading 1\"/></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Titre2\"><w:name w:val=\"heading 2\"/></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Titredudocument\"><w:name w:val=\"Title\"/></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Paragraphedeliste\"><w:name w:val=\"List Paragraph\"/></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Puces\"><w:name w:val=\"List Bullet\"/></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Plan\"><w:name w:val=\"Corps\"/><w:pPr><w:outlineLvl w:val=\"2\"/></w:pPr></w:style>\
        <w:style w:type=\"paragraph\" w:styleId=\"Plan2\"><w:name w:val=\"Corps 2\"/><w:basedOn w:val=\"Plan\"/></w:style>\
        </w:styles>";

    const NUMBERING: &str = "<w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
        <w:abstractNum w:abstractNumId=\"0\"><w:lvl w:ilvl=\"0\"><w:numFmt w:val=\"decimal\"/></w:lvl><w:lvl w:ilvl=\"1\"><w:numFmt w:val=\"lowerLetter\"/></w:lvl></w:abstractNum>\
        <w:abstractNum w:abstractNumId=\"1\"><w:lvl w:ilvl=\"0\"><w:numFmt w:val=\"bullet\"/></w:lvl></w:abstractNum>\
        <w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num><w:num w:numId=\"2\"><w:abstractNumId w:val=\"1\"/></w:num></w:numbering>";

    fn document(body: &str) -> Vec<u8> {
        let doc = format!("<?xml version=\"1.0\"?><w:document {NS}><w:body>{body}</w:body></w:document>");
        docx(&[
            ("word/document.xml", &doc),
            ("word/styles.xml", STYLES),
            ("word/numbering.xml", NUMBERING),
            ("word/_rels/document.xml.rels", "<Relationships><Relationship Id=\"rId9\" Target=\"https://portail.exemple.fr/aide?a=1&amp;b=2\" TargetMode=\"External\"/></Relationships>"),
            ("docProps/core.xml", "<cp:coreProperties><dc:title>Guide du VPN</dc:title><cp:keywords>vpn; télétravail, accès</cp:keywords></cp:coreProperties>"),
        ])
    }

    fn item(depth: u8, number: Option<u32>, text: &str) -> Block {
        Block::Item { depth, number, text: text.into() }
    }

    #[test]
    fn titres_paragraphes_et_proprietes() {
        let body = [p(Some("Titredudocument"), "Guide VPN"), p(Some("Titre1"), "Installation"), p(None, "Le client &amp; le certificat &#233;taient fournis."), p(Some("Titre2"), "Erreur 809"), p(Some("Plan2"), "Un plan hérité")].concat();
        let e = extract(&document(&body)).unwrap();
        assert_eq!(e.title.as_deref(), Some("Guide du VPN"), "le titre des propriétés l'emporte");
        assert_eq!(e.tags, vec!["vpn", "télétravail", "accès"]);
        assert_eq!(
            e.blocks,
            vec![
                Block::Heading { level: 1, text: "Guide VPN".into() },
                Block::Heading { level: 1, text: "Installation".into() },
                Block::Para("Le client & le certificat étaient fournis.".into()),
                Block::Heading { level: 2, text: "Erreur 809".into() },
                Block::Heading { level: 3, text: "Un plan hérité".into() },
            ]
        );
    }

    #[test]
    fn listes_numerotees_a_puces_et_imbriquees() {
        let body = [
            p(None, "Marche à suivre :"),
            numbered(1, 0, "Ouvrez le menu."),
            numbered(1, 0, "Choisissez :"),
            numbered(1, 1, "VPN"),
            numbered(1, 1, "Réseau"),
            numbered(1, 0, "Validez."),
            p(None, "Puis :"),
            numbered(2, 0, "un point"),
            p(Some("Puces"), "une puce de style"),
            numbered(1, 0, "Nouvelle liste"),
        ]
        .concat();
        let e = extract(&document(&body)).unwrap();
        assert_eq!(
            e.blocks,
            vec![
                Block::Para("Marche à suivre :".into()),
                item(0, Some(1), "Ouvrez le menu."),
                item(0, Some(2), "Choisissez :"),
                item(1, Some(1), "VPN"),
                item(1, Some(2), "Réseau"),
                item(0, Some(3), "Validez."),
                Block::Para("Puis :".into()),
                item(0, None, "un point"),
                item(0, None, "une puce de style"),
                item(0, Some(1), "Nouvelle liste"), // un paragraphe ordinaire a interrompu la liste : la numérotation repart de 1
            ]
        );
    }

    #[test]
    fn tableaux_liens_et_texte_supprime() {
        let table = "<w:tbl><w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:p><w:r><w:t>Code</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Cause</w:t></w:r></w:p></w:tc></w:tr>\
            <w:tr><w:tc><w:p><w:r><w:t>809</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Pare-feu</w:t></w:r></w:p><w:p><w:r><w:t>bloqué</w:t></w:r></w:p></w:tc></w:tr></w:tbl>";
        let link = "<w:p><w:r><w:t>Voir </w:t></w:r><w:hyperlink r:id=\"rId9\"><w:r><w:t>le portail</w:t></w:r></w:hyperlink><w:del><w:r><w:delText>supprimé</w:delText></w:r></w:del><w:r><w:tab/><w:t>fin</w:t></w:r></w:p>";
        let e = extract(&document(&format!("{table}{link}"))).unwrap();
        assert_eq!(
            e.blocks,
            vec![
                Block::Table { header: true, rows: vec![vec!["Code".into(), "Cause".into()], vec!["809".into(), "Pare-feu bloqué".into()]] },
                Block::Para("Voir le portail (https://portail.exemple.fr/aide?a=1&b=2) fin".into()),
            ]
        );
    }

    #[test]
    fn document_vide_illisible_ou_piege() {
        let vide = extract(&document("")).unwrap();
        assert!(!vide.has_text() && vide.warnings.iter().any(|w| w.contains("aucun texte")));
        assert!(extract(b"PK\x03\x04 pas une archive").unwrap_err().to_string().contains("pas un document Word"));
        let sans_corps = docx(&[("autre.xml", "<a/>")]);
        assert!(extract(&sans_corps).unwrap_err().to_string().contains("word/document.xml"));
        assert!(looks_like_docx(&document("")) && !looks_like_docx(&sans_corps) && !looks_like_docx(b"texte"));
        // XML tronqué : on garde ce qui a été lu
        let tronque = docx(&[("word/document.xml", &format!("<w:document {NS}><w:body>{}<w:p><w:r><w:t>coupé", p(None, "Début complet.")))]);
        assert_eq!(extract(&tronque).unwrap().blocks, vec![Block::Para("Début complet.".into())]);
    }

    #[test]
    fn niveaux_de_titre_des_noms_de_style() {
        for (name, level) in [("heading 1", Some(1)), ("Heading3", Some(3)), ("Titre 2", Some(2)), ("titre4", Some(4)), ("Title", Some(1)), ("Titre", Some(1)), ("Subtitle", Some(2)), ("Sous-titre", Some(2)), ("Normal", None), ("Heading", None), ("Titre de tableau", None)] {
            assert_eq!(heading_level(name), level, "{name}");
        }
    }
}
