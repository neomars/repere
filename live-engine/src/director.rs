//! Directives : le LLM écrit du texte normal avec des directives inline, par exemple `[[source: #12]]` (citer un extrait de document
//! proposé à ce tour), `[[show: #42]]` (montrer une image proposée à ce tour) ou `[[replies: Oui | Non]]` (réponses rapides cliquables).
//! Ce format est bien plus fiable que le function-calling sur des modèles locaux,
//! et permet d'agir dès que la directive apparaît dans le flux.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Directive {
    /// Une image de la liste proposée à ce tour : `[[show: #42]]`. Le moteur vérifie qu'elle a bien été proposée.
    ShowId { id: i64 },
    /// Des extraits de document cités : `[[source: #12]]` ou `[[source: #12, #15]]`. Le moteur vérifie qu'ils ont bien été proposés.
    Source { ids: Vec<i64> },
    /// Des réponses rapides : `[[replies: Oui | Non]]`.
    Replies { items: Vec<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Text(String),
    Directive(Directive),
}

/// Analyseur incrémental : on lui donne les jetons au fil de l'eau.
#[derive(Default)]
pub struct StreamParser {
    buf: String,
}

const OPEN: &str = "[[";
const CLOSE: &str = "]]";
/// Une directive plus longue que ça est du texte ordinaire (sécurité contre un `[[` isolé).
const MAX_DIRECTIVE: usize = 300;

impl StreamParser {
    pub fn feed(&mut self, chunk: &str) -> Vec<Piece> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        loop {
            if let Some(start) = self.buf.find(OPEN) {
                if start > 0 {
                    out.push(Piece::Text(self.buf[..start].to_string()));
                    self.buf.drain(..start);
                }
                if let Some(end) = self.buf.find(CLOSE) {
                    let body = self.buf[OPEN.len()..end].to_string();
                    self.buf.drain(..end + CLOSE.len());
                    if let Some(d) = parse_directive(&body) {
                        out.push(Piece::Directive(d));
                    }
                    continue;
                }
                if self.buf.len() > MAX_DIRECTIVE {
                    // Directive jamais refermée (le LLM a écrit `]` au lieu de `]]`) ou fausse alerte.
                    let t = std::mem::take(&mut self.buf);
                    out.push(unclosed_directive(&t).map_or(Piece::Text(t), Piece::Directive));
                }
                return out;
            }
            // Pas de `[[` complet. On retient un éventuel `[` final (début de `[[`).
            let keep = if self.buf.ends_with('[') { 1 } else { 0 };
            let emit_len = self.buf.len() - keep;
            if emit_len > 0 {
                out.push(Piece::Text(self.buf[..emit_len].to_string()));
                self.buf.drain(..emit_len);
            }
            return out;
        }
    }

    pub fn finish(&mut self) -> Vec<Piece> {
        let rest = std::mem::take(&mut self.buf);
        if rest.is_empty() {
            vec![]
        } else {
            vec![unclosed_directive(&rest).map_or(Piece::Text(rest), Piece::Directive)]
        }
    }
}

/// Récupère une directive `[[replies: "A"] | "B"]` mal refermée (un seul `]`, ou aucun) : sans cela elle serait
/// affichée et lue à voix haute. Seules les propositions de réponse sont rattrapées ; le reste redevient du texte.
fn unclosed_directive(s: &str) -> Option<Directive> {
    let body = s.strip_prefix(OPEN)?;
    let (name, _) = body.split_once(':')?;
    if name.trim().to_lowercase() != "replies" {
        return None;
    }
    parse_directive(body)
}

