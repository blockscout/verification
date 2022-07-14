use super::{fetcher::FetchError, Fetcher, Version};
use crate::{compiler::fetcher::update_compilers, scheduler};
use async_trait::async_trait;
use cron::Schedule;
use s3::Bucket;
use std::{collections::HashSet, path::PathBuf, str::FromStr, sync::Arc};
use thiserror::Error;
use tokio::task::JoinHandle;

#[derive(Error, Debug)]
enum ListError {
    #[error("listing s3 directory failed: {0}")]
    Fetch(s3::error::S3Error),
}

#[derive(Default, Clone)]
struct Versions(Arc<parking_lot::RwLock<HashSet<Version>>>);

impl Versions {
    fn spawn_refresh_job(self, bucket: Arc<Bucket>, cron_schedule: Schedule) {
        log::info!("spawn version refresh job");
        scheduler::spawn_job(cron_schedule, "refresh compiler version", move || {
            let bucket = bucket.clone();
            let versions = self.clone();
            async move {
                log::info!("looking for new compilers versions");
                let refresh_result = Self::fetch_versions(&bucket).await;
                match refresh_result {
                    Ok(fetched_versions) => {
                        update_compilers(&versions.0, fetched_versions, |list| list.len());
                    }
                    Err(err) => {
                        log::error!("error during version refresh: {}", err);
                    }
                }
            }
        });
    }

    async fn fetch_versions(bucket: &Bucket) -> Result<HashSet<Version>, ListError> {
        let folders = bucket
            .list("".to_string(), Some("/".to_string()))
            .await
            .map_err(ListError::Fetch)?;

        let fetched_versions = folders
            .into_iter()
            .filter_map(|x| x.common_prefixes)
            .flatten()
            .filter_map(|x| Version::from_str(&x.prefix).ok())
            .collect();

        Ok(fetched_versions)
    }
}

pub struct S3Fetcher {
    bucket: Arc<Bucket>,
    folder: PathBuf,
    versions: Versions,
}

impl S3Fetcher {
    pub async fn new(
        bucket: Arc<Bucket>,
        folder: PathBuf,
        refresh_schedule: Option<Schedule>,
    ) -> anyhow::Result<S3Fetcher> {
        let versions = Versions::fetch_versions(&bucket).await?;
        let versions = Versions(Arc::new(parking_lot::RwLock::new(versions)));
        if let Some(cron_schedule) = refresh_schedule {
            versions
                .clone()
                .spawn_refresh_job(bucket.clone(), cron_schedule)
        }
        Ok(S3Fetcher {
            bucket,
            folder,
            versions,
        })
    }
}

fn spawn_fetch_s3(
    bucket: Arc<Bucket>,
    path: PathBuf,
) -> JoinHandle<Result<(Vec<u8>, u16), FetchError>> {
    tokio::spawn(async move {
        bucket
            .get_object(path.to_str().unwrap())
            .await
            .map_err(anyhow::Error::msg)
            .map_err(FetchError::Fetch)
    })
}

fn status_code_error(name: &str, status_code: u16) -> FetchError {
    FetchError::Fetch(anyhow::anyhow!(
        "s3 returned non 200 status code while fetching {}: {}",
        name,
        status_code
    ))
}

#[async_trait]
impl Fetcher for S3Fetcher {
    async fn fetch(&self, ver: &Version) -> Result<PathBuf, FetchError> {
        {
            let versions = self.versions.0.read();
            if !versions.contains(ver) {
                return Err(FetchError::NotFound(ver.clone()));
            }
        }

        let folder = PathBuf::from(ver.to_string());
        let data = spawn_fetch_s3(self.bucket.clone(), folder.join("solc"));
        let hash = spawn_fetch_s3(self.bucket.clone(), folder.join("sha256.hash"));
        let (hash, status_code) = hash.await.map_err(FetchError::Schedule)??;
        if status_code != 200 {
            return Err(status_code_error("hash data", status_code));
        }
        let (data, status_code) = data.await.map_err(FetchError::Schedule)??;
        if status_code != 200 {
            return Err(status_code_error("executable file", status_code));
        }
        // TODO: use hash
        let _ = hash;
        super::fetcher::save_executable(data.into(), &self.folder, ver).await
    }

    fn all_versions(&self) -> Vec<Version> {
        let versions = self.versions.0.read();
        versions.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use s3::{creds::Credentials, Region};
}
