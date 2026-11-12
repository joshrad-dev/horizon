//! `std::fs`'s reading calls, seeing the files of a packed prepared folder
//! (`prepared_files.rs`, version 3: the iOS app's) and of a resource packs'
//! mirror over one. Everywhere else each call is exactly its `std::fs` namesake.
//! Code that reads the prepared folder, or a resource pack's folder (which it
//! can be), reads through these.
//!
//! Under a packed root a file comes from its one file, except what that leaves
//! on the disk beside it (the sounds, the built-in packs' zips). Under a mirror
//! ([`crate::prepared_files::open_overlay`]) a file on the mirror's disk (a
//! pack's) comes first, then the packed folder's unless the packs hide it.
use crate::prepared_files::{Found, PreparedFiles, lookup};
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Where a file's bytes are.
enum Located {
    /// On the disk; `true` under a registered root, where it may well not be.
    Disk(PathBuf, bool),
    Packed(Arc<PreparedFiles>, String),
    Missing,
}

/// Whether `path` is on the disk, and a folder: looked at with `lstat` first, so a path
/// that isn't there never reaches `stat` (`tools/ios`'s tracing tells a missed read by
/// that), then followed when it is a link (a mirror's links to a pack's files).
fn on_disk(path: &Path) -> Option<bool> {
    let meta = fs::symlink_metadata(path).ok()?;
    if meta.is_symlink() {
        return fs::metadata(path).ok().map(|m| m.is_dir());
    }
    Some(meta.is_dir())
}

/// `HORIZON_RESOURCE_FS_TRACE=1`: each file asked for under a registered root and not found
/// there goes to stderr, for finding what a packed folder lacks.
fn traced(path: &Path) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("HORIZON_RESOURCE_FS_TRACE").is_some()) {
        eprintln!("resource_fs: missing {}", path.display());
    }
}

fn locate(path: &Path) -> Located {
    let Some(Found {
        files,
        rel,
        overlay,
    }) = lookup(path)
    else {
        return Located::Disk(path.to_owned(), false);
    };
    match overlay {
        None if files.holds_file(&rel) => Located::Packed(files, rel),
        None => Located::Disk(path.to_owned(), true),
        Some(overlay) => {
            if on_disk(path) == Some(false) {
                Located::Disk(path.to_owned(), true)
            } else if overlay.hidden.contains(&rel) {
                Located::Missing
            } else if files.holds_file(&rel) {
                Located::Packed(files, rel)
            } else {
                Located::Disk(overlay.vanilla.join(&rel), true)
            }
        }
    }
}

/// [`std::fs::read`].
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    match locate(path.as_ref()) {
        Located::Disk(path, true) if on_disk(&path).is_none() => {
            Err(io::ErrorKind::NotFound.into())
        }
        Located::Disk(path, _) => fs::read(path),
        Located::Packed(files, rel) => files.read(&rel).unwrap_or_else(|| {
            traced(path.as_ref());
            Err(io::ErrorKind::NotFound.into())
        }),
        Located::Missing => {
            traced(path.as_ref());
            Err(io::ErrorKind::NotFound.into())
        }
    }
}

/// [`std::fs::read_to_string`].
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// [`Path::is_file`].
pub fn is_file(path: impl AsRef<Path>) -> bool {
    let found = match locate(path.as_ref()) {
        Located::Disk(path, true) => on_disk(&path) == Some(false),
        Located::Disk(path, false) => return path.is_file(),
        Located::Packed(files, rel) => files.entry(&rel).is_some(),
        Located::Missing => false,
    };
    if !found {
        traced(path.as_ref());
    }
    found
}

/// [`Path::is_dir`].
pub fn is_dir(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    match lookup(path) {
        None => path.is_dir(),
        Some(found) => {
            found.rel.is_empty()
                || found.files.children(&found.rel).is_some()
                // A file the packed folder has is no folder, wherever it is.
                || (found.files.entry(&found.rel).is_none()
                    && (on_disk(path) == Some(true)
                        || found
                            .overlay
                            .is_some_and(|o| on_disk(&o.vanilla.join(&found.rel)) == Some(true))))
        }
    }
}

/// [`Path::exists`].
pub fn exists(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    is_file(path) || is_dir(path)
}

/// [`std::fs::metadata`] (following links, as it does).
pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    let path = path.as_ref();
    let packed_time = |files: &PreparedFiles| files.file.metadata().and_then(|m| m.modified()).ok();
    match locate(path) {
        Located::Packed(files, rel) => {
            if let Some(len) = files.len(&rel) {
                return Ok(Metadata {
                    dir: false,
                    len,
                    modified: packed_time(&files),
                });
            }
        }
        Located::Disk(at, registered) => {
            if (!registered || on_disk(&at).is_some())
                && let Ok(meta) = fs::metadata(&at)
            {
                return Ok(Metadata {
                    dir: meta.is_dir(),
                    len: meta.len(),
                    modified: meta.modified().ok(),
                });
            }
        }
        Located::Missing => return Err(io::ErrorKind::NotFound.into()),
    }
    match lookup(path) {
        Some(found) if is_dir(path) => Ok(Metadata {
            dir: true,
            len: 0,
            modified: packed_time(&found.files),
        }),
        Some(_) => Err(io::ErrorKind::NotFound.into()),
        None => fs::metadata(path).map(|meta| Metadata {
            dir: meta.is_dir(),
            len: meta.len(),
            modified: meta.modified().ok(),
        }),
    }
}

/// The disk's entries of `dir` that `keep` keeps, as (name, is a folder); none when it isn't there.
fn disk_children(dir: &Path, keep: impl Fn(&str, bool) -> bool) -> io::Result<Vec<(String, bool)>> {
    if on_disk(dir) != Some(true) {
        return Ok(Vec::new());
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut children = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        // Following links, as the mirror's are.
        let folder = entry.path().is_dir();
        if keep(&name, folder) {
            children.push((name, folder));
        }
    }
    Ok(children)
}

