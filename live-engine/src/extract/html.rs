//! HTML : pages d'un intranet ou d'un wiki enregistrées en fichier. On lit le contenu principal (`main`, `article` ou `body`) :
//! titres, paragraphes, listes, tableaux, blocs de code. Menus, pieds de page, formulaires, scripts et styles sont ignorés.

use super::{clean_line, is_web_address, Block, Extracted};
use scraper::{ElementRef, Html, Node, Selector};

/// Éléments dont le contenu n'est pas du texte de document.
const SKIPPED: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "canvas", "iframe", "object", "embed", "nav", "footer", "aside", "form", "button", "select",
    "textarea", "input", "head", "title", "dialog", "menu", "audio", "video", "map",
];

/// Éléments qui forment des blocs : le texte qui précède et celui qui suit n'appartiennent pas au même paragraphe.
const BLOCKS: &[&str] = &[
    "address", "article", "blockquote", "body", "caption", "center", "details", "dd", "div", "dl", "dt", "fieldset", "figcaption", "figure", "form",
    "header", "hgroup", "hr", "html", "li", "main", "ol", "p", "section", "summary", "table", "tbody", "td", "tfoot", "th", "thead", "tr", "ul",
];

/// Décode une page : UTF-8 si le fichier l'est, sinon l'encodage annoncé par la page (`charset`), à défaut Windows-1252.
fn decode(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return super::text::decode(bytes);
    }
    let plain = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if let Ok(s) = std::str::from_utf8(plain) {
        return s.to_string();
    }
    let head = String::from_utf8_lossy(&plain[..plain.len().min(2048)]).to_lowercase();
    let declared = head.find("charset=").map(|i| {
        head[i + 8..].trim_start_matches(['"', '\'']).chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')).collect::<String>()
    });
    let encoding = declared.and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes())).unwrap_or(encoding_rs::WINDOWS_1252);
    encoding.decode_without_bom_handling(plain).0.into_owned()
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("sélecteur CSS constant")
}

/// L'élément est-il caché à l'écran (donc pas du texte pour le lecteur) ?
fn hidden(el: ElementRef) -> bool {
    let v = el.value();
    v.attr("hidden").is_some()
        || v.attr("aria-hidden") == Some("true")
        || v.attr("style").is_some_and(|s| s.to_lowercase().replace(' ', "").contains("display:none"))
}

fn skipped(el: ElementRef) -> bool {
    SKIPPED.contains(&el.value().name()) || hidden(el)
}

/// Tout le texte d'un élément sur une ligne, les blocs séparés par une espace.
fn inline_text(el: ElementRef) -> String {
    let mut out = String::new();
    collect_text(el, &mut out);
    clean_line(&out)
}

fn collect_text(el: ElementRef, out: &mut String) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) => {
                let Some(child_el) = ElementRef::wrap(child) else { continue };
                if skipped(child_el) {
                    continue;
                }
                let block = BLOCKS.contains(&e.name()) || e.name() == "br";
                if block {
                    out.push(' ');
                }
                if e.name() == "code" || e.name() == "kbd" || e.name() == "samp" {
                    let inner = inline_text(child_el);
                    if !inner.is_empty() {
                        out.push_str(&format!("`{inner}`"));
                    }
                } else {
                    collect_text(child_el, out);
                }
                if e.name() == "a" {
                    if let Some(href) = e.attr("href").map(str::trim).filter(|h| is_web_address(h)) {
                        let label = inline_text(child_el);
                        if label != href {
                            out.push_str(&format!(" ({href})"));
                        }
                    }
                }
                if block {
                    out.push(' ');
                }
            }
            _ => {}
        }
    }
}

struct Walker {
    blocks: Vec<Block>,
    /// Texte en cours, jusqu'à la prochaine frontière de bloc.
    buf: String,
}

impl Walker {
    fn flush(&mut self) {
        let t = clean_line(&self.buf);
        self.buf.clear();
        if !t.is_empty() {
            self.blocks.push(Block::Para(t));
        }
    }

    /// Le contenu d'un élément, avec ses blocs et son texte courant.
    fn children(&mut self, el: ElementRef, depth: u8) {
        for child in el.children() {
            match child.value() {
                Node::Text(t) => self.buf.push_str(t),
                Node::Element(_) => {
                    if let Some(c) = ElementRef::wrap(child) {
                        self.element(c, depth);
                    }
                }
                _ => {}
            }
        }
    }