/// « "Oui" », « [Oui] », « «Oui» », « - Oui », « 1. Oui » → « Oui » : le LLM met parfois des guillemets, des crochets
/// ou une puce autour des choix. Un guillemet n'est retiré que s'il encadre la proposition (ou s'il est orphelin) :
/// `Il a dit "oui", puis "non"` reste entière.
fn clean_reply(s: &str) -> String {
    let t = s.trim_matches(|c: char| c.is_whitespace() || matches!(c, '|' | '[' | ']'));
    let t = t.trim_start_matches(|c: char| matches!(c, '-' | '–' | '—' | '•' | '*') || c.is_whitespace());
    // numérotation « 1. » / « 2) »
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    let t = match t[digits..].chars().next() {
        Some('.' | ')') if (1..=2).contains(&digits) && t[digits + 1..].starts_with(char::is_whitespace) => &t[digits + 1..],
        _ => t,
    };
    let mut t = t.trim();
    let opened = t.starts_with(['"', '“', '«']);
    if opened {
        t = t[t.chars().next().map_or(0, char::len_utf8)..].trim_start();
    }
    if let Some(last) = t.chars().last().filter(|c| matches!(c, '"' | '”' | '»')) {
        let orphan = match last {
            '"' => t.matches('"').count() % 2 == 1,
            '”' => t.matches('”').count() > t.matches('“').count(),
            _ => t.matches('»').count() > t.matches('«').count(),
        };
        if opened || orphan {
            t = t[..t.len() - last.len_utf8()].trim_end();
        }
    }
    if t.len() > 2 && t.starts_with('\'') && t.ends_with('\'') {
        t = t[1..t.len() - 1].trim();
    }
    t.trim_matches(|c: char| c.is_whitespace() || matches!(c, '|' | '[' | ']')).to_string()
}

/// `"A", "B", "C"` (au lieu de `A | B | C`) → A, B, C. `None` si le texte n'est pas une suite de citations entre
/// guillemets séparées par des virgules : une proposition qui en contient une (`Il a dit "oui"`) reste entière.
fn quoted_items(s: &str) -> Option<Vec<String>> {
    let closers = ['"', '”', '»'];
    let mut items = vec![];
    let mut rest = s;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '[' | ']'));
        if rest.is_empty() {
            break;
        }
        let open = rest.chars().next()?;
        if !matches!(open, '"' | '“' | '«') {
            return None;
        }
        let body = &rest[open.len_utf8()..];
        let end = body.find(closers)?;
        items.push(body[..end].trim().to_string());
        rest = &body[end + body[end..].chars().next()?.len_utf8()..];
    }
    (!items.is_empty()).then_some(items)
}

/// Un bouton n'est pas un paragraphe : un modèle bavard écrit des propositions de trois lignes. Coupé entre deux mots.
fn short_reply(r: &str) -> String {
    const MAX: usize = 120;
    if r.chars().count() <= MAX {
        return r.to_string();
    }
    let cut: String = r.chars().take(MAX).collect();
    let end = cut.rfind(' ').filter(|i| *i > MAX / 2).unwrap_or(cut.len());
    format!("{}…", cut[..end].trim_end_matches([',', ';', ':', '.', ' ']))
}

/// Les propositions d'une directive `[[replies: …]]` : séparées par `|` ou par des retours à la ligne, ou bien — le LLM
/// s'y laisse prendre — écrites `"A", "B", "C"`. Quatre au plus.
fn split_replies(body: &str) -> Vec<String> {
    body.split(['|', '\n'])
        .flat_map(|part| quoted_items(part).unwrap_or_else(|| vec![part.to_string()]))
        .map(|r| clean_reply(&r))
        .filter(|r| !r.is_empty())
        .map(|r| short_reply(&r))
        .take(4)
        .collect()
}

/// « #42 », « # 42 », « #42, #57 » (le premier), « id=42 » : le numéro d'un média proposé à l'IA.
fn parse_media_id(s: &str) -> Option<i64> {
    let s = s.trim();
    let rest = s.strip_prefix('#').or_else(|| s.strip_prefix("id=")).or_else(|| s.strip_prefix("id:"))?;
    rest.trim().split(|c: char| !c.is_ascii_digit()).next()?.parse::<i64>().ok().filter(|id| *id > 0)
}

