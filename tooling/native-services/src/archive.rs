//! Archive extraction that refuses to write outside its destination.

use crate::{ServiceResult, error};
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

/// Extracts every regular file and directory of a zip archive into
/// `destination`. Entries with absolute or `..` paths are rejected.
pub fn extract_zip(archive: &Path, destination: &Path) -> ServiceResult<()> {
    let file = File::open(archive)
        .map_err(|io| error(format!("cannot open `{}`: {io}", archive.display())))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|zip| {
        error(format!(
            "`{}` is not a zip archive: {zip}",
            archive.display()
        ))
    })?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|zip| error(format!("cannot read `{}`: {zip}", archive.display())))?;
        let relative = entry.enclosed_name().ok_or_else(|| {
            error(format!(
                "`{}` contains an unsafe path `{}`",
                archive.display(),
                entry.name()
            ))
        })?;
        let target = destination.join(relative);
        if entry.is_dir() {
            create(&target)?;
            continue;
        }
        if entry.is_symlink() {
            return Err(error(format!(
                "`{}` contains a symbolic link `{}`",
                archive.display(),
                entry.name()
            )));
        }
        write(&target, &mut entry)?;
    }
    Ok(())
}

/// Extracts a gzip-compressed tar archive into `destination`, dropping the
/// first `strip` path components (like `tar --strip-components`). Regular
/// files and directories are extracted; links and special files are
/// skipped, since Windows cannot create them without extra privileges and
/// builds do not need them.
pub fn extract_tar_gz(archive: &Path, destination: &Path, strip: usize) -> ServiceResult<()> {
    let file = File::open(archive)
        .map_err(|io| error(format!("cannot open `{}`: {io}", archive.display())))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let entries = tar
        .entries()
        .map_err(|io| error(format!("cannot read `{}`: {io}", archive.display())))?;
    for entry in entries {
        let mut entry =
            entry.map_err(|io| error(format!("cannot read `{}`: {io}", archive.display())))?;
        let path = entry
            .path()
            .map_err(|io| error(format!("bad path in `{}`: {io}", archive.display())))?
            .into_owned();
        let Some(relative) = safe_relative(&path, strip) else {
            if path.components().count() <= strip {
                continue;
            }
            return Err(error(format!(
                "`{}` contains an unsafe path `{}`",
                archive.display(),
                path.display()
            )));
        };
        let target = destination.join(&relative);
        match entry.header().entry_type() {
            tar::EntryType::Directory => create(&target)?,
            tar::EntryType::Regular | tar::EntryType::Continuous => write(&target, &mut entry)?,
            _ => {}
        }
    }
    Ok(())
}

/// The path without its first `strip` components, if what remains is a
/// non-empty path of plain names.
fn safe_relative(path: &Path, strip: usize) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for (index, component) in path.components().enumerate() {
        match component {
            Component::Normal(name) if index >= strip => relative.push(name),
            Component::Normal(_) => {}
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!relative.as_os_str().is_empty()).then_some(relative)
}

fn create(directory: &Path) -> ServiceResult<()> {
    fs::create_dir_all(directory)
        .map_err(|io| error(format!("cannot create `{}`: {io}", directory.display())))
}

fn write(target: &Path, reader: &mut impl io::Read) -> ServiceResult<()> {
    if let Some(parent) = target.parent() {
        create(parent)?;
    }
    let mut output = File::create(target)
        .map_err(|io| error(format!("cannot create `{}`: {io}", target.display())))?;
    io::copy(reader, &mut output)
        .map_err(|io| error(format!("cannot write `{}`: {io}", target.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn relative_paths_strip_leading_components_and_reject_traversal() {
        assert_eq!(
            safe_relative(Path::new("minio-abc/cmd/main.go"), 1),
            Some(PathBuf::from("cmd/main.go"))
        );
        assert_eq!(safe_relative(Path::new("minio-abc"), 1), None);
        assert_eq!(safe_relative(Path::new("minio-abc/../x"), 1), None);
        assert_eq!(safe_relative(Path::new("/etc/passwd"), 0), None);
    }

    #[test]
    fn zip_entries_are_extracted_and_traversal_is_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let good = temporary.path().join("good.zip");
        let mut writer = zip::ZipWriter::new(File::create(&good).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.add_directory("storage/", options).unwrap();
        writer.start_file("storage/a.txt", options).unwrap();
        writer.write_all(b"hello").unwrap();
        writer.finish().unwrap();
        let out = temporary.path().join("out");
        extract_zip(&good, &out).unwrap();
        assert_eq!(fs::read(out.join("storage/a.txt")).unwrap(), b"hello");

        let bad = temporary.path().join("bad.zip");
        let mut writer = zip::ZipWriter::new(File::create(&bad).unwrap());
        writer.start_file("../escape.txt", options).unwrap();
        writer.write_all(b"no").unwrap();
        writer.finish().unwrap();
        assert!(extract_zip(&bad, &temporary.path().join("bad-out")).is_err());
        assert!(!temporary.path().join("escape.txt").exists());
    }

    #[test]
    fn tar_gz_archives_are_stripped_and_extracted() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("source.tar.gz");
        let encoder = flate2::write::GzEncoder::new(
            File::create(&path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "minio-abc/go.mod", &b"go"[..])
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        let out = temporary.path().join("out");
        extract_tar_gz(&path, &out, 1).unwrap();
        assert_eq!(fs::read(out.join("go.mod")).unwrap(), b"go");
    }
}
