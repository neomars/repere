//! Lecture des documents de la base de connaissances (Markdown, HTML, Word, PDF, texte) : chaque format est ramené à la même suite
//! de blocs (titres, paragraphes, listes, tableaux), que `chunk` découpe ensuite en passages. Rien n'est exécuté ni téléchargé :
//! on ne lit que le texte du fichier (scripts, styles, images et liens extérieurs sont ignorés).

mod chunk;
mod docx;
mod html;
mod markdown;
mod pdf;
mod text;

pub use chunk::chunk;

use anyhow::{bail, Result};

/// Taille maximale d'un document lu (un PDF scanné de plusieurs centaines de pages dépasse déjà largement ce que la base utilise).
pub const MAX_DOCUMENT_BYTES: usize = 50 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Html,
    Docx,
    Pdf,
    Text,
}

impl Format {
    /// Le format d'après le contenu (signature du fichier) puis le nom : un `.docx` renommé en `.zip` reste un document Word,
    /// une page HTML enregistrée sans extension reste une page HTML. `None` : format non pris en charge.
    pub fn detect(name: &str, bytes: &[u8]) -> Option<Format> {
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
        if bytes.starts_with(b"%PDF-") {
            return Some(Format::Pdf);
        }
        if bytes.starts_with(b"PK\x03\x04") {
            return (ext == "docx" || ext == "docm" || ext == "dotx" || ext == "zip").then_some(Format::Docx).filter(|_| docx::looks_like_docx(bytes));
        }
        match ext.as_str() {
            "md" | "markdown" | "mdown" => Some(Format::Markdown),
            "html" | "htm" | "xhtml" => Some(Format::Html),
            "txt" | "text" | "log" => Some(Format::Text),
            "docx" | "pdf" => None, // l'extension le promet, le contenu non : ni Word ni PDF
            _ => {
                let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]).to_lowercase();
                (head.contains("<!doctype html") || head.contains("<html")).then_some(Format::Html)
            }
        }
    }

    /// Valeur enregistrée dans la base.
    pub fn key(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Html => "html",
            Format::Docx => "docx",
            Format::Pdf => "pdf",
            Format::Text => "txt",
        }
    }

    pub fn from_key(key: &str) -> Option<Format> {
        [Format::Markdown, Format::Html, Format::Docx, Format::Pdf, Format::Text].into_iter().find(|f| f.key() == key)
    }

    /// Type MIME pour télécharger le fichier d'origine.
    pub fn mime(self) -> &'static str {
        match self {
            Format::Markdown => "text/markdown; charset=utf-8",
            Format::Html => "text/html; charset=utf-8",
            Format::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Format::Pdf => "application/pdf",
            Format::Text => "text/plain; charset=utf-8",
        }
    }
}

/// Un morceau de document, dans l'ordre de lecture.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// Titre de section (niveau 1 à 6).
    Heading { level: u8, text: String },
    Para(String),
    /// Élément de liste ; `number` = rang dans une liste numérotée (1, 2, 3…), `None` pour une puce. `depth` : 0 au premier niveau.
    Item { depth: u8, number: Option<u32>, text: String },
    Code(String),
    /// Lignes de cellules ; la première est l'en-tête quand `header` est vrai.
    Table { header: bool, rows: Vec<Vec<String>> },
    /// Les blocs qui suivent sont à partir de cette page (PDF).
    Page(u32),
}

/// Ce qu'on tire d'un fichier.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extracted {
    /// Titre indiqué par le document lui-même (en-tête, balise `title`, propriétés Word) ; `None` : le nom du fichier en tient lieu.
    pub title: Option<String>,
    pub tags: Vec<String>,
    /// Adresse d'origine du document (page intranet), si le fichier la donne.
    pub url: Option<String>,
    pub blocks: Vec<Block>,
    /// Ce qui a été lu avec difficulté (page de PDF illisible, document sans texte…), à montrer à la personne qui importe.
    pub warnings: Vec<String>,
}