/// « #12 », « #12, #15 », « #12 | #15 », « 12 » : les numéros de sources d'une directive `source` (quatre au plus). Si un morceau n'est pas
/// un numéro (« source: Guide VPN 2 »), ce n'est pas une citation : rien n'est cité plutôt qu'un numéro lu au hasard.
fn parse_source_ids(s: &str) -> Option<Vec<i64>> {
    let mut ids: Vec<i64> = vec![];
    for token in s.replace("# ", "#").split([',', ';', '|', ' ', '\n']).map(str::trim).filter(|t| !t.is_empty()) {
        let digits = token.strip_prefix('#').or_else(|| token.strip_prefix("id=")).unwrap_or(token).trim();
        let id = digits.parse::<i64>().ok().filter(|id| *id > 0)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.truncate(4);
    (!ids.is_empty()).then_some(ids)
}

/// Une directive reconnue. Tout le reste (nom inconnu, ancienne syntaxe `[[show: tag1, tag2]]`…) est ignoré : ni affiché ni lu.
pub fn parse_directive(body: &str) -> Option<Directive> {
    let (name, rest) = body.split_once(':')?;
    match name.trim().to_lowercase().as_str() {
        "show" => parse_media_id(rest.split('|').next().unwrap_or("")).map(|id| Directive::ShowId { id }),
        "source" | "sources" => parse_source_ids(rest).map(|ids| Directive::Source { ids }),
        "replies" => {
            let items = split_replies(rest);
            if items.is_empty() { None } else { Some(Directive::Replies { items }) }
        }
        _ => None,
    }
}

/// Mots dont le point ne termine pas une phrase (« Dr. Martin », « M. Dupont »).
const ABBREVIATIONS: [&str; 14] = ["dr", "m", "mm", "mme", "mlle", "mr", "mrs", "pr", "prof", "st", "ste", "vs", "cf", "no"];

/// Le texte avant ce point est-il le numéro d'une étape (« 1. », « 12. » en début de ligne) ? Ce n'est pas la fin d'une phrase.
fn is_list_marker(before: &str) -> bool {
    let line = before.rsplit('\n').next().unwrap_or(before).trim();
    (1..=2).contains(&line.len()) && line.chars().all(|c| c.is_ascii_digit())
}

/// Le texte avant ce point finit-il par une abréviation ou une initiale (« J. K. Rowling ») ?
fn ends_with_abbreviation(before: &str) -> bool {
    let word: String = before.chars().rev().take_while(|c| c.is_alphanumeric()).collect::<Vec<_>>().into_iter().rev().collect();
    let mut cs = word.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) => c.is_uppercase(),
        _ => ABBREVIATIONS.contains(&word.to_lowercase().as_str()),
    }
}

/// Au-delà de ce nombre de caractères sans ponctuation forte, on coupe à une virgule (latence et taille de bloc).
const RUNON: usize = 220;
/// Taille maximale d'un morceau confié à la voix.
pub const MAX_CHUNK: usize = 240;

/// Point de coupe dans un texte trop long : dernière virgule / point-virgule / deux-points suivi d'un espace,
/// à défaut dernier espace, avant `max` caractères. `None` si le texte tient.
fn cut_point(s: &str, max: usize) -> Option<usize> {
    if s.chars().count() <= max {
        return None;
    }
    let limit = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
    let head = &s[..limit];
    let soft = head.char_indices().rev().find(|(i, c)| {
        matches!(c, ',' | ';' | ':') && *i > 40 && s[i + c.len_utf8()..].chars().next().is_some_and(char::is_whitespace)
    });
    if let Some((i, c)) = soft {
        return Some(i + c.len_utf8());
    }
    head.char_indices().rev().find(|(i, c)| c.is_whitespace() && *i > 40).map(|(i, _)| i)
}

/// Découpe un texte en morceaux de `max` caractères au plus (la voix s'en sort mal avec de longs blocs).
pub fn chunk_for_tts(text: &str, max: usize) -> Vec<String> {
    let mut rest = text.trim();
    let mut out = vec![];
    while let Some(cut) = cut_point(rest, max) {
        let (head, tail) = rest.split_at(cut);
        if head.chars().any(|c| c.is_alphanumeric()) {
            out.push(head.trim().to_string());
        }
        rest = tail.trim_start();
    }
    if rest.chars().any(|c| c.is_alphanumeric()) {
        out.push(rest.trim().to_string());
    }
    out
}

/// Découpe le texte en phrases pour alimenter la synthèse vocale au fil de l'eau.
#[derive(Default)]
pub struct SentenceSplitter {
    buf: String,
}