    fn element(&mut self, el: ElementRef, depth: u8) {
        if skipped(el) {
            return;
        }
        let name = el.value().name();
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                let text = inline_text(el);
                if !text.is_empty() {
                    self.blocks.push(Block::Heading { level: name.as_bytes()[1] - b'0', text });
                }
            }
            "ul" | "ol" => {
                self.flush();
                let mut number = if name == "ol" { Some(el.value().attr("start").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(1)) } else { None };
                for li in el.child_elements().filter(|c| c.value().name() == "li" && !skipped(*c)) {
                    self.list_item(li, depth, number);
                    number = number.map(|n| n + 1);
                }
            }
            "pre" => {
                self.flush();
                let raw: String = el.text().collect();
                let code = raw.trim_matches('\n').trim_end().to_string();
                if !code.trim().is_empty() {
                    self.blocks.push(Block::Code(code));
                }
            }
            "table" => {
                self.flush();
                self.table(el);
            }
            "br" => self.buf.push(' '),
            "hr" => self.flush(),
            "code" | "kbd" | "samp" => {
                let inner = inline_text(el);
                if !inner.is_empty() {
                    self.buf.push_str(&format!("`{inner}`"));
                }
            }
            "a" => {
                self.children(el, depth);
                if let Some(href) = el.value().attr("href").map(str::trim).filter(|h| is_web_address(h)) {
                    if inline_text(el) != href {
                        self.buf.push_str(&format!(" ({href})"));
                    }
                }
            }
            _ if BLOCKS.contains(&name) => {
                self.flush();
                self.children(el, depth);
                self.flush();
            }
            _ => self.children(el, depth), // élément de texte (span, strong, em…)
        }
    }

    /// Un élément de liste : son texte propre, puis les éléments de ses sous-listes.
    fn list_item(&mut self, li: ElementRef, depth: u8, number: Option<u32>) {
        let own = |w: &mut Walker| {
            let text = clean_line(&std::mem::take(&mut w.buf));
            if !text.is_empty() {
                w.blocks.push(Block::Item { depth: depth.min(5), number, text });
            }
        };
        self.flush();
        for child in li.children() {
            match child.value() {
                Node::Text(t) => self.buf.push_str(t),
                Node::Element(e) => {
                    let Some(c) = ElementRef::wrap(child) else { continue };
                    if skipped(c) {
                        continue;
                    }
                    if matches!(e.name(), "ul" | "ol") {
                        own(self);
                        self.element(c, depth + 1);
                    } else if matches!(e.name(), "p" | "div") {
                        // paragraphes dans l'élément : un seul texte
                        self.buf.push(' ');
                        self.element_inline(c, depth);
                        self.buf.push(' ');
                    } else {
                        self.element(c, depth);
                    }
                }
                _ => {}
            }
        }
        own(self);
    }

    /// Le contenu d'un bloc ajouté au texte en cours (sans le fermer).
    fn element_inline(&mut self, el: ElementRef, depth: u8) {
        for child in el.children() {
            match child.value() {
                Node::Text(t) => self.buf.push_str(t),
                Node::Element(_) => {
                    if let Some(c) = ElementRef::wrap(child) {
                        if !skipped(c) && !matches!(c.value().name(), "ul" | "ol" | "table" | "pre") {
                            self.element(c, depth);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn table(&mut self, table: ElementRef) {
        let mut rows: Vec<Vec<String>> = vec![];
        let mut header = false;
        for tr in table.descendent_elements().filter(|e| e.value().name() == "tr" && !skipped(*e)) {
            let cells: Vec<ElementRef> = tr.child_elements().filter(|c| matches!(c.value().name(), "td" | "th")).collect();
            if cells.is_empty() {
                continue;
            }
            if rows.is_empty() {
                header = cells.iter().all(|c| c.value().name() == "th");
            }
            rows.push(cells.into_iter().map(inline_text).collect());
        }
        if rows.iter().flatten().any(|c| !c.is_empty()) {
            self.blocks.push(Block::Table { header, rows });
        }
    }
}

/// Le titre de la balise `title` porte souvent le nom du site (« Compte verrouillé - Intranet DSI »). Quand le titre de la page
/// (premier `h1`) le recoupe, le plus précis des deux l'emporte ; sinon, avec plusieurs segments, le premier (par convention, le
/// nom du site vient en dernier).
fn best_title(tag: Option<String>, h1: Option<String>) -> Option<String> {
    let segments: Vec<String> = match &tag {
        Some(t) => {
            let mut parts = vec![t.clone()];
            for sep in [" - ", " – ", " — ", " | ", " · ", " :: ", " » "] {
                parts = parts.iter().flat_map(|p| p.split(sep).map(str::to_string).collect::<Vec<_>>()).collect();
            }
            parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
        }
        None => vec![],
    };
    let related = |a: &str, b: &str| {
        let (a, b) = (crate::terms::fold(a).trim().to_string(), crate::terms::fold(b).trim().to_string());
        !a.is_empty() && !b.is_empty() && (a.contains(&b) || b.contains(&a))
    };
    match (&h1, segments.first()) {
        (Some(h), Some(first)) => {
            let best = segments.iter().filter(|s| related(s, h)).chain(std::iter::once(h)).max_by_key(|s| s.chars().count()).cloned();
            match segments.iter().any(|s| related(s, h)) {
                true => best,
                false => Some(first.clone()),
            }
        }
        (Some(h), None) => Some(h.clone()),
        (None, Some(_)) => tag,
        (None, None) => None,
    }
}

pub fn extract(bytes: &[u8]) -> Extracted {
    let html = Html::parse_document(&decode(bytes));
    let mut out = Extracted {
        title: html.select(&selector("head > title, title")).next().map(|t| t.text().collect::<String>()).map(|t| clean_line(&t)).filter(|t| !t.is_empty()),
        url: html
            .select(&selector("link[rel~=canonical]"))
            .next()
            .and_then(|l| l.value().attr("href"))
            .map(|u| u.trim().to_string())
            .filter(|u| is_web_address(u)),
        ..Default::default()
    };
    if let Some(keywords) = html.select(&selector("meta[name=keywords]")).next().and_then(|m| m.value().attr("content")) {
        out.tags = keywords.split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect();
    }

    // le contenu principal de la page, à défaut le corps entier
    let root = ["main", "article", "[role=main]", "body"]
        .iter()
        .find_map(|css| html.select(&selector(css)).find(|e| !hidden(*e)))
        .unwrap_or_else(|| html.root_element());
    let mut w = Walker { blocks: vec![], buf: String::new() };
    w.children(root, 0);
    w.flush();
    out.blocks = w.blocks;
    let h1 = out.blocks.iter().find_map(|b| if let Block::Heading { level: 1, text } = b { Some(text.clone()) } else { None });
    out.title = best_title(out.title.take(), h1);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(html: &str) -> Vec<Block> {
        extract(html.as_bytes()).blocks
    }

    fn para(t: &str) -> Block {
        Block::Para(t.into())
    }

    #[test]
    fn titre_adresse_et_etiquettes_de_la_page() {
        let e = extract(
            "<html><head><title> Guide VPN </title><link rel=\"canonical\" href=\"https://intranet.exemple.fr/vpn\"><meta name=\"keywords\" content=\"vpn, télétravail\"></head><body><p>x</p></body></html>"
                .as_bytes(),
        );
        assert_eq!(e.title.as_deref(), Some("Guide VPN"));
        assert_eq!(e.url.as_deref(), Some("https://intranet.exemple.fr/vpn"));
        assert_eq!(e.tags, vec!["vpn", "télétravail"]);
        // une adresse qui n'est pas web n'est pas gardée
        let e = extract(b"<html><head><link rel=canonical href=javascript:x></head><body>x</body></html>");
        assert_eq!(e.url, None);
    }

    #[test]
    fn le_titre_de_la_page_ecarte_le_nom_du_site() {
        let t = |tag: Option<&str>, h1: Option<&str>| best_title(tag.map(String::from), h1.map(String::from));
        assert_eq!(t(Some("Compte verrouillé : que faire ? - Intranet DSI"), Some("Compte verrouillé : que faire ?")).as_deref(), Some("Compte verrouillé : que faire ?"));
        // le titre de la page précise celui de l'onglet
        assert_eq!(t(Some("Messagerie Outlook - Intranet DSI"), Some("Messagerie Outlook : problèmes courants")).as_deref(), Some("Messagerie Outlook : problèmes courants"));
        // et inversement
        assert_eq!(t(Some("Guide VPN complet | Intranet"), Some("Guide VPN")).as_deref(), Some("Guide VPN complet"));
        // rien en commun : le premier segment est le titre, le nom du site vient après
        assert_eq!(t(Some("Hameçonnage et sécurité - Intranet DSI"), Some("Reconnaître un courriel suspect")).as_deref(), Some("Hameçonnage et sécurité"));
        assert_eq!(t(None, Some("Seul le h1")).as_deref(), Some("Seul le h1"));
        assert_eq!(t(Some("Guide VPN - Installation"), None).as_deref(), Some("Guide VPN - Installation"), "sans h1, rien n'est coupé");
        assert_eq!(t(None, None), None);
    }

    #[test]
    fn titres_paragraphes_et_texte_en_ligne() {
        let b = blocks("<body><h1>Guide <em>VPN</em></h1><p>Le VPN permet de <b>travailler</b> à distance,<br>depuis chez soi.</p><div>Un bloc <span>sans</span> paragraphe</div><h2>Erreur 809</h2></body>");
        assert_eq!(
            b,
            vec![
                Block::Heading { level: 1, text: "Guide VPN".into() },
                para("Le VPN permet de travailler à distance, depuis chez soi."),
                para("Un bloc sans paragraphe"),
                Block::Heading { level: 2, text: "Erreur 809".into() },
            ]
        );
    }

    #[test]
    fn navigation_scripts_formulaires_et_caches_sont_ignores() {
        let b = blocks(
            "<body><nav>Accueil | Contact</nav><script>var x = 'secret';</script><style>p{}</style><main><p>Utile.</p><div hidden>caché</div><p style=\"display: none\">invisible</p><form><input value=\"x\"><button>Envoyer</button></form><aside>Publicité</aside></main><footer>Mentions légales</footer></body>",
        );
        assert_eq!(b, vec![para("Utile.")]);
        // sans <main>, un <article> est le contenu principal : l'en-tête du site n'en fait pas partie
        let b = blocks("<body><div>Bandeau du site</div><article><h1>Sujet</h1><p>Texte.</p></article></body>");
        assert_eq!(b, vec![Block::Heading { level: 1, text: "Sujet".into() }, para("Texte.")]);
    }

    #[test]
    fn listes_numerotees_a_puces_et_imbriquees() {
        let b = blocks("<ol><li>Ouvrez le menu.</li><li>Choisissez :<ul><li>Réseau</li><li>VPN</li></ul></li><li><p>Validez.</p><p>Fermez.</p></li></ol><ol start=\"4\"><li>Quatre</li></ol><ul><li>A</li></ul>");
        let item = |depth, number, text: &str| Block::Item { depth, number, text: text.into() };
        assert_eq!(
            b,
            vec![
                item(0, Some(1), "Ouvrez le menu."),
                item(0, Some(2), "Choisissez :"),
                item(1, None, "Réseau"),
                item(1, None, "VPN"),
                item(0, Some(3), "Validez. Fermez."),
                item(0, Some(4), "Quatre"),
                item(0, None, "A"),
            ]
        );
    }

    #[test]
    fn tableaux_code_et_liens() {
        let b = blocks(
            "<table><thead><tr><th>Code</th><th>Cause</th></tr></thead><tbody><tr><td>809</td><td>Pare-feu <b>bloqué</b></td></tr><tr><td>691</td><td><ul><li>Mot</li><li>de passe</li></ul></td></tr></tbody></table>\
             <pre>ipconfig /flushdns\nipconfig /renew</pre><p>Voir <a href=\"https://portail.exemple.fr/aide\">le portail</a>, <a href=\"https://x.fr\">https://x.fr</a>, <a href=\"#ancre\">ici</a> et <code>gpupdate</code>.</p>",
        );
        assert_eq!(
            b,
            vec![
                Block::Table { header: true, rows: vec![vec!["Code".into(), "Cause".into()], vec!["809".into(), "Pare-feu bloqué".into()], vec!["691".into(), "Mot de passe".into()]] },
                Block::Code("ipconfig /flushdns\nipconfig /renew".into()),
                para("Voir le portail (https://portail.exemple.fr/aide), https://x.fr, ici et `gpupdate`."),
            ]
        );
    }

    #[test]
    fn encodage_declare_par_la_page() {
        // Windows-1252 : « é » = 0xE9, annoncé comme ISO-8859-1 (c'est le cas de la plupart des vieilles pages)
        let page = b"<html><head><meta charset=\"iso-8859-1\"><title>R\xE9seau</title></head><body><p>\xC9cran \x80 </p></body></html>";
        let e = extract(page);
        assert_eq!(e.title.as_deref(), Some("Réseau"));
        assert_eq!(e.blocks, vec![para("Écran €")]);
        // UTF-8 reste UTF-8, avec ou sans déclaration
        assert_eq!(blocks("<p>Écran « bleu »</p>"), vec![para("Écran « bleu »")]);
    }

    #[test]
    fn une_page_vide_ou_cassee_ne_plante_pas() {
        assert!(blocks("").is_empty());
        assert!(blocks("<<<>>> <p unclosed <b>").len() <= 1);
        assert_eq!(blocks("texte seul sans balise"), vec![para("texte seul sans balise")]);
    }
}
