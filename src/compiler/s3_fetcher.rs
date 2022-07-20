use super::{fetcher::FetchError, Fetcher, Version};
use crate::{compiler::fetcher::update_compilers, scheduler};
use async_trait::async_trait;
use bytes::Bytes;
use cron::Schedule;
use primitive_types::H256;
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

    async fn fetch_file(&self, ver: &Version) -> Result<(Bytes, H256), FetchError> {
        {
            let versions = self.versions.0.read();
            if !versions.contains(ver) {
                return Err(FetchError::NotFound(ver.clone()));
            }
        }

        let folder = PathBuf::from(ver.to_string());
        let data = spawn_fetch_s3(self.bucket.clone(), folder.join("solc"));
        let hash = spawn_fetch_s3(self.bucket.clone(), folder.join("sha256.hash"));
        let (data, hash) = futures::join!(data, hash);
        let (hash, status_code) = hash??;
        if status_code != 200 {
            return Err(status_code_error("hash data", status_code));
        }
        let (data, status_code) = data??;
        if status_code != 200 {
            return Err(status_code_error("executable file", status_code));
        }
        Ok((data.into(), H256::from_slice(&hash)))
    }
}

#[async_trait]
impl Fetcher for S3Fetcher {
    async fn fetch(&self, ver: &Version) -> Result<PathBuf, FetchError> {
        let (data, hash) = self.fetch_file(ver).await?;
        super::fetcher::save_executable(data, hash, &self.folder, ver).await
    }

    fn all_versions(&self) -> Vec<Version> {
        let versions = self.versions.0.read();
        versions.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use s3::{creds::Credentials, Region};
    use serde::Serialize;
    use sha2::{Digest, Sha256};
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    fn mock_get_object(p: &str, obj: &[u8]) -> Mock {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(obj))
    }

    #[derive(Serialize)]
    struct Prefix {
        #[serde(rename = "Prefix")]
        prefix: String,
    }

    #[derive(Serialize)]
    struct ListBucketResult {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "Prefix")]
        prefix: String,
        #[serde(rename = "IsTruncated")]
        is_truncated: bool,
        #[serde(rename = "CommonPrefixes", default)]
        common_prefixes: Vec<Prefix>,
    }

    fn mock_list_objects(p: &str, prefixes: impl Iterator<Item = String>) -> Mock {
        let value = ListBucketResult {
            name: p.into(),
            prefix: p.into(),
            is_truncated: false,
            common_prefixes: prefixes.map(|prefix| Prefix { prefix }).collect(),
        };
        let data = quick_xml::se::to_string(&value).unwrap();
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_string(data))
    }

    fn test_bucket(endpoint: String) -> Arc<Bucket> {
        let region = Region::Custom {
            region: "".into(),
            endpoint,
        };
        Arc::new(
            Bucket::new(
                "solc-releases",
                region,
                Credentials::new(Some(""), Some(""), None, None, None).unwrap(),
            )
            .unwrap()
            .with_path_style(), // for local testing
        )
    }

    #[tokio::test]
    async fn fetch_file() {
        let expected_file = "this is 100% a valid compiler trust me";
        let expected_hash = Sha256::digest(&expected_file);

        let version = Version::from_str("v0.4.10+commit.f0d539ae").unwrap();

        let mock_server = MockServer::start().await;

        mock_get_object(
            "/solc-releases/v0.4.10%2Bcommit.f0d539ae/solc",
            expected_file.as_bytes(),
        )
        .mount(&mock_server)
        .await;

        mock_get_object(
            "/solc-releases/v0.4.10%2Bcommit.f0d539ae/sha256.hash",
            &expected_hash,
        )
        .mount(&mock_server)
        .await;

        // create type directly to avoid extra work in constructor
        let fetcher = S3Fetcher {
            bucket: test_bucket(mock_server.uri()),
            folder: Default::default(),
            versions: Versions(Arc::new(parking_lot::RwLock::new(HashSet::from([
                version.clone()
            ])))),
        };
        let (compiler, hash) = fetcher.fetch_file(&version).await.unwrap();
        assert_eq!(expected_file, compiler);
        assert_eq!(expected_hash.as_slice(), hash.as_ref());
    }

    #[tokio::test]
    async fn list() {
        let expected_versions: Vec<_> = [
            "v0.4.10+commit.f0d539ae",
            "v0.8.13+commit.abaa5c0e",
            "v0.5.1+commit.c8a2cb62",
        ]
        .into_iter()
        .map(Version::from_str)
        .map(|x| x.unwrap())
        .collect();

        let mock_server = MockServer::start().await;
        mock_list_objects(
            "/solc-releases/",
            expected_versions.iter().map(|x| x.to_string()),
        )
        .mount(&mock_server)
        .await;

        let versions = Versions::fetch_versions(&test_bucket(mock_server.uri()))
            .await
            .unwrap();
        let expected_versions = HashSet::from_iter(expected_versions.into_iter());
        assert_eq!(expected_versions, versions);
    }

    #[tokio::test]
    async fn refresh_list() {
        let all_versions: Vec<_> = [
            "v0.4.10+commit.f0d539ae",
            "v0.8.13+commit.abaa5c0e",
            "v0.5.1+commit.c8a2cb62",
        ]
        .into_iter()
        .map(Version::from_str)
        .map(|x| x.unwrap())
        .collect();

        let mock_server = MockServer::start().await;
        mock_list_objects("/solc-releases/", std::iter::empty())
            .mount(&mock_server)
            .await;

        let fetcher = S3Fetcher::new(
            test_bucket(mock_server.uri()),
            Default::default(),
            Some(Schedule::from_str("* * * * * * *").unwrap()),
        )
        .await
        .unwrap();

        {
            let versions = fetcher.versions.0.read();
            assert!(versions.is_empty());
        }

        {
            let expected_versions = &all_versions[0..2];
            mock_server.reset().await;
            mock_list_objects(
                "/solc-releases/",
                expected_versions.iter().map(|x| x.to_string()),
            )
            .mount(&mock_server)
            .await;

            tokio::time::sleep(Duration::from_secs(2)).await;

            let expected_versions = HashSet::from_iter(expected_versions.into_iter().cloned());
            let versions = fetcher.versions.0.read();
            assert_eq!(expected_versions, *versions);
        }

        {
            let expected_versions = &all_versions[1..3];
            mock_server.reset().await;
            mock_list_objects(
                "/solc-releases/",
                expected_versions
                    .iter()
                    .map(|x| x.to_string())
                    .chain(std::iter::once("some_garbage".into())),
            )
            .mount(&mock_server)
            .await;

            tokio::time::sleep(Duration::from_secs(2)).await;

            let expected_versions = HashSet::from_iter(expected_versions.into_iter().cloned());
            let versions = fetcher.versions.0.read();
            assert_eq!(expected_versions, *versions);
        }
    }
}