impl SentenceSplitter {
    pub fn feed(&mut self, text: &str) -> Vec<String> {
        self.buf.push_str(text);
        let mut out = Vec::new();
        loop {
            let idx = self.buf.char_indices().find(|(i, c)| {
                matches!(c, '.' | '!' | '?' | '…' | '\n')
                    // pas de coupure sur "3.5" ou "etc.x" : il faut un espace/fin après
                    && self.buf[i + c.len_utf8()..].chars().next().is_some_and(|n| n.is_whitespace())
                    && !(*c == '.' && (ends_with_abbreviation(&self.buf[..*i]) || is_list_marker(&self.buf[..*i])))
            });
            match idx {
                Some((i, c)) => {
                    let end = i + c.len_utf8();
                    let s = self.buf[..end].trim().to_string();
                    self.buf.drain(..end);
                    if s.chars().any(|c| c.is_alphanumeric()) { out.push(s); }
                }
                None => break,
            }
        }
        // Longue phrase sans ponctuation forte : on n'attend pas la fin pour commencer à parler.
        while let Some(cut) = cut_point(&self.buf, RUNON) {
            let s = self.buf[..cut].trim().to_string();
            self.buf.drain(..cut);
            if s.chars().any(|c| c.is_alphanumeric()) { out.push(s); }
        }
        out
    }

    pub fn finish(&mut self) -> Option<String> {
        let s = std::mem::take(&mut self.buf).trim().to_string();
        if s.chars().any(|c| c.is_alphanumeric()) { Some(s) } else { None }
    }
}
/// Retire ce qui ne se lit pas à voix haute : marques de mise en forme Markdown (`**gras**`, `*italique*`, `# titre`,
/// `` `code` ``) et emojis. Le texte qu'elles entourent est conservé (« Cliquez sur **Démarrer > Paramètres** » est lu en
/// entier). Un tiret bas devient une espace : « mot_de_passe » se lit « mot de passe ». Sans mémoire, donc applicable
/// jeton par jeton, avant le découpage en phrases.
pub fn for_speech(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '*' | '#' | '`' | '~') && !is_pictograph(*c))
        .map(|c| if c == '_' { ' ' } else { c })
        .collect()
}

