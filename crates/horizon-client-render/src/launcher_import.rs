//! Files the launcher brings in (the iOS document picker, `launcher_ui::Platform::pick_files`):
//! what each one is, from its name and what it holds, and the copy into the game folder where the
//! game looks for that kind. Nothing already there is replaced. Modpacks are only recognized here:
//! installing one fetches its mods from the registry (`game/launcher.rs`).
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// What a file brought in is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A Horizon content mod (`.hmod`): into `horizon-mods/`.
    Mod,
    /// A Horizon modpack (`.hpack`): its overrides and its mods.
    Modpack,
    /// A resource pack (a zip with `pack.mcmeta`): into `resourcepacks/`.
    ResourcePack,
    /// A shader pack (a zip with `shaders/`, or Horizon's own `.hshader`): into `shaderpacks/`.
    ShaderPack,
    /// A saved world (a zip with `level.dat`, at its root or in one folder): unpacked into
    /// `saves/`.
    World,
    /// The on-screen controls' layout (`.json` with the layout's format).
    Layout,
}

impl Kind {
    /// The kind's translation key, for the launcher's notices.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Mod => "horizon.launcher.import.kind.mod",
            Kind::Modpack => "horizon.launcher.import.kind.modpack",
            Kind::ResourcePack => "horizon.launcher.import.kind.resource_pack",
            Kind::ShaderPack => "horizon.launcher.import.kind.shader_pack",
            Kind::World => "horizon.launcher.import.kind.world",
            Kind::Layout => "horizon.launcher.import.kind.layout",
        }
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The folder in a world zip that holds `level.dat`: `""` at its root, else `name/`.
fn world_root(names: &[String]) -> Option<String> {
    names.iter().find_map(|name| {
        let dir = name.strip_suffix("level.dat")?;
        (dir.is_empty() || (dir.ends_with('/') && dir.matches('/').count() == 1))
            .then(|| dir.to_owned())
    })
}

fn zip_names(path: &Path) -> io::Result<Vec<String>> {
    names_of(&zip::ZipArchive::new(fs::File::open(path)?).map_err(io::Error::other)?)
}

fn names_of(archive: &zip::ZipArchive<fs::File>) -> io::Result<Vec<String>> {
    archive
        .file_names()
        .map(|name| name.map(|n| n.into_owned()).map_err(io::Error::other))
        .collect()
}

/// What `path` is, or `None` when the game has no place for it.
pub fn classify(path: &Path) -> Option<Kind> {
    match extension(path).as_str() {
        "hmod" => Some(Kind::Mod),
        "hpack" => Some(Kind::Modpack),
        "hshader" => Some(Kind::ShaderPack),
        "json" => Some(Kind::Layout),
        "zip" => {
            let names = zip_names(path).ok()?;
            let has = |name: &str| names.iter().any(|n| n == name);
            if has("pack.mcmeta") {
                Some(Kind::ResourcePack)
            } else if has("horizon-shaders.toml") || names.iter().any(|n| n.starts_with("shaders/"))
            {
                Some(Kind::ShaderPack)
            } else {
                world_root(&names).map(|_| Kind::World)
            }
        }
        _ => None,
    }
}

/// `dir/name`, or `dir/stem (2).ext` and on when that is taken: an import never replaces.
pub fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| fs::symlink_metadata(p).is_err())
        .expect("a free name")
}

fn copy_into(from: &Path, dir: &Path) -> io::Result<PathBuf> {
    let name = from
        .file_name()
        .ok_or_else(|| io::Error::other("no file name"))?
        .to_string_lossy()
        .into_owned();
    fs::create_dir_all(dir)?;
    let target = free_name(dir, &name);
    fs::copy(from, &target)?;
    Ok(target)
}