impl Extracted {
    /// Y a-t-il du texte à chercher ?
    pub fn has_text(&self) -> bool {
        self.blocks.iter().any(|b| match b {
            Block::Heading { text, .. } | Block::Para(text) | Block::Code(text) | Block::Item { text, .. } => text.chars().any(char::is_alphanumeric),
            Block::Table { rows, .. } => rows.iter().flatten().any(|c| c.chars().any(char::is_alphanumeric)),
            Block::Page(_) => false,
        })
    }
}

/// Lit un document. Une erreur est un fichier illisible (message en français, montré tel quel) ; un fichier lisible mais sans
/// texte n'est pas une erreur ici, c'est `Extracted::has_text` qui le dit.
pub fn extract(format: Format, bytes: &[u8]) -> Result<Extracted> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        bail!("fichier trop volumineux ({} Mo, maximum {} Mo)", bytes.len() / (1024 * 1024), MAX_DOCUMENT_BYTES / (1024 * 1024));
    }
    let mut out = match format {
        Format::Markdown => markdown::extract(&text::decode(bytes)),
        Format::Html => html::extract(bytes),
        Format::Docx => docx::extract(bytes)?,
        Format::Pdf => pdf::extract(bytes)?,
        Format::Text => text::extract(&text::decode(bytes)),
    };
    out.title = out.title.map(|t| clean_line(&t)).filter(|t| !t.is_empty());
    out.tags = normalize_tags(&out.tags);
    out.url = out.url.map(|u| u.trim().to_string()).filter(|u| is_web_address(u));
    Ok(out)
}

/// Une adresse qu'on accepte d'afficher comme lien : web seulement (jamais `javascript:` ni `file:`).
pub fn is_web_address(u: &str) -> bool {
    let l = u.trim().to_lowercase();
    (l.starts_with("http://") || l.starts_with("https://")) && !l.contains(char::is_whitespace)
}

/// Texte sur une seule ligne : espaces insécables, tirets conditionnels et caractères invisibles remplacés ou retirés, espaces
/// multiples réduits.
pub fn clean_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        let c = match c {
            '\u{00ad}' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}' | '\u{feff}' => continue,
            '\u{00a0}' | '\u{202f}' | '\u{2007}' | '\u{2009}' => ' ',
            c if c.is_control() => ' ',
            c => c,
        };
        if c.is_whitespace() {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    out
}

