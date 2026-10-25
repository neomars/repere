//! Fichiers de la médiathèque sur le disque : retrouver celui qu'une adresse (« /videos/x.mp4 ») désigne dans le dossier
//! d'uploads, et le supprimer — sans jamais sortir de ce dossier.

use crate::media::{MediaItem, MediaLibrary};
use std::path::{Component, Path, PathBuf};

/// Dossiers d'uploads où vivent les médias et leurs miniatures.
const DIRS: [&str; 3] = ["videos", "photos", "thumbnails"];

/// Le fichier que désigne une adresse de média (`/videos/x.mp4`, `/thumbnails/t.png`), à l'intérieur de `uploads` : `None` pour une
/// adresse qui sort de ces dossiers (`..`, chemin absolu, autre dossier) ou qui ne mène à aucun fichier.
pub fn upload_path(uploads: &Path, url: &str) -> Option<PathBuf> {
    let rel = Path::new(url.strip_prefix('/')?);
    let mut parts = rel.components();
    let Some(Component::Normal(top)) = parts.next() else { return None };
    if !DIRS.iter().any(|d| top == *d) || !rel.components().all(|c| matches!(c, Component::Normal(_))) || rel.components().count() < 2 {
        return None;
    }
    let path = uploads.join(rel);
    // un lien symbolique qui mène ailleurs ne doit pas servir à effacer hors du dossier : on regarde où vit le fichier lui-même
    let parent = path.parent()?.canonicalize().ok()?;
    if !parent.starts_with(uploads.canonicalize().ok()?) {
        return None;
    }
    std::fs::symlink_metadata(&path).ok().filter(|m| m.is_file() || m.file_type().is_symlink()).map(|_| path)
}

/// Résultat d'une suppression de fichiers.
#[derive(Debug, Default, PartialEq)]
pub struct Removed {
    pub removed: usize,
    pub failed: Vec<String>,
}

/// Supprime du disque les fichiers (média et miniature) de ces médias, déjà retirés de la médiathèque. Un fichier encore utilisé par
/// un autre média de la médiathèque reste. Les fichiers importés « par lien » n'effacent pas l'original : seul le lien de l'app part.
pub fn remove_media_files(uploads: &Path, items: &[MediaItem], media: &MediaLibrary) -> Removed {
    let mut out = Removed::default();
    for item in items {
        for url in std::iter::once(item.url.as_str()).chain(item.thumb.as_deref()) {
            if media.url_used(url) {
                continue;
            }
            let Some(path) = upload_path(uploads, url) else { continue };
            match std::fs::remove_file(&path) {
                Ok(()) => out.removed += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {} // déjà parti (par exemple supprimé avec sa scène)
                Err(e) => out.failed.push(format!("{url} : {e}")),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("repere-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for sub in DIRS {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        d
    }

    #[test]
    fn une_adresse_ne_designe_que_des_fichiers_des_dossiers_d_uploads() {
        let up = tmp("path");
        std::fs::write(up.join("videos/a.mp4"), b"x").unwrap();
        std::fs::create_dir_all(up.join("videos/sous")).unwrap();
        std::fs::write(up.join("videos/sous/b.mp4"), b"x").unwrap();
        std::fs::write(up.join("autre.txt"), b"secret").unwrap();
        assert_eq!(upload_path(&up, "/videos/a.mp4"), Some(up.join("videos/a.mp4")));
        assert_eq!(upload_path(&up, "/videos/sous/b.mp4"), Some(up.join("videos/sous/b.mp4")));
        for mauvaise in ["/videos/../autre.txt", "/autre.txt", "autre.txt", "/videos", "/videos/", "/../etc/passwd", "/etc/passwd", "/videos/absent.mp4", "/parts/a.mp4", "", "/"] {
            assert_eq!(upload_path(&up, mauvaise), None, "{mauvaise}");
        }
        #[cfg(unix)]
        {
            // un lien symbolique vers un dossier extérieur ne permet pas d'en effacer le contenu
            let dehors = tmp("dehors");
            std::fs::write(dehors.join("videos/proche.mp4"), b"x").unwrap();
            std::os::unix::fs::symlink(dehors.join("videos"), up.join("videos/lien")).unwrap();
            assert_eq!(upload_path(&up, "/videos/lien/proche.mp4"), None);
            std::fs::remove_dir_all(dehors).ok();
        }
        std::fs::remove_dir_all(up).ok();
    }

    #[test]
    fn suppression_du_media_et_de_sa_miniature_sans_toucher_au_reste() {
        let up = tmp("rm");
        let lib = MediaLibrary::open_in_memory().unwrap();
        for f in ["videos/a.mp4", "thumbnails/thumb-a.png", "photos/p.jpg", "videos/b.mp4"] {
            std::fs::write(up.join(f), b"x").unwrap();
        }
        let item = |url: &str, thumb: Option<&str>| MediaItem { url: url.into(), thumb: thumb.map(String::from), ..Default::default() };
        let a = item("/videos/a.mp4", Some("/thumbnails/thumb-a.png"));
        let p = item("/photos/p.jpg", Some("/thumbnails/absente.png"));
        let hors = item("/etc/passwd", None);
        // « b » est encore dans la médiathèque et partage sa vidéo avec un autre média : elle reste
        lib.insert(&item("/videos/b.mp4", None)).unwrap();
        let b_copie = item("/videos/b.mp4", None);
        let r = remove_media_files(&up, &[a, p, hors, b_copie], &lib);
        assert_eq!(r, Removed { removed: 3, failed: vec![] }, "vidéo + miniature + photo");
        assert!(!up.join("videos/a.mp4").exists() && !up.join("thumbnails/thumb-a.png").exists() && !up.join("photos/p.jpg").exists());
        assert!(up.join("videos/b.mp4").exists(), "encore utilisée");
        std::fs::remove_dir_all(up).ok();
    }
}
