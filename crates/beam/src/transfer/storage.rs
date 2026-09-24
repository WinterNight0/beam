//! Filesystem concerns: free space, which volume a directory is on, and
//! committing a finished file across a volume boundary.

use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;

/// Headroom demanded on top of the bytes a transfer actually needs.
///
/// Filling a disk exactly is a bad outcome for everything else running on the
/// machine, and the size a transfer reports is not the only thing that grows
/// while it runs.
pub const SPACE_MARGIN: u64 = 10 * 1024 * 1024;

/// Why a transfer was refused before it started.
#[derive(Debug, thiserror::Error)]
pub enum SpaceError {
    #[error(
        "not enough room on the drive holding {where_}: {needed} needed, {available} available"
    )]
    Insufficient {
        where_: String,
        needed: String,
        available: String,
    },
    #[error("could not check the free space on {path}: {source}")]
    Unavailable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Bytes free on the volume holding `dir`.
pub fn available_space(dir: &Path) -> Result<u64, SpaceError> {
    fs4::available_space(dir).map_err(|source| SpaceError::Unavailable {
        path: dir.to_path_buf(),
        source,
    })
}

/// Identifies the volume a directory sits on.
///
/// Used to tell whether committing a finished file will be a rename or a copy,
/// and whether the destination needs room of its own.
fn volume_id(dir: &Path) -> std::io::Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Ok(std::fs::metadata(dir)?.dev().to_string())
    }
    #[cfg(windows)]
    {
        // The prefix of a canonical path is the volume: `\\?\C:` or a UNC
        // share. Comparing those is enough to know whether a rename can work.
        let canonical = std::fs::canonicalize(dir)?;
        Ok(canonical
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().to_uppercase())
            .unwrap_or_default())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = dir;
        Ok(String::new())
    }
}

