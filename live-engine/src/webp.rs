//! WebP animés. Les navigateurs les affichent, mais ffmpeg ne sait pas les lire (« image data not found ») : sans miniature, ni
//! image à faire analyser. On en prend donc la première image, remise dans un WebP fixe que ffmpeg décode.
//!
//! Un WebP est un conteneur RIFF : des morceaux (`fourcc`, taille, contenu). Un WebP animé a un morceau `VP8X` (drapeau
//! d'animation), puis des `ANMF` : chacun contient une image complète (`VP8 ` avec son `ALPH` éventuel, ou `VP8L`) et sa place sur
//! le canevas. Seul le contenu de la première image nous intéresse (sa position sur le canevas est ignorée).

use std::path::{Path, PathBuf};

/// Taille maximale lue : une première image tient largement dans quelques Mo, un fichier plus gros n'est pas traité.
const MAX_BYTES: u64 = 64 * 1024 * 1024;

type Chunk<'a> = ([u8; 4], &'a [u8]);

/// Les morceaux consécutifs de `data` (taille sur 4 octets, contenu complété à une longueur paire). `None` si le fichier est tronqué.
fn chunks(data: &[u8]) -> Option<Vec<Chunk<'_>>> {
    let mut out = vec![];
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let id: [u8; 4] = data[pos..pos + 4].try_into().ok()?;
        let len = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().ok()?) as usize;
        let end = (pos + 8).checked_add(len)?;
        if end > data.len() {
            return None;
        }
        out.push((id, &data[pos + 8..end]));
        pos = end + (len & 1);
    }
    Some(out)
}

fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend((payload.len() as u32).to_le_bytes());
    v.extend(payload);
    if payload.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn riff(body: &[u8]) -> Vec<u8> {
    let mut v = b"RIFF".to_vec();
    v.extend((4 + body.len() as u32).to_le_bytes());
    v.extend(b"WEBP");
    v.extend(body);
    v
}

/// Le contenu des morceaux d'un fichier WebP (après son en-tête RIFF).
fn top_chunks(data: &[u8]) -> Option<Vec<Chunk<'_>>> {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return None;
    }
    chunks(&data[12..])
}

/// Le fichier est-il un WebP animé ?
#[cfg(test)]
pub fn is_animated(data: &[u8]) -> bool {
    top_chunks(data).is_some_and(|cs| cs.iter().any(|(id, p)| id == b"ANMF" || (id == b"VP8X" && p.first().is_some_and(|f| f & 0x02 != 0))))
}

/// La première image d'un WebP animé, sous forme de WebP fixe. `None` si le fichier n'est pas animé ou illisible.
pub fn first_frame(data: &[u8]) -> Option<Vec<u8>> {
    let top = top_chunks(data)?;
    let (_, anmf) = top.iter().find(|(id, _)| id == b"ANMF")?;
    if anmf.len() < 16 {
        return None;
    }
    let le24 = |b: &[u8]| u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16;
    let (w1, h1) = (le24(&anmf[6..9]), le24(&anmf[9..12])); // largeur et hauteur moins un
    let frame = chunks(&anmf[16..])?;
    let find = |want: &[u8; 4]| frame.iter().find(|(id, _)| id == want).map(|(_, p)| *p);
    let body = if let Some(lossless) = find(b"VP8L") {
        chunk(b"VP8L", lossless)
    } else {
        let lossy = find(b"VP8 ")?;
        match find(b"ALPH") {
            // image avec transparence : format étendu (drapeau « alpha », taille du canevas), puis l'alpha et l'image
            Some(alpha) => {
                let mut x = vec![0x10, 0, 0, 0];
                x.extend(&w1.to_le_bytes()[..3]);
                x.extend(&h1.to_le_bytes()[..3]);
                [chunk(b"VP8X", &x), chunk(b"ALPH", alpha), chunk(b"VP8 ", lossy)].concat()
            }
            None => chunk(b"VP8 ", lossy),
        }
    };
    Some(riff(&body))
}

