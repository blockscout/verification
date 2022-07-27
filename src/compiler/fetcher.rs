use crate::types::Mismatch;

use super::version::Version;
use crate::scheduler;
use async_trait::async_trait;
use bytes::Bytes;
use cron::Schedule;
use parking_lot::{
    lock_api::{RwLock, RwLockReadGuard, RwLockWriteGuard},
    RawRwLock,
};
use len_trait::Len;
use primitive_types::H256;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::ErrorKind,
    os::unix::prelude::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FetchError {
    #[error("version {0} not found")]
    NotFound(Version),
    #[error("couldn't fetch the file: {0}")]
    Fetch(#[from] anyhow::Error),
    #[error("hashsum of fetched file mismatch: {0}")]
    HashMismatch(#[from] Mismatch<H256>),
    #[error("couldn't create file: {0}")]
    File(#[from] std::io::Error),
    #[error("tokio sheduling error: {0}")]
    Schedule(#[from] tokio::task::JoinError),
}

#[async_trait]
pub trait Fetcher: Send + Sync {
    async fn fetch(&self, ver: &Version) -> Result<PathBuf, FetchError>;
    fn all_versions(&self) -> Vec<Version>;
}

#[async_trait]
pub trait VersionsFetcher: Send + Sync + 'static {
    type Response;
    type Error: std::fmt::Display;

    async fn fetch_versions(&self) -> Result<Self::Response, Self::Error>;
}

#[derive(Debug, Clone, Default)]
pub struct RefreshableVersions<T> {
    versions: Arc<parking_lot::RwLock<T>>,
}

impl<T> RefreshableVersions<T> {
    pub fn new(inner: T) -> Self {
        RefreshableVersions {
            versions: Arc::new(RwLock::new(inner)),
        }
    }

    pub fn read(&self) -> RwLockReadGuard<'_, RawRwLock, T> {
        self.versions.read()
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, RawRwLock, T> {
        self.versions.write()
    }

    pub fn spawn_refresh_job<Fetcher>(
        self,
        fetcher: Fetcher,
        cron_schedule: Schedule,
    ) where
        T: Clone + PartialEq + Send + Sync + 'static + Len,
        Fetcher: VersionsFetcher<Response = T> + Clone,
    {
        log::info!("spawn version refresh job");
        scheduler::spawn_job(cron_schedule, "refresh compiler version", move || {
            let versions = self.clone();
            let fetcher = fetcher.clone();
            async move {
                log::info!("looking for new compilers versions");
                let refresh_result = fetcher.fetch_versions().await;
                match refresh_result {
                    Ok(fetched_versions) => {
                        versions.update_versions(fetched_versions);
                    }
                    Err(err) => {
                        log::error!("error during version refresh: {}", err);
                    }
                }
            }
        });
    }

    fn update_versions(&self, new: T)
    where
        T: PartialEq + Len,
    {
        let need_to_update = {
            let old = self.read();
            new != *old
        };
        if need_to_update {
            let (old_len, new_len) = {
                // we don't need to check condition again,
                // we can just override the value
                let mut old = self.write();
                let old_len = old.len();
                let new_len = new.len();
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

pub fn check_hashsum(bytes: &Bytes, expected: H256) -> Result<(), Mismatch<H256>> {
    let start = std::time::Instant::now();

    let found = Sha256::digest(bytes);
    let found = H256::from_slice(&found);

    let took = std::time::Instant::now() - start;
    // TODO: change to tracing
    log::debug!("check hashsum of {} bytes took {:?}", bytes.len(), took,);
    if expected != found {
        Err(Mismatch::new(expected, found))
    } else {
        Ok(())
    }
}

pub async fn write_executable(
    data: Bytes,
    sha: H256,
    path: &Path,
    ver: &Version,
) -> Result<PathBuf, FetchError> {
    let folder = path.join(ver.to_string());
    let file = folder.join("solc");

    let save_result = {
        let file = file.clone();
        let data = data.clone();
        tokio::task::spawn_blocking(move || -> Result<(), FetchError> {
            std::fs::create_dir_all(&folder)?;
            std::fs::remove_file(file.as_path()).or_else(|e| {
                if e.kind() == ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(e)
                }
            })?;
            let mut file = create_executable(file.as_path())?;
            std::io::copy(&mut data.as_ref(), &mut file)?;
            Ok(())
        })
    };
    let check_result = tokio::task::spawn_blocking(move || check_hashsum(&data, sha));

    let (check_result, save_result) = futures::join!(check_result, save_result);
    check_result??;
    save_result??;

    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[tokio::test]
    async fn save_text() {
        let tmp_dir = std::env::temp_dir();
        let data = "this is a compiler binary";
        let bytes = Bytes::from_static(data.as_bytes());
        let sha = Sha256::digest(data.as_bytes());
        let version = Version::from_str("v0.4.10+commit.f0d539ae").unwrap();
        let file = write_executable(bytes, H256::from_slice(&sha), &tmp_dir, &version)
            .await
            .unwrap();
        let content = tokio::fs::read_to_string(file).await.unwrap();
        assert_eq!(data, content);
    }
}