/// Whether two directories are on the same volume.
///
/// A failure to tell is reported as "not the same", which is the conservative
/// answer: it asks for more free space and uses the copy path, both of which
/// are safe if wrong.
pub fn same_volume(a: &Path, b: &Path) -> bool {
    match (volume_id(a), volume_id(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Checks there is room for a transfer before anyone is asked to accept it.
///
/// `remaining` is what is still to be downloaded, so a resume only has to
/// justify the part it has not got yet. `total` is the whole file, which is
/// what the destination needs if it is on a different volume and the finished
/// file has to be copied there.
pub fn check_space(
    partial_dir: &Path,
    destination_dir: &Path,
    remaining: u64,
    total: u64,
) -> Result<(), SpaceError> {
    let partial_available = available_space(partial_dir)?;
    let partial_needed = remaining.saturating_add(SPACE_MARGIN);
    if partial_available < partial_needed {
        return Err(SpaceError::Insufficient {
            where_: format!("partial transfers ({})", partial_dir.display()),
            needed: crate::ui::format_bytes(partial_needed),
            available: crate::ui::format_bytes(partial_available),
        });
    }

    // On one volume the commit is a rename, so the finished file costs nothing
    // beyond the bytes already counted above.
    if same_volume(partial_dir, destination_dir) {
        return Ok(());
    }

    let destination_available = available_space(destination_dir)?;
    let destination_needed = total.saturating_add(SPACE_MARGIN);
    if destination_available < destination_needed {
        return Err(SpaceError::Insufficient {
            where_: format!("the destination ({})", destination_dir.display()),
            needed: crate::ui::format_bytes(destination_needed),
            available: crate::ui::format_bytes(destination_available),
        });
    }

    Ok(())
}

/// Moves a finished file into place.
///
/// A rename is atomic and free, but only within one volume, and `--out` on
/// another drive is ordinary on Windows. When the rename cannot cross, the file
/// is copied into a temporary file **inside the destination directory**,
/// flushed to disk, and renamed from there — so the last step is still a rename
/// within one volume, and a crash part-way through leaves a temporary file
/// rather than a half-written download wearing the destination's name.
pub async fn commit(part: &Path, destination: &Path) -> std::io::Result<()> {
    match tokio::fs::rename(part, destination).await {
        Ok(()) => Ok(()),
        Err(e) if is_cross_volume(&e) => commit_by_copy(part, destination).await,
        Err(e) => Err(e),
    }
}

/// The copy path of [`commit`], separated so it can be tested on a machine with
/// only one volume.
pub async fn commit_by_copy(part: &Path, destination: &Path) -> std::io::Result<()> {
    let directory = destination.parent().unwrap_or_else(|| Path::new("."));

    let staging = tempfile::Builder::new()
        .prefix(".beam-commit-")
        .tempfile_in(directory)?;
    let staging_path = staging.path().to_path_buf();
    // The bytes go through tokio, so hand the file over and keep the handle
    // only to control where it ends up.
    let (handle, staging_path_owned) = staging.keep()?;
    debug_assert_eq!(staging_path, staging_path_owned);

    let copy_result = async {
        let mut source = tokio::fs::File::open(part).await?;
        let mut target = tokio::fs::File::from_std(handle);
        tokio::io::copy(&mut source, &mut target).await?;
        target.flush().await?;
        // Durable before the rename, so the destination name never points at
        // bytes that are still only in a cache.
        target.sync_all().await?;
        Ok::<(), std::io::Error>(())
    }
    .await;

    if let Err(e) = copy_result {
        let _ = tokio::fs::remove_file(&staging_path_owned).await;
        return Err(e);
    }

    if let Err(e) = tokio::fs::rename(&staging_path_owned, destination).await {
        let _ = tokio::fs::remove_file(&staging_path_owned).await;
        return Err(e);
    }

    let _ = tokio::fs::remove_file(part).await;
    Ok(())
}

/// Whether an error means "these paths are on different volumes".
fn is_cross_volume(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::CrossesDevices {
        return true;
    }
    // Belt and braces: EXDEV on Unix, ERROR_NOT_SAME_DEVICE on Windows, in case
    // a platform reports one without the mapped kind.
    match e.raw_os_error() {
        #[cfg(unix)]
        Some(18) => true,
        #[cfg(windows)]
        Some(17) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_is_on_the_same_volume_as_itself() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).expect("create a");
        std::fs::create_dir_all(&b).expect("create b");
        assert!(same_volume(&a, &b));
    }

    #[test]
    fn a_path_that_does_not_exist_is_treated_as_a_different_volume() {
        // Not knowing must fall on the safe side: ask for more space, and copy
        // rather than rename.
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(!same_volume(tmp.path(), &tmp.path().join("nowhere")));
    }

    #[test]
    fn free_space_is_reported_for_a_real_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let space = available_space(tmp.path()).expect("available space");
        assert!(
            space > 0,
            "a writable temp directory reported no free space"
        );
    }

    #[test]
    fn a_transfer_that_fits_is_allowed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out).expect("create out");
        check_space(tmp.path(), &out, 1024, 1024).expect("a 1 KiB transfer should fit");
    }

    #[test]
    fn a_transfer_larger_than_the_disk_is_refused_by_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out).expect("create out");

        let error = check_space(tmp.path(), &out, u64::MAX / 2, u64::MAX / 2)
            .expect_err("half the address space should not fit");
        let message = error.to_string();
        assert!(
            message.contains("not enough room") && message.contains("available"),
            "unhelpful message: {message}"
        );
    }

    #[test]
    fn only_the_missing_bytes_have_to_fit() {
        // A resume that needs one more kilobyte must not be refused because the
        // whole file would not fit again.
        let tmp = tempfile::tempdir().expect("tempdir");
        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out).expect("create out");
        assert!(
            check_space(tmp.path(), &out, 1024, u64::MAX / 2).is_ok(),
            "a nearly finished transfer was refused over the size it already has"
        );
    }

    #[tokio::test]
    async fn commit_moves_the_file_into_place() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let part = tmp.path().join("part");
        let destination = tmp.path().join("done.bin");
        std::fs::write(&part, b"contents").expect("write part");

        commit(&part, &destination).await.expect("commit");

        assert_eq!(std::fs::read(&destination).expect("read"), b"contents");
        assert!(!part.exists(), "the part file was left behind");
    }

    #[tokio::test]
    async fn the_copy_path_produces_the_same_result() {
        // This is the path taken when --out is on another drive. It is tested
        // directly because a second volume cannot be assumed; see the manual
        // step in docs/test-plan.md.
        let tmp = tempfile::tempdir().expect("tempdir");
        let part = tmp.path().join("part");
        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out).expect("create out");
        let destination = out.join("done.bin");

        let payload: Vec<u8> = (0..300_000).map(|i| (i % 251) as u8).collect();
        std::fs::write(&part, &payload).expect("write part");

        commit_by_copy(&part, &destination).await.expect("commit");

        assert_eq!(std::fs::read(&destination).expect("read"), payload);
        assert!(!part.exists(), "the part file was left behind");
    }

    #[tokio::test]
    async fn the_copy_path_replaces_a_reserved_placeholder() {
        // The receiver reserves the destination name with create_new before
        // committing, so the copy path has to land on top of an existing empty
        // file rather than refusing.
        let tmp = tempfile::tempdir().expect("tempdir");
        let part = tmp.path().join("part");
        let destination = tmp.path().join("done.bin");
        std::fs::write(&part, b"real contents").expect("write part");
        std::fs::write(&destination, b"").expect("reserve the name");

        commit_by_copy(&part, &destination).await.expect("commit");
        assert_eq!(std::fs::read(&destination).expect("read"), b"real contents");
    }

    #[tokio::test]
    async fn a_failed_copy_leaves_no_rubbish_in_the_destination() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out).expect("create out");
        let missing_part = tmp.path().join("not-there");

        assert!(
            commit_by_copy(&missing_part, &out.join("done.bin"))
                .await
                .is_err()
        );

        let leftovers: Vec<_> = std::fs::read_dir(&out)
            .expect("read out")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            leftovers.is_empty(),
            "the failed copy left {leftovers:?} behind"
        );
    }
}
