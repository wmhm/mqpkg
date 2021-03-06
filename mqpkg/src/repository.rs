// This file is dual licensed under the terms of the Apache License, Version
// 2.0, and the BSD License. See the LICENSE file in the root of this repository
// for complete details.

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::BufReader;

use indexmap::IndexMap;
use log::info;
use reqwest::blocking::Client as HTTPClient;
use semver::{Version, VersionReq};
use serde::Deserialize;
use url::Url;

use crate::config;
use crate::errors::RepositoryError;
use crate::resolver::{Candidate, StaticDependencies};
use crate::types::{PackageName, Source};

const LOGNAME: &str = "mqpkg::repository";

type Result<T, E = RepositoryError> = core::result::Result<T, E>;

#[derive(Deserialize, Debug)]
struct MetaData {
    #[serde(rename = "name")]
    _name: String,
}

#[derive(Deserialize, Debug)]
struct Release {
    #[serde(default)]
    dependencies: HashMap<PackageName, VersionReq>,
    #[serde(rename = "urls")]
    _urls: Vec<Url>,
    #[serde(rename = "digests")]
    _digests: HashMap<String, String>,
}

#[derive(Deserialize, Debug)]
struct RepoData {
    #[serde(rename = "meta")]
    _meta: MetaData,
    packages: HashMap<PackageName, HashMap<Version, Release>>,
}

#[derive(Debug)]
pub(crate) struct Repository {
    client: HTTPClient,
    data: IndexMap<config::Repository, RepoData>,
}

impl Repository {
    pub(crate) fn new() -> Result<Repository> {
        let client = HTTPClient::builder().gzip(true).build()?;
        let data = IndexMap::<config::Repository, RepoData>::new();

        Ok(Repository { client, data })
    }

    pub(crate) fn fetch(
        mut self,
        repos: &[config::Repository],
        callback: impl Fn(),
    ) -> Result<Repository> {
        info!(target: LOGNAME, "fetching package metadata");
        for repo in repos.iter() {
            let data: RepoData = match repo.url.scheme() {
                "file" => {
                    let file = File::open(repo.url.to_file_path().unwrap())?;
                    let reader = BufReader::new(file);

                    serde_json::from_reader(reader)?
                }
                _ => self
                    .client
                    .get(repo.url.clone())
                    .send()?
                    .error_for_status()?
                    .json()?,
            };
            self.data.insert(repo.clone(), data);
            (callback)();
        }

        Ok(self)
    }

    pub(crate) fn candidates<P: AsRef<PackageName>>(&self, package: P) -> Vec<Candidate> {
        let mut candidates = Vec::<Candidate>::new();

        // Because our underlying type of self.data is an IndexMap, this will ensure
        // that our Vec is sorted by the order our repositories were defined in, however
        // the list of versions within that is not sorted, so we'll need to resort
        // the full list later.
        for (idx, (repo, data)) in self.data.iter().enumerate() {
            if let Some(packages) = data.packages.get(package.as_ref()) {
                for (version, release) in packages.iter() {
                    candidates.push(Candidate::new(
                        version,
                        Box::new(RepositorySource::new(
                            u64::try_from(idx).unwrap(),
                            repo.clone(),
                        )),
                        Box::new(StaticDependencies::new(release.dependencies.clone())),
                    ));
                }
            }
        }

        candidates
    }
}

#[derive(Debug, Clone)]
struct RepositorySource {
    repository_id: u64,
    repository: config::Repository,
}

impl RepositorySource {
    fn new(repository_id: u64, repository: config::Repository) -> RepositorySource {
        RepositorySource {
            repository_id,
            repository,
        }
    }
}

impl fmt::Display for RepositorySource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let identifier = if !self.repository.name.is_empty() {
            self.repository.name.as_str()
        } else {
            self.repository.url.as_str()
        };
        write!(f, "Repository(id={}, {})", self.repository_id, identifier)
    }
}

impl Source for RepositorySource {
    fn id(&self) -> u64 {
        100
    }

    fn discriminator(&self) -> u64 {
        self.repository_id
    }
}