/// What is directly in `dir` under a registered root, as (name, is a folder).
fn registered_children(dir: &Path, found: &Found) -> io::Result<Vec<(String, bool)>> {
    let child = |name: &str| {
        if found.rel.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{name}", found.rel)
        }
    };
    let hidden = |name: &str| {
        found
            .overlay
            .as_ref()
            .is_some_and(|o| o.hidden.contains(&child(name)))
    };
    let mut children: Vec<(String, bool)> = found
        .files
        .children(&found.rel)
        .unwrap_or_default()
        .into_iter()
        .filter(|(name, folder)| *folder || !hidden(name))
        .map(|(name, folder)| (name.to_owned(), folder))
        .collect();
    let mut add = |more: Vec<(String, bool)>| {
        for (name, folder) in more {
            if !children.iter().any(|(n, _)| *n == name) {
                children.push((name, folder));
            }
        }
    };
    match &found.overlay {
        // The packed folder's disk: what its file leaves there.
        None => add(disk_children(dir, |name, folder| {
            folder || !found.files.holds_file(&child(name))
        })?),
        // A mirror's disk (the packs' files), then what the packed folder leaves on its own.
        Some(overlay) => {
            add(disk_children(dir, |name, _| {
                name != crate::prepared_files::OVERLAY_FILE
            })?);
            add(disk_children(
                &overlay.vanilla.join(&found.rel),
                |name, folder| folder || (!found.files.holds_file(&child(name)) && !hidden(name)),
            )?);
        }
    }
    if children.is_empty() && !found.rel.is_empty() && !is_dir(dir) {
        return Err(io::ErrorKind::NotFound.into());
    }
    children.sort();
    Ok(children)
}

/// [`std::fs::read_dir`], with the same methods on its entries (`path`,
/// `file_name`, `file_type`, `metadata`).
pub fn read_dir(dir: impl AsRef<Path>) -> io::Result<ReadDir> {
    let dir = dir.as_ref();
    let entries: Vec<io::Result<DirEntry>> = match lookup(dir) {
        None => fs::read_dir(dir)?
            .map(|entry| {
                let entry = entry?;
                let kind = entry.file_type()?;
                Ok(DirEntry {
                    path: entry.path(),
                    kind: FileType {
                        dir: kind.is_dir(),
                        symlink: kind.is_symlink(),
                    },
                })
            })
            .collect(),
        Some(found) => registered_children(dir, &found)?
            .into_iter()
            .map(|(name, folder)| {
                Ok(DirEntry {
                    path: dir.join(name),
                    kind: FileType {
                        dir: folder,
                        symlink: false,
                    },
                })
            })
            .collect(),
    };
    Ok(ReadDir(entries.into_iter()))
}

/// The files under `dir` and its subfolders, as `/`-separated paths from `dir`
/// in name order, when `dir` is under a registered root; `None` elsewhere (walk
/// the folder). Under a packed root these are the files its file holds; under a
/// mirror, those not hidden and the mirror's own.
pub fn files_under(dir: &Path) -> Option<Vec<String>> {
    let found = lookup(dir)?;
    let skip = if found.rel.is_empty() {
        0
    } else {
        found.rel.len() + 1
    };
    let mut names: BTreeSet<String> = found
        .files
        .list(&found.rel)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| {
            found
                .overlay
                .as_ref()
                .is_none_or(|o| !o.hidden.contains(*name))
        })
        .map(|name| name[skip..].to_owned())
        .collect();
    if found.overlay.is_some() {
        fn walk(dir: &Path, prefix: &str, out: &mut BTreeSet<String>) {
            for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                let rel = format!("{prefix}{name}");
                if entry.path().is_dir() {
                    walk(&entry.path(), &format!("{rel}/"), out);
                } else if name != crate::prepared_files::OVERLAY_FILE {
                    out.insert(rel);
                }
            }
        }
        walk(dir, "", &mut names);
    }
    Some(names.into_iter().collect())
}

/// The entries of a folder ([`read_dir`]).
pub struct ReadDir(std::vec::IntoIter<io::Result<DirEntry>>);
impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

/// A file or folder in a folder ([`read_dir`]).
pub struct DirEntry {
    path: PathBuf,
    kind: FileType,
}
impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }
    pub fn file_name(&self) -> std::ffi::OsString {
        self.path.file_name().unwrap_or_default().to_owned()
    }
    pub fn file_type(&self) -> io::Result<FileType> {
        Ok(self.kind)
    }
    pub fn metadata(&self) -> io::Result<Metadata> {
        metadata(&self.path)
    }
}

/// Whether a [`DirEntry`] is a folder, a file or a link (not followed, as `std`'s).
#[derive(Clone, Copy, Debug)]
pub struct FileType {
    dir: bool,
    symlink: bool,
}
impl FileType {
    pub fn is_dir(&self) -> bool {
        self.dir && !self.symlink
    }
    pub fn is_file(&self) -> bool {
        !self.dir && !self.symlink
    }
    pub fn is_symlink(&self) -> bool {
        self.symlink
    }
}

/// What [`metadata`] tells of a file or folder.
#[derive(Clone, Copy, Debug)]
pub struct Metadata {
    dir: bool,
    len: u64,
    modified: Option<std::time::SystemTime>,
}
impl Metadata {
    pub fn is_dir(&self) -> bool {
        self.dir
    }
    pub fn is_file(&self) -> bool {
        !self.dir
    }
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// For a file in a packed root: when its file was written.
    pub fn modified(&self) -> io::Result<std::time::SystemTime> {
        self.modified
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "no modification time"))
    }
}
