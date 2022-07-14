use crate::consts::DEFAULT_COMPILER_LIST;
use config::{Config as LibConfig, File};
use cron::Schedule;
use serde::Deserialize;
use std::{net::SocketAddr, num::NonZeroUsize, path::PathBuf, str::FromStr};
use url::Url;

#[derive(Deserialize, Clone, Default)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfiguration,
    pub solidity: SolidityConfiguration,
    pub sourcify: SourcifyConfiguration,
}

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct ServerConfiguration {
    pub addr: SocketAddr,
}

impl Default for ServerConfiguration {
    fn default() -> Self {
        Self {
            addr: SocketAddr::from_str("0.0.0.0:8043").expect("should be valid url"),
        }
    }
}

#[derive(Deserialize, Clone)]
pub struct ListFetcherConfig {
    pub compilers_list_url: Url,
}

impl Default for ListFetcherConfig {
    fn default() -> Self {
        Self {
            compilers_list_url: Url::try_from(DEFAULT_COMPILER_LIST).expect("valid url"),
        }
    }
}

#[derive(Deserialize, Default, Clone)]
pub struct S3FetcherConfig {
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub bucket: String,
}

#[derive(Deserialize, Clone)]
pub enum FetcherConfig {
    List(ListFetcherConfig),
    S3(S3FetcherConfig),
}

impl Default for FetcherConfig {
    fn default() -> Self {
        Self::List(Default::default())
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct SolidityConfiguration {
    pub enabled: bool,
    pub fetcher: FetcherConfig,
    pub compiler_folder: PathBuf,
    #[serde(with = "serde_with::rust::display_fromstr")]
    pub refresh_versions_schedule: Schedule,
}

impl Default for SolidityConfiguration {
    fn default() -> Self {
        Self {
            enabled: true,
            fetcher: Default::default(),
            compiler_folder: "compilers/".into(),
            refresh_versions_schedule: Schedule::from_str("0 0 * * * * *").unwrap(), // every hour
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct SourcifyConfiguration {
    pub enabled: bool,
    pub api_url: Url,
    /// Number of attempts the server makes to Sourcify API.
    /// Should be at least one. Set to `3` by default.
    pub verification_attempts: NonZeroUsize,
    pub request_timeout: u64,
}

impl Default for SourcifyConfiguration {
    fn default() -> Self {
        Self {
            enabled: true,
            api_url: Url::try_from("https://sourcify.dev/server/").expect("valid url"),
            verification_attempts: NonZeroUsize::new(3).expect("Is not zero"),
            request_timeout: 10,
        }
    }
}

impl Config {
    pub fn from_file(file: PathBuf) -> Result<Self, config::ConfigError> {
        let mut builder =
            LibConfig::builder().add_source(config::Environment::with_prefix("VERIFICATION"));
        if file.exists() {
            builder = builder.add_source(File::from(file));
        }
        builder
            .build()
            .expect("Failed to build config")
            .try_deserialize()
    }
}
