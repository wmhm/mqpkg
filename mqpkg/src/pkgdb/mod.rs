// This file is dual licensed under the terms of the Apache License, Version
// 2.0, and the BSD License. See the LICENSE file in the root of this repository
// for complete details.

use std::collections::HashMap;
use std::default::Default;
use std::mem::drop;

use log::trace;
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use vfs::VfsPath;

use crate::errors::DBError;
use crate::pkgdb::transactions::{Transaction, TransactionManager};
use crate::types::{PackageName, PackageSpecifier};

mod transactions;

const LOGNAME: &str = "mqpkg::pkgdb";

const PKGDB_DIR: &str = "pkgdb";
const STATE_FILE: &str = "state.yml";

type Result<T, E = DBError> = core::result::Result<T, E>;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct PackageRequest {
    pub(crate) name: PackageName,
    pub(crate) version: VersionReq,
}

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(default)]
struct State {
    requested: HashMap<PackageName, PackageRequest>,
}

impl State {
    fn load(fs: &VfsPath) -> Result<State> {
        let filename = state_path(fs)?;
        trace!(
            target: LOGNAME,
            "loading state from {:?}",
            filename.as_str()
        );
        let state: State = if filename.is_file()? {
            serde_yaml::from_reader(filename.open_file()?)
                .map_err(|source| DBError::InvalidState { source })?
        } else {
            trace!(target: LOGNAME, "could not find state, using default");
            State {
                ..Default::default()
            }
        };

        Ok(state)
    }

    fn save(&self, fs: &VfsPath) -> Result<()> {
        ensure_dir(&pkgdb_path(fs)?)?;

        let filename = state_path(fs)?;
        trace!(target: LOGNAME, "saving state to {:?}", filename.as_str());
        let file = filename.create_file()?;
        serde_yaml::to_writer(file, self).map_err(|source| DBError::InvalidState { source })?;
        Ok(())
    }
}

pub(crate) struct Database {
    id: String,
    fs: VfsPath,
    state: Option<State>,
}

impl Database {
    pub(crate) fn new(fs: VfsPath, id: String) -> Result<Database> {
        Ok(Database {
            id,
            fs,
            state: None,
        })
    }

    pub(crate) fn transaction(&self) -> Result<TransactionManager> {
        Ok(TransactionManager::new(&self.id)?)
    }

    pub(crate) fn begin<'r>(&mut self, txnm: &'r TransactionManager) -> Result<Transaction<'r>> {
        let txn = txnm.begin()?;
        trace!(target: LOGNAME, "begin transaction");
        Ok(txn)
    }

    pub(crate) fn commit(&mut self, txn: Transaction) -> Result<()> {
        trace!(target: LOGNAME, "commit transaction");
        let fs = self.fs.clone();

        // Save all our various pieces of data that we've built up in our
        // transaction.
        self.state()?.save(&fs)?;
        self.state = None;

        // Drop our transaction, which unlocks everything, and ensures that
        // our transaction is open to everyone to use again. We could just
        // let the fact that txn moved into commit auto drop this, but this
        // documents our intent more, rather than just having an unused
        // parameter.
        drop(txn);

        Ok(())
    }

    pub(crate) fn add(&mut self, package: &PackageSpecifier) -> Result<()> {
        let state = self.state()?;
        trace!(
            target: LOGNAME,
            "adding {}({}) to requested packages",
            package.name,
            package.version
        );
        state.requested.insert(
            package.name.clone(),
            PackageRequest {
                name: package.name.clone(),
                version: package.version.clone(),
            },
        );
        Ok(())
    }

    pub(crate) fn requested(&mut self) -> Result<&HashMap<PackageName, PackageRequest>> {
        Ok(&self.state()?.requested)
    }
}

impl Database {
    fn in_transaction(&self) -> Result<bool> {
        Ok(self.transaction()?.is_active()?)
    }

    fn state(&mut self) -> Result<&mut State> {
        if self.in_transaction()? && self.state.is_none() {
            self.state = Some(State::load(&self.fs)?);
        }

        self.state.as_mut().ok_or(DBError::NoTransaction)
    }
}

fn pkgdb_path(fs: &VfsPath) -> Result<VfsPath> {
    Ok(fs.join(PKGDB_DIR)?)
}

fn state_path(fs: &VfsPath) -> Result<VfsPath> {
    Ok(pkgdb_path(fs)?.join(STATE_FILE)?)
}

fn ensure_dir(path: &VfsPath) -> Result<()> {
    if !path.is_dir()? {
        path.create_dir()?;
    }

    Ok(())
}

macro_rules! transaction {
    ($db:expr, $body:block) => {{
        let __txnm = $db.transaction()?;
        let __txn = $db.begin(&__txnm)?;
        let __result = $body;

        $db.commit(__txn)?;

        __result
    }};

    ($db:expr, $body:expr) => {{
        transaction!($db, { $body })
    }};
}

pub(crate) use transaction;