fn is_pictograph(c: char) -> bool {
    let u = c as u32;
    u >= 0x1F000 || (0x2600..=0x27BF).contains(&u) || (0x2B00..=0x2BFF).contains(&u) || (0xFE00..=0xFE0F).contains(&u) || u == 0x200D
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(chunks: &[&str]) -> Vec<Piece> {
        let mut p = StreamParser::default();
        let mut out = vec![];
        for c in chunks { out.extend(p.feed(c)); }
        out.extend(p.finish());
        out
    }

    fn text(pieces: &[Piece]) -> String {
        pieces.iter().filter_map(|p| if let Piece::Text(t) = p { Some(t.as_str()) } else { None }).collect()
    }

    fn items(body: &str) -> Vec<String> {
        match parse_directive(body) {
            Some(Directive::Replies { items }) => items,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn directive_decoupee_en_plusieurs_jetons() {
        let r = all(&["Voici la procédure. [", "[show: #1", "2]", "] Dites-moi si cela fonctionne."]);
        assert_eq!(text(&r), "Voici la procédure.  Dites-moi si cela fonctionne.");
        assert_eq!(r.iter().filter(|p| matches!(p, Piece::Directive(_))).count(), 1);
        assert!(r.contains(&Piece::Directive(Directive::ShowId { id: 12 })));
    }

    #[test]
    fn replies_mal_refermees_ne_sont_ni_affichees_ni_lues() {
        let src = "Essayons autre chose. [[replies: \"Ouvrir un ticket\"] | \"Reformuler ma question\"] | ";
        let want = Piece::Directive(Directive::Replies { items: vec!["Ouvrir un ticket".into(), "Reformuler ma question".into()] });
        let r = all(&[src]);
        assert_eq!(text(&r), "Essayons autre chose. ");
        assert_eq!(r.last(), Some(&want));
        // même chose si les jetons arrivent un par un
        let chunks: Vec<String> = src.chars().map(String::from).collect();
        let r = all(&chunks.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(text(&r), "Essayons autre chose. ");
        assert_eq!(r.last(), Some(&want));
    }

    #[test]
    fn propositions_entre_guillemets_separees_par_des_virgules() {
        // Régression : « "A", "B", "C" » finissait dans un seul bouton.
        let (q, a, b) = ("Quel est le message d'erreur exact ?", "Ouvrir un ticket", "Reformuler ma question");
        let want = Some(Directive::Replies { items: vec![q.into(), a.into(), b.into()] });
        assert_eq!(parse_directive(&format!("replies: \"{q}\", \"{a}\", \"{b}\"")), want);
        assert_eq!(parse_directive(&format!("replies:\n\"{q}\",\n\"{a}\",\n\"{b}\"\n")), want);
        assert_eq!(parse_directive(&format!("replies: “{q}”, “{a}”, “{b}”")), want);
        assert_eq!(parse_directive(&format!("replies: « {q} », « {a} », « {b} »")), want);
        assert_eq!(parse_directive(&format!("replies: [\"{q}\", \"{a}\"] | \"{b}\"")), want);
        // directive entière, en plusieurs jetons, et la bonne ponctuation reste dans le texte
        let r = all(&["Bonjour. [[replies: \"Oui, avec ", "plaisir\", \"Non merci\"]] Je vous écoute."]);
        assert_eq!(text(&r), "Bonjour.  Je vous écoute.");
        assert!(r.contains(&Piece::Directive(Directive::Replies { items: vec!["Oui, avec plaisir".into(), "Non merci".into()] })));
    }

    #[test]
    fn un_bouton_n_est_pas_un_paragraphe() {
        let long = "Oui, avec grand plaisir, je vous détaille toutes les étapes depuis le début sans oublier le moindre détail de la configuration du poste que nous avons vue ensemble";
        let items = items(&format!("replies: {long} | Non"));
        assert!(items[0].chars().count() <= 121 && items[0].ends_with('…') && items[0].starts_with("Oui, avec grand plaisir"), "{}", items[0]);
        assert_eq!(items[1], "Non");
    }

    #[test]
    fn propositions_en_liste_et_cas_limites() {
        assert_eq!(items("replies:\n- Oui\n- Non\n• Peut-être"), vec!["Oui", "Non", "Peut-être"]);
        assert_eq!(items("replies: 1. Oui\n2) Non"), vec!["Oui", "Non"]);
        // une virgule dans une proposition entre guillemets ne la coupe pas
        assert_eq!(items("replies: \"Oui, bien sûr\" | Non"), vec!["Oui, bien sûr", "Non"]);
        // une citation au milieu d'une proposition la laisse entière
        assert_eq!(items("replies: Il a dit \"oui\", puis \"non\" | Plus tard"), vec!["Il a dit \"oui\", puis \"non\"", "Plus tard"]);
        // des nombres ne sont pas des puces
        assert_eq!(items("replies: 2 jours plus tard | 10 minutes"), vec!["2 jours plus tard", "10 minutes"]);
        assert_eq!(items("replies: A | B | C | D | E"), vec!["A", "B", "C", "D"], "quatre au plus");
        assert_eq!(parse_directive("replies: \"\" |  | []"), None);
    }

    #[test]
    fn crochet_isole_reste_du_texte() {
        let r = all(&["a [b] c"]);
        assert_eq!(text(&r), "a [b] c");
    }

    #[test]
    fn plusieurs_directives_a_la_suite() {
        let r = all(&["[[replies: Oui | Non | Peut-être]][[show: #3]]"]);
        assert_eq!(r, vec![
            Piece::Directive(Directive::Replies { items: vec!["Oui".into(), "Non".into(), "Peut-être".into()] }),
            Piece::Directive(Directive::ShowId { id: 3 }),
        ]);
    }

    #[test]
    fn directives_inconnues_sont_ignorees() {
        // Un nom inconnu (ou une recherche par mots-clés à l'ancienne) n'est ni affiché, ni lu, ni exécuté.
        for body in ["voice: warm", "mood: calme", "ambient: bureau", "state: etape=3", "n'importe quoi", "show: reseau, wifi", "show: #0", "show: #vpn"] {
            assert_eq!(parse_directive(body), None, "{body}");
        }
        let r = all(&["Je regarde. [[voi", "ce: warm]] Voilà. [[state: a=1]]"]);
        assert!(r.iter().all(|p| matches!(p, Piece::Text(_))), "{r:?}");
        assert_eq!(text(&r), "Je regarde.  Voilà. ");
    }

    #[test]
    fn directive_d_une_source_precise() {
        assert_eq!(parse_directive("show: #42"), Some(Directive::ShowId { id: 42 }));
        assert_eq!(parse_directive("show:# 7 "), Some(Directive::ShowId { id: 7 }));
        assert_eq!(parse_directive("SHOW: #12, #15"), Some(Directive::ShowId { id: 12 }), "le premier seulement");
        assert_eq!(parse_directive("show: #42 | autre chose"), Some(Directive::ShowId { id: 42 }));
        assert_eq!(parse_directive("show: id=9"), Some(Directive::ShowId { id: 9 }));
        let r = all(&["Voir la fiche. [[show: ", "#42]] Là."]);
        assert!(r.contains(&Piece::Directive(Directive::ShowId { id: 42 })));
        assert_eq!(text(&r), "Voir la fiche.  Là.");
    }

    #[test]
    fn directive_de_citation_d_extraits() {
        let one = |ids: &[i64]| Some(Directive::Source { ids: ids.to_vec() });
        assert_eq!(parse_directive("source: #12"), one(&[12]));
        assert_eq!(parse_directive("SOURCE:# 7 "), one(&[7]));
        assert_eq!(parse_directive("source: #12, #15"), one(&[12, 15]));
        assert_eq!(parse_directive("source: #12 | #15 ; #12"), one(&[12, 15]), "sans doublon");
        assert_eq!(parse_directive("sources: 3 4"), one(&[3, 4]));
        assert_eq!(parse_directive("source: id=9"), one(&[9]));
        assert_eq!(parse_directive("source: #1, #2, #3, #4, #5, #6"), one(&[1, 2, 3, 4]), "quatre au plus");
        // un morceau qui n'est pas un numéro : ce n'est pas une citation, rien n'est lu au hasard
        for body in ["source: Guide VPN 2", "source: #vpn", "source: #0", "source:", "source: #12, guide", "source: https://intranet.exemple.fr/vpn"] {
            assert_eq!(parse_directive(body), None, "{body}");
        }
        // dans le flux, découpée en jetons, et invisible
        let r = all(&["Autorisez les ports UDP 500 et 4500. [[sour", "ce: #1", "2]] Relancez le client."]);
        assert_eq!(text(&r), "Autorisez les ports UDP 500 et 4500.  Relancez le client.");
        assert!(r.contains(&Piece::Directive(Directive::Source { ids: vec![12] })));
    }

    #[test]
    fn phrases_pour_le_tts() {
        let mut s = SentenceSplitter::default();
        let mut out = s.feed("Bonjour. La version 3.5 est installée ! Et ");
        out.extend(s.feed("vous ? Je vérifie"));
        out.extend(s.finish());
        assert_eq!(out, vec!["Bonjour.", "La version 3.5 est installée !", "Et vous ?", "Je vérifie"]);
    }

    #[test]
    fn le_numero_d_une_etape_ne_coupe_pas_la_phrase() {
        let mut s = SentenceSplitter::default();
        let mut out = s.feed("1. Ouvrez le menu. 2. Cliquez sur OK. 12. Redémarrez.");
        out.extend(s.finish());
        assert_eq!(out, vec!["1. Ouvrez le menu.", "2. Cliquez sur OK.", "12. Redémarrez."]);
        // en début de ligne aussi
        let mut s = SentenceSplitter::default();
        let mut out = s.feed("Voici la marche à suivre :\n1. Ouvrez le menu.\n2. Cliquez sur OK.\n");
        out.extend(s.finish());
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out[0].ends_with("1. Ouvrez le menu.") && out[1] == "2. Cliquez sur OK.", "{out:?}");
    }

    #[test]
    fn la_mise_en_forme_n_est_pas_lue_mais_le_texte_oui() {
        assert_eq!(for_speech("Cliquez sur **Démarrer > Paramètres**, puis sur *Réseau*."), "Cliquez sur Démarrer > Paramètres, puis sur Réseau.");
        assert_eq!(for_speech("## Étapes\n- `ipconfig /all`"), " Étapes\n- ipconfig /all");
        assert_eq!(for_speech("Voilà ✅ c'est réglé 😀 !").split_whitespace().collect::<Vec<_>>().join(" "), "Voilà c'est réglé !");
        assert_eq!(for_speech("Ouvrez le fichier mot_de_passe.txt"), "Ouvrez le fichier mot de passe.txt");
        assert_eq!(for_speech("Rien à retirer ici."), "Rien à retirer ici.");
    }

    /// Ce que la voix lira pour une réponse livrée par petits jetons, comme un vrai LLM en streaming.
    fn lu(reply: &str, chunk: usize) -> Vec<String> {
        let mut sp = SentenceSplitter::default();
        let mut sentences = vec![];
        for part in reply.chars().collect::<Vec<_>>().chunks(chunk) {
            let part: String = part.iter().collect();
            sentences.extend(sp.feed(&for_speech(&part)));
        }
        sentences.extend(sp.finish());
        sentences.iter().flat_map(|s| chunk_for_tts(&s.split_whitespace().collect::<Vec<_>>().join(" "), MAX_CHUNK)).collect()
    }

    #[test]
    fn la_mise_en_forme_ne_fait_perdre_aucune_phrase() {
        let r = "**Important :** redémarrez le poste. Puis *vérifiez* la connexion. Dites-moi si cela fonctionne.";
        for chunk in [1, 3, 7, 200] {
            assert_eq!(
                lu(r, chunk),
                vec!["Important : redémarrez le poste.", "Puis vérifiez la connexion.", "Dites-moi si cela fonctionne."],
                "jetons de {chunk}"
            );
        }
    }

    #[test]
    fn un_passage_en_gras_est_lu_en_entier() {
        for chunk in [1, 4, 100] {
            assert_eq!(
                lu("Allez dans **Démarrer > Paramètres > Réseau et Internet**. Cliquez sur *Wi-Fi*.", chunk),
                vec!["Allez dans Démarrer > Paramètres > Réseau et Internet.", "Cliquez sur Wi-Fi."],
                "jetons de {chunk}"
            );
        }
    }

    #[test]
    fn un_texte_sans_rien_a_lire_reste_vide() {
        assert!(lu("**", 1).is_empty());
        assert!(lu("✅ 😀 ###", 2).is_empty());
    }

    #[test]
    fn abreviations_ne_coupent_pas_la_phrase() {
        assert_eq!(lu("La version 3.5 est prête, Dr. Martin ? Entrez, M. Dupont vous attend.", 6),
                   vec!["La version 3.5 est prête, Dr. Martin ?", "Entrez, M. Dupont vous attend."]);
        assert_eq!(lu("Contactez A. Martin au service informatique. Il répondra vite.", 100),
                   vec!["Contactez A. Martin au service informatique.", "Il répondra vite."]);
    }

    #[test]
    fn phrase_sans_ponctuation_coupee_a_une_virgule() {
        let long = "Je vous conseille de commencer par vérifier que le câble réseau est bien branché des deux côtés, puis de redémarrer le poste, \
                    de tester la connexion sur une autre prise murale, et enfin de contacter le support si le problème persiste malgré tout cela";
        let parts = lu(long, 5);
        assert!(parts.len() >= 2, "{parts:?}");
        assert!(parts.iter().all(|p| p.chars().count() <= MAX_CHUNK), "{parts:?}");
        assert_eq!(parts.join(" ").split_whitespace().count(), long.split_whitespace().count(), "aucun mot perdu");
    }

    #[test]
    fn morceaux_pour_la_voix_sans_perdre_de_mots() {
        let t = "mot ".repeat(200);
        let parts = chunk_for_tts(&t, MAX_CHUNK);
        assert!(parts.len() > 1 && parts.iter().all(|p| p.chars().count() <= MAX_CHUNK));
        assert_eq!(parts.join(" ").split_whitespace().count(), 200);
        assert!(chunk_for_tts("  ...  ", MAX_CHUNK).is_empty());
    }
}