/// Unpacks the world zip at `from` into a new folder of `saves`, named after the zip's folder
/// (or the zip itself). Entries outside that folder, or with a path that would leave it, are
/// left out.
fn unpack_world(from: &Path, saves: &Path) -> io::Result<PathBuf> {
    let mut archive = zip::ZipArchive::new(fs::File::open(from)?).map_err(io::Error::other)?;
    let names = names_of(&archive)?;
    let root = world_root(&names).ok_or_else(|| io::Error::other("no level.dat"))?;
    let name = match root.strip_suffix('/') {
        Some(folder) => folder.to_owned(),
        None => from
            .file_stem()
            .map_or_else(|| "World".into(), |s| s.to_string_lossy().into_owned()),
    };
    fs::create_dir_all(saves)?;
    let target = free_name(saves, &name);
    fs::create_dir(&target)?;
    let result = (|| {
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(io::Error::other)?;
            let Some(inside) = entry.enclosed_name() else {
                continue;
            };
            let Ok(rest) = inside.strip_prefix(&root) else {
                continue;
            };
            if rest.as_os_str().is_empty()
                || rest
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
            {
                continue;
            }
            let out = target.join(rest);
            if entry.is_dir() {
                fs::create_dir_all(&out)?;
                continue;
            }
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            io::copy(&mut entry, &mut fs::File::create(&out)?)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(target),
        Err(e) => {
            let _ = fs::remove_dir_all(&target);
            Err(e)
        }
    }
}

/// Copies `from`, of kind `kind`, into the game folder `home`: where it landed.
/// [`Kind::Modpack`] and [`Kind::Layout`] are the caller's.
pub fn import(from: &Path, kind: Kind, home: &Path) -> io::Result<PathBuf> {
    match kind {
        Kind::Mod => copy_into(from, &home.join(crate::mod_list::DIR)),
        Kind::ResourcePack => {
            let dir = home.join("resourcepacks");
            crate::packs::copy_pack(from, &dir)?;
            Ok(dir.join(from.file_name().unwrap_or_default()))
        }
        Kind::ShaderPack => copy_into(from, &home.join(crate::shader_pack_list::DIR)),
        Kind::World => unpack_world(from, &home.join("saves")),
        Kind::Modpack | Kind::Layout => Err(io::Error::other("not a copy")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut out = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, bytes) in entries {
            out.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            out.write_all(bytes).unwrap();
        }
        out.finish().unwrap();
    }

    #[test]
    fn a_picked_zip_lands_where_its_kind_belongs() {
        let dir = std::env::temp_dir().join(format!("horizon-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let home = dir.join("home");
        fs::create_dir_all(&home).unwrap();

        let pack = dir.join("Faithful.zip");
        zip(&pack, &[("pack.mcmeta", b"{}"), ("assets/x.png", b"")]);
        let shaders = dir.join("Lights.zip");
        zip(&shaders, &[("shaders/final.fsh", b"")]);
        let world = dir.join("download.zip");
        zip(
            &world,
            &[
                ("My World/level.dat", b"level"),
                ("My World/region/r.0.0.mca", b"region"),
                ("../escape.txt", b"no"),
                ("Other/stray.txt", b"no"),
            ],
        );
        let notes = dir.join("notes.zip");
        zip(&notes, &[("readme.txt", b"")]);

        assert_eq!(classify(&pack), Some(Kind::ResourcePack));
        assert_eq!(classify(&shaders), Some(Kind::ShaderPack));
        assert_eq!(classify(&world), Some(Kind::World));
        assert_eq!(classify(&notes), None);

        import(&pack, Kind::ResourcePack, &home).unwrap();
        assert!(home.join("resourcepacks/Faithful.zip").is_file());
        // A second copy of the same shader pack keeps the first.
        import(&shaders, Kind::ShaderPack, &home).unwrap();
        let second = import(&shaders, Kind::ShaderPack, &home).unwrap();
        assert_eq!(second, home.join("shaderpacks/Lights (2).zip"));
        assert!(home.join("shaderpacks/Lights.zip").is_file());

        let saved = import(&world, Kind::World, &home).unwrap();
        assert_eq!(saved, home.join("saves/My World"));
        assert_eq!(fs::read(saved.join("level.dat")).unwrap(), b"level");
        assert!(saved.join("region/r.0.0.mca").is_file());
        assert!(!saved.join("stray.txt").exists() && !dir.join("escape.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
