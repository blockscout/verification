use super::version::Version;
use async_trait::async_trait;
use bytes::Bytes;
use std::{
    fs::{File, OpenOptions},
    io::ErrorKind,
    os::unix::prelude::OpenOptionsExt,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FetchError {
    #[error("version {0} not found")]
    NotFound(Version),
    #[error("couldn't fetch the file: {0}")]
    Fetch(anyhow::Error),
    #[error("couldn't create file: {0}")]
    File(std::io::Error),
    #[error("tokio sheduling error: {0}")]
    Schedule(tokio::task::JoinError),
}

#[async_trait]
pub trait Fetcher: Send + Sync {
    async fn fetch(&self, ver: &Version) -> Result<PathBuf, FetchError>;
    fn all_versions(&self) -> Vec<Version>;
}

#[cfg(target_family = "unix")]
fn create_executable(path: &Path) -> Result<File, std::io::Error> {
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o777)
        .open(path)
}

pub async fn save_executable(
    data: Bytes,
    path: &Path,
    ver: &Version,
) -> Result<PathBuf, FetchError> {
    let folder = path.join(ver.to_string());
    let file = folder.join("solc");
    let result = file.clone();
    tokio::task::spawn_blocking(move || -> Result<(), FetchError> {
        std::fs::create_dir_all(&folder).map_err(FetchError::File)?;
        std::fs::remove_file(file.as_path())
            .or_else(|e| {
                if e.kind() == ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .map_err(FetchError::File)?;
        let mut file = create_executable(file.as_path()).map_err(FetchError::File)?;
        std::io::copy(&mut data.as_ref(), &mut file).map_err(FetchError::File)?;
        Ok(())
    })
    .await
    .map_err(FetchError::Schedule)??;
    Ok(result)
}

pub fn update_compilers<T: PartialEq>(
    value: &parking_lot::RwLock<T>,
    new: T,
    sizer: impl Fn(&T) -> usize,
) {
    let need_to_update = {
        let old = value.read();
        new != *old
    };
    if need_to_update {
        let (old_len, new_len) = {
            // we don't need to check condition again,
            // we can just override the value
            let mut old = value.write();
            let old_len = sizer(&old);
            let new_len = sizer(&new);
            *old = new;
            (old_len, new_len)
        };
        log::info!(
            "found new compiler versions. old length: {}, new length: {}",
            old_len,
            new_len,
        );
    } else {
        log::info!("no new versions found")
    }
}