/// Si `path` est un WebP animé : une copie fixe de sa première image, dans un fichier temporaire (à supprimer ensuite).
pub fn still_copy(path: &Path) -> Option<PathBuf> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut data = vec![];
    f.take(MAX_BYTES).read_to_end(&mut data).ok()?;
    let still = first_frame(&data)?;
    let tmp = std::env::temp_dir().join(format!(
        "repere-webp-{}-{}.webp",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
    ));
    std::fs::write(&tmp, still).ok()?;
    Some(tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un WebP « animé » à deux images, construit à la main (le contenu des images n'a pas besoin d'être décodable ici).
    fn anime(first: &[Chunk], second: &[u8]) -> Vec<u8> {
        let frame = |w1: u32, h1: u32, content: Vec<u8>| {
            let mut p = vec![0, 0, 0, 0, 0, 0]; // position
            p.extend(&w1.to_le_bytes()[..3]);
            p.extend(&h1.to_le_bytes()[..3]);
            p.extend([100, 0, 0, 0]); // durée, drapeaux
            p.extend(content);
            chunk(b"ANMF", &p)
        };
        let first_content: Vec<u8> = first.iter().flat_map(|(id, p)| chunk(id, p)).collect();
        let mut vp8x = vec![0x02, 0, 0, 0];
        vp8x.extend([63, 0, 0, 31, 0, 0]); // canevas 64 × 32
        let body = [chunk(b"VP8X", &vp8x), chunk(b"ANIM", &[0; 6]), frame(63, 31, first_content), frame(63, 31, chunk(b"VP8L", second))].concat();
        riff(&body)
    }

    #[test]
    fn reconnait_un_webp_anime() {
        let a = anime(&[(*b"VP8L", &[1, 2, 3])], &[9, 9]);
        assert!(is_animated(&a));
        let fixe = riff(&chunk(b"VP8L", &[1, 2, 3]));
        assert!(!is_animated(&fixe));
        assert!(!is_animated(b"RIFF....AVI "));
        assert!(!is_animated(b"") && !is_animated(b"pas un webp du tout"));
    }

    #[test]
    fn premiere_image_d_un_webp_anime_sans_perte() {
        let a = anime(&[(*b"VP8L", &[1, 2, 3])], &[9, 9]);
        // le contenu de la première image (3 octets : complété à 4), rien de la seconde
        assert_eq!(first_frame(&a).unwrap(), riff(&chunk(b"VP8L", &[1, 2, 3])));
        assert!(first_frame(&riff(&chunk(b"VP8L", &[1]))).is_none(), "un WebP fixe n'a pas de première image à extraire");
    }

    #[test]
    fn premiere_image_avec_ou_sans_transparence() {
        let lossy = anime(&[(*b"VP8 ", &[5, 6, 7, 8])], &[9, 9]);
        assert_eq!(first_frame(&lossy).unwrap(), riff(&chunk(b"VP8 ", &[5, 6, 7, 8])));
        let alpha = anime(&[(*b"ALPH", &[1, 1]), (*b"VP8 ", &[5, 6, 7, 8])], &[9, 9]);
        let f = first_frame(&alpha).unwrap();
        let top = top_chunks(&f).unwrap();
        assert_eq!(top.iter().map(|(id, _)| String::from_utf8_lossy(id).to_string()).collect::<Vec<_>>(), vec!["VP8X", "ALPH", "VP8 "]);
        // format étendu : drapeau alpha, canevas de la taille de l'image (64 × 32)
        assert_eq!(top[0].1, &[0x10, 0, 0, 0, 63, 0, 0, 31, 0, 0]);
    }

    #[test]
    fn fichiers_illisibles_ou_tronques() {
        let a = anime(&[(*b"VP8L", &[1, 2, 3])], &[9, 9]);
        assert!(first_frame(&a[..a.len() - 20]).is_none(), "tronqué");
        assert!(first_frame(&[]).is_none());
        let mut faux = a.clone();
        faux[16..20].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0x7F]); // taille du premier morceau absurde
        assert!(first_frame(&faux).is_none());
    }

    #[tokio::test]
    async fn un_webp_anime_reel_donne_une_miniature_avec_ffmpeg() {
        let Some(ff) = crate::importer::ffmpeg_path() else { return };
        let d = std::env::temp_dir().join(format!("repere-webp-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let anim = d.join("anim.webp");
        let ok = std::process::Command::new(&ff)
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc=size=160x120:rate=10:duration=1", "-c:v", "libwebp_anim", "-loop", "0", "-y"])
            .arg(&anim).status().is_ok_and(|s| s.success());
        if !ok { return } // ffmpeg sans encodeur WebP animé : rien à vérifier
        let png = d.join("t.png");
        assert!(!crate::importer::thumbnail(&ff, &anim, &png, crate::importer::Kind::Photo).await, "ffmpeg ne lit pas les WebP animés");
        let still = still_copy(&anim).expect("première image");
        assert!(crate::importer::thumbnail(&ff, &still, &png, crate::importer::Kind::Photo).await, "mais lit sa première image");
        assert!(png.metadata().unwrap().len() > 100);
        std::fs::remove_file(&still).ok();
        std::fs::remove_dir_all(&d).ok();
    }
}
