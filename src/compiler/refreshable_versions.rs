use crate::scheduler;
use async_trait::async_trait;
use cron::Schedule;
use len_trait::Len;
use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::{fmt, sync::Arc};

#[async_trait]
pub trait VersionsFetcher: Send + Sync + 'static {
    type Response;
    type Error: fmt::Display;

    async fn fetch_versions(&self) -> Result<Self::Response, Self::Error>;
}

#[derive(Clone)]
pub struct RefreshableVersions<Fetcher: VersionsFetcher> {
    fetcher: Fetcher,
    versions: Arc<RwLock<<Fetcher as VersionsFetcher>::Response>>,
}

impl<Fetcher, T> fmt::Debug for RefreshableVersions<Fetcher>
where
    Fetcher: VersionsFetcher<Response = T> + fmt::Debug,
    T: fmt::Debug,
{
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("RefreshableVersions")
            .field("fetcher", &format_args!("{:?}", self.fetcher))
            .field("versions", &format_args!("{:?}", self.versions))
            .finish()
    }
}

impl<Fetcher, T> Default for RefreshableVersions<Fetcher>
where
    Fetcher: VersionsFetcher<Response = T> + Default,
    T: Default,
{
    fn default() -> Self {
        Self {
            fetcher: Fetcher::default(),
            versions: Arc::new(RwLock::new(T::default())),
        }
    }
}

impl<Fetcher, T> RefreshableVersions<Fetcher>
where
    Fetcher: VersionsFetcher<Response = T>,
{
    pub async fn new(fetcher: Fetcher) -> Result<Self, Fetcher::Error> {
        let inner = fetcher.fetch_versions().await?;
        Ok(RefreshableVersions {
            fetcher,

            versions: Arc::new(RwLock::new(inner)),
        })
    }

    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        self.versions.read()
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        self.versions.write()
    }

    pub fn spawn_refresh_job(self, cron_schedule: Schedule)
    where
        T: PartialEq + Send + Sync + Len,
        Fetcher: Clone,
    {
        log::info!("spawn version refresh job");
        scheduler::spawn_job(cron_schedule, "refresh compiler version", move || {
            let versions = self.clone();
            let fetcher = self.fetcher.clone();
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
