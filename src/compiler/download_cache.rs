use super::{
    fetcher::{FetchError, Fetcher},
    list_fetcher::check_hashsum,
    version::Version,
};
use std::{collections::HashMap, path::PathBuf, str::FromStr, sync::Arc};

#[derive(Default)]
pub struct DownloadCache {
    cache: parking_lot::Mutex<HashMap<Version, Arc<tokio::sync::RwLock<Option<PathBuf>>>>>,
}

impl DownloadCache {
    pub fn new() -> Self {
        DownloadCache {
            cache: Default::default(),
        }
    }

    async fn try_get(&self, ver: &Version) -> Option<PathBuf> {
        let entry = {
            let cache = self.cache.lock();
            cache.get(ver).cloned()
        };
        match entry {
            Some(lock) => {
                let file = lock.read().await;
                file.as_ref().cloned()
            }
            None => None,
        }
    }
}

impl DownloadCache {
    pub async fn get<D: Fetcher + ?Sized>(
        &self,
        fetcher: &D,
        ver: &Version,
    ) -> Result<PathBuf, FetchError> {
        match self.try_get(ver).await {
            Some(file) => Ok(file),
            None => self.fetch(fetcher, ver).await,
        }
    }

    async fn fetch<D: Fetcher + ?Sized>(
        &self,
        fetcher: &D,
        ver: &Version,
    ) -> Result<PathBuf, FetchError> {
        let lock = {
            let mut cache = self.cache.lock();
            Arc::clone(cache.entry(ver.clone()).or_default())
        };
        let mut entry = lock.write().await;
        match entry.as_ref() {
            Some(file) => Ok(file.clone()),
            None => {
                log::info!(target: "compiler_cache", "installing file version {}", ver);
                let file = fetcher.fetch(ver).await?;
                *entry = Some(file.clone());
                Ok(file)
            }
        }
    }
}

impl DownloadCache {
    pub async fn load_from_dir<D: Fetcher + ?Sized>(&self, fetcher: &D) -> std::io::Result<()> {
        let entries = std::fs::read_dir(fetcher.folder())?;
        let versions = DownloadCache::find_versions_in_dir(entries);
        for (version, solc_path) in versions {
            if let Some(expected_hash) = fetcher.get_hash(&version) {
                let solc_bytes = std::fs::read(&solc_path)?.into();
                match check_hashsum(&solc_bytes, expected_hash) {
                    Ok(_) => {
                        log::info!("found local compiler version {}", version);
                        let lock = {
                            let mut cache = self.cache.lock();
                            Arc::clone(cache.entry(version.clone()).or_default())
                        };
                        *lock.write().await = Some(solc_path);
                    }
                    Err(mismatch) => {
                        log::warn!(
                            "found file {:?}, but hashsum is different: {}",
                            solc_path,
                            mismatch
                        );
                    }
                }
            } else {
                log::warn!(
                    "found file {:?}, but there is no version {:?} in version list",
                    solc_path,
                    version
                );
            };
        }
        Ok(())
    }

    fn find_versions_in_dir(dir: std::fs::ReadDir) -> HashMap<Version, PathBuf> {
        dir.filter_map(|entry| {
            entry.ok().and_then(|e| {
                let path = e.path();
                let mut solc_path = path.clone();
                solc_path.push("solc");
                path.file_name()
                    .and_then(|n| n.to_str().map(String::from))
                    .and_then(|n| Version::from_str(&n).ok().map(|v| (v, solc_path)))
            })
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::version::ReleaseVersion;
    use async_trait::async_trait;
    use futures::{executor::block_on, join, pin_mut};
    use std::time::Duration;
    use tokio::{spawn, task::yield_now, time::timeout};

    fn new_version(major: u64) -> Version {
        Version::Release(ReleaseVersion {
            version: semver::Version::new(major, 0, 0),
            commit: [0, 1, 2, 3],
        })
    }

    /// Tests, that caching works, meaning that cache downloads each version only once
    #[test]
    fn value_is_cached() {
        #[derive(Default)]
        struct MockFetcher {
            counter: parking_lot::Mutex<HashMap<Version, u32>>,
        }

        #[async_trait]
        impl Fetcher for MockFetcher {
            async fn fetch(&self, ver: &Version) -> Result<PathBuf, FetchError> {
                *self.counter.lock().entry(ver.clone()).or_default() += 1;
                Ok(PathBuf::from(ver.to_string()))
            }

            fn all_versions(&self) -> Vec<Version> {
                vec![]
            }

            fn folder(&self) -> &PathBuf {
                todo!()
            }
        }

        let fetcher = MockFetcher::default();
        let cache = DownloadCache::new();

        let vers: Vec<_> = (0..3).map(new_version).collect();

        let get_and_check = |ver: &Version| {
            let value = block_on(cache.get(&fetcher, ver)).unwrap();
            assert_eq!(value, PathBuf::from(ver.to_string()));
        };

        get_and_check(&vers[0]);
        get_and_check(&vers[1]);
        get_and_check(&vers[0]);
        get_and_check(&vers[0]);
        get_and_check(&vers[1]);
        get_and_check(&vers[1]);
        get_and_check(&vers[2]);
        get_and_check(&vers[2]);
        get_and_check(&vers[1]);
        get_and_check(&vers[0]);

        let counter = fetcher.counter.lock();
        assert_eq!(counter.len(), 3);
        assert!(counter.values().all(|&count| count == 1));
    }

    /// Tests, that cache will not block requests for already downloaded values,
    /// while it downloads others
    #[tokio::test]
    async fn downloading_not_blocks() {
        const TIMEOUT: Duration = Duration::from_secs(10);

        #[derive(Clone)]
        struct MockBlockingFetcher {
            sync: Arc<tokio::sync::Mutex<()>>,
        }

        #[async_trait]
        impl Fetcher for MockBlockingFetcher {
            async fn fetch(&self, ver: &Version) -> Result<PathBuf, FetchError> {
                self.sync.lock().await;
                Ok(PathBuf::from(ver.to_string()))
            }

            fn all_versions(&self) -> Vec<Version> {
                vec![]
            }

            fn folder(&self) -> &PathBuf {
                todo!()
            }
        }

        let sync = Arc::<tokio::sync::Mutex<()>>::default();
        let fetcher = MockBlockingFetcher { sync: sync.clone() };
        let cache = Arc::new(DownloadCache::new());

        let vers: Vec<_> = (0..3).map(new_version).collect();

        // fill the cache
        cache.get(&fetcher, &vers[1]).await.unwrap();

        // lock the fetcher
        let guard = sync.lock().await;

        // try to download (it will block on mutex)
        let handle = {
            let cache = cache.clone();
            let vers = vers.clone();
            let fetcher = fetcher.clone();
            spawn(
                async move { join!(cache.get(&fetcher, &vers[0]), cache.get(&fetcher, &vers[2])) },
            )
        };
        // so we could rerun future after timeout
        pin_mut!(handle);
        // give the thread to the scheduler so it could run "handle" task
        yield_now().await;

        // check, that while we're downloading we don't block the cache
        timeout(TIMEOUT, cache.get(&fetcher, &vers[1]))
            .await
            .expect("should not block")
            .expect("expected value not error");

        // check, that we're blocked on downloading
        timeout(Duration::from_millis(100), &mut handle)
            .await
            .expect_err("should block");

        // release the lock
        std::mem::drop(guard);

        // now we can finish downloading
        let vals = timeout(TIMEOUT, handle)
            .await
            .expect("should not block")
            .unwrap();
        vals.0.expect("expected value got error");
        vals.1.expect("expected value got error");
    }
}