/// Étiquettes d'un document : en minuscules, sans doublon, une quinzaine au plus.
pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for t in tags {
        let t = clean_line(t).to_lowercase();
        if !t.is_empty() && t.chars().count() <= 40 && !out.contains(&t) {
            out.push(t);
        }
    }
    out.truncate(15);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_format_vient_du_contenu_puis_du_nom() {
        assert_eq!(Format::detect("guide.md", b"# Titre"), Some(Format::Markdown));
        assert_eq!(Format::detect("page.HTM", b"<p>x</p>"), Some(Format::Html));
        assert_eq!(Format::detect("sans-extension", b"  <!DOCTYPE html><html><body>x"), Some(Format::Html));
        assert_eq!(Format::detect("notes.txt", b"bonjour"), Some(Format::Text));
        // un PDF reste un PDF, quel que soit son nom
        assert_eq!(Format::detect("export.bin", b"%PDF-1.7\n..."), Some(Format::Pdf));
        // l'extension promet un PDF ou un document Word que le contenu n'est pas
        assert_eq!(Format::detect("faux.pdf", b"juste du texte"), None);
        assert_eq!(Format::detect("faux.docx", b"juste du texte"), None);
        // un zip quelconque n'est pas un document Word
        assert_eq!(Format::detect("archive.zip", b"PK\x03\x04 pas un docx"), None);
        assert_eq!(Format::detect("image.png", b"\x89PNG"), None);
        assert_eq!(Format::detect("tableur.xlsx", b"PK\x03\x04"), None);
    }

    #[test]
    fn les_formats_se_retrouvent_par_leur_cle() {
        for f in [Format::Markdown, Format::Html, Format::Docx, Format::Pdf, Format::Text] {
            assert_eq!(Format::from_key(f.key()), Some(f));
            assert!(!f.key().is_empty() && f.mime().contains('/'));
        }
        assert_eq!(Format::from_key("exe"), None);
    }

    #[test]
    fn texte_nettoye_sur_une_ligne() {
        assert_eq!(clean_line("  Réseau\u{00a0}Wi-Fi \t\n invité\u{200b} "), "Réseau Wi-Fi invité");
        assert_eq!(clean_line("trait\u{00ad}d'union"), "traitd'union");
        assert_eq!(clean_line("\u{feff}\u{0007}"), "");
    }

    #[test]
    fn seules_les_adresses_web_sont_des_liens() {
        assert!(is_web_address("https://intranet.exemple.fr/vpn#erreur-809"));
        assert!(is_web_address(" http://aide "));
        assert!(!is_web_address("javascript:alert(1)"));
        assert!(!is_web_address("file:///etc/passwd"));
        assert!(!is_web_address("https://exemple.fr/a b"));
        assert!(!is_web_address("//exemple.fr"));
    }

    #[test]
    fn un_document_trop_gros_est_refuse_avec_un_message_clair() {
        let gros = vec![b'a'; MAX_DOCUMENT_BYTES + 1];
        let e = extract(Format::Text, &gros).unwrap_err().to_string();
        assert!(e.contains("trop volumineux") && e.contains("50 Mo"), "{e}");
    }

    /// Les documents du jeu d'évaluation (eval/corpus) : un de chaque format, tels que les services les produisent.
    fn corpus() -> Vec<(String, Vec<u8>)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../eval/corpus");
        let mut files: Vec<_> = std::fs::read_dir(&dir).expect("dossier eval/corpus").flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
        files.sort();
        files.into_iter().map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read(&p).unwrap())).collect()
    }

    #[test]
    fn les_documents_du_corpus_sont_lisibles_et_decoupes_en_passages() {
        let docs = corpus();
        assert!(docs.len() >= 15, "{} documents", docs.len());
        let mut formats = std::collections::HashSet::new();
        for (name, bytes) in &docs {
            let format = Format::detect(name, bytes).unwrap_or_else(|| panic!("{name} : format inconnu"));
            formats.insert(format.key());
            let e = extract(format, bytes).unwrap_or_else(|err| panic!("{name} : {err}"));
            assert!(e.has_text() && e.warnings.is_empty(), "{name} : {:?}", e.warnings);
            let titre = e.title.clone().or_else(|| e.blocks.iter().find_map(|b| if let Block::Heading { level: 1, text } = b { Some(text.clone()) } else { None }));
            let titre = titre.unwrap_or_else(|| panic!("{name} : aucun titre"));
            let chunks = chunk(&titre, &e.blocks);
            assert!(!chunks.is_empty() && chunks.len() <= 12, "{name} : {} passages", chunks.len());
            for c in &chunks {
                let n = c.text.chars().count();
                assert!((40..=1600).contains(&n), "{name} : passage de {n} caractères : {:?}", c.text);
            }
            if std::env::var("AFFICHER_CORPUS").is_ok() {
                println!("\n=== {name} ({}) « {titre} » : {} passages", format.key(), chunks.len());
                for (i, c) in chunks.iter().enumerate() {
                    println!("--- {} [{}] p.{:?} ({} car.)\n{}", i + 1, c.section.join(" › "), c.page, c.text.chars().count(), c.text);
                }
            }
        }
        assert_eq!(formats.len(), 5, "un document de chaque format : {formats:?}");
    }

    #[test]
    fn titre_adresse_et_etiquettes_sont_normalises() {
        let e = extract(Format::Markdown, "---\ntitle: \"  Guide   VPN \"\ntags: VPN, Télétravail, vpn, \ntitle2: x\nurl: javascript:alert(1)\n---\nTexte.\n".as_bytes()).unwrap();
        assert_eq!(e.title.as_deref(), Some("Guide VPN"));
        assert_eq!(e.tags, vec!["vpn", "télétravail"]);
        assert_eq!(e.url, None, "une adresse qui n'est pas web est écartée");
        assert!(e.has_text());
        assert!(!extract(Format::Text, b"  \n\n").unwrap().has_text());
    }
}
