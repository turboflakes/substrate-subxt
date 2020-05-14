// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of substrate-subxt.
//
// subxt is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// subxt is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with substrate-subxt.  If not, see <http://www.gnu.org/licenses/>.

//! Implements support for the frame_staking module.

use crate::{
    frame::Store,
    metadata::{
        Metadata,
        MetadataError,
    },
};
use codec::{
    Decode,
    Encode,
    HasCompact,
};
use sp_core::storage::StorageKey;
use sp_runtime::{
    Perbill,
    RuntimeDebug,
};
use std::{
    fmt::Debug,
    marker::PhantomData,
};

/// A record of the nominations made by a specific account.
#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
pub struct Nominations<AccountId> {
    /// The targets of nomination.
    pub targets: Vec<AccountId>,
    /// The era the nominations were submitted.
    ///
    /// Except for initial nominations which are considered submitted at era 0.
    pub submitted_in: EraIndex,
    /// Whether the nominations have been suppressed.
    pub suppressed: bool,
}

/// Information regarding the active era (era in used in session).
#[derive(Encode, Decode, RuntimeDebug)]
pub struct ActiveEraInfo {
    /// Index of era.
    pub index: EraIndex,
    /// Moment of start expresed as millisecond from `$UNIX_EPOCH`.
    ///
    /// Start can be none if start hasn't been set for the era yet,
    /// Start is set on the first on_finalize of the era to guarantee usage of `Time`.
    start: Option<u64>,
}

/// Data type used to index nominators in the compact type
pub type NominatorIndex = u32;

/// Data type used to index validators in the compact type.
pub type ValidatorIndex = u16;

/// Maximum number of validators that can be stored in a snapshot.
pub const MAX_VALIDATORS: usize = ValidatorIndex::max_value() as usize;

/// Maximum number of nominators that can be stored in a snapshot.
pub const MAX_NOMINATORS: usize = NominatorIndex::max_value() as usize;

/// Counter for the number of eras that have passed.
pub type EraIndex = u32;

/// Counter for the number of "reward" points earned by a given validator.
pub type RewardPoint = u32;

/// A destination account for payment.
#[derive(PartialEq, Eq, Copy, Clone, Encode, Decode, RuntimeDebug)]
pub enum RewardDestination {
    /// Pay into the stash account, increasing the amount at stake accordingly.
    Staked,
    /// Pay into the stash account, not increasing the amount at stake.
    Stash,
    /// Pay into the controller account.
    Controller,
}

impl Default for RewardDestination {
    fn default() -> Self {
        RewardDestination::Staked
    }
}

/// Preference of what happens regarding validation.
#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
pub struct ValidatorPrefs {
    /// Reward that validator takes up-front; only the rest is split between themselves and
    /// nominators.
    #[codec(compact)]
    pub commission: Perbill,
}

impl Default for ValidatorPrefs {
    fn default() -> Self {
        ValidatorPrefs {
            commission: Default::default(),
        }
    }
}

/// The subset of the `frame::Trait` that a client must implement.
pub trait Staking: super::system::System {}

/// Just a Balance/BlockNumber tuple to encode when a chunk of funds will be unlocked.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
pub struct UnlockChunk<Balance: HasCompact> {
    /// Amount of funds to be unlocked.
    #[codec(compact)]
    value: Balance,
    /// Era number at which point it'll be unlocked.
    #[codec(compact)]
    era: EraIndex,
}

/// The ledger of a (bonded) stash.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
pub struct StakingLedger<AccountId, Balance: HasCompact> {
    /// The stash account whose balance is actually locked and at stake.
    pub stash: AccountId,
    /// The total amount of the stash's balance that we are currently accounting for.
    /// It's just `active` plus all the `unlocking` balances.
    #[codec(compact)]
    pub total: Balance,
    /// The total amount of the stash's balance that will be at stake in any forthcoming
    /// rounds.
    #[codec(compact)]
    pub active: Balance,
    /// Any balance that is becoming free, which may eventually be transferred out
    /// of the stash (assuming it doesn't get slashed first).
    pub unlocking: Vec<UnlockChunk<Balance>>,
    /// List of eras for which the stakers behind a validator have claimed rewards. Only updated
    /// for validators.
    pub claimed_rewards: Vec<EraIndex>,
}

const MODULE: &str = "Staking";

/// Number of eras to keep in history.
///
/// Information is kept for eras in `[current_era - history_depth; current_era]`.
///
/// Must be more than the number of eras delayed by session otherwise.
/// I.e. active era must always be in history.
/// I.e. `active_era > current_era - history_depth` must be guaranteed.
#[derive(Encode, Decode, Copy, Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub struct HistoryDepth<T: Staking>(PhantomData<T>);

impl<T: Staking> Store<T> for HistoryDepth<T> {
    const MODULE: &'static str = MODULE;
    const FIELD: &'static str = "HistoryDepth";
    type Returns = u32;

    fn key(&self, metadata: &Metadata) -> Result<StorageKey, MetadataError> {
        Ok(metadata
            .module(Self::MODULE)?
            .storage(Self::FIELD)?
            .plain()?
            .key())
    }
}

/// The ideal number of staking participants.
#[derive(Encode, Decode, Copy, Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub struct ValidatorCount<T: Staking>(PhantomData<T>);

impl<T: Staking> Store<T> for ValidatorCount<T> {
    const MODULE: &'static str = MODULE;
    const FIELD: &'static str = "ValidatorCount";
    type Returns = u32;

    fn key(&self, metadata: &Metadata) -> Result<StorageKey, MetadataError> {
        Ok(metadata
            .module(Self::MODULE)?
            .storage(Self::FIELD)?
            .plain()?
            .key())
    }
}

/// Minimum number of staking participants before emergency conditions are imposed.
#[derive(Encode, Decode, Copy, Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub struct MinimumValidatorCount<T: Staking>(PhantomData<T>);

impl<T: Staking> Store<T> for MinimumValidatorCount<T> {
    const MODULE: &'static str = MODULE;
    const FIELD: &'static str = "MinimumValidatorCount";
    type Returns = u32;

    fn key(&self, metadata: &Metadata) -> Result<StorageKey, MetadataError> {
        Ok(metadata
            .module(Self::MODULE)?
            .storage(Self::FIELD)?
            .plain()?
            .key())
    }
}

/// Any validators that may never be slashed or forcibly kicked. It's a Vec since they're
/// easy to initialize and the performance hit is minimal (we expect no more than four
/// invulnerables) and restricted to testnets.
#[derive(Encode, Copy, Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub struct Invulnerables<T: Staking>(pub core::marker::PhantomData<T>);

impl<T: Staking> Store<T> for Invulnerables<T> {
    const MODULE: &'static str = MODULE;
    const FIELD: &'static str = "Invulnerables";
    type Returns = Vec<T::AccountId>;

    fn key(&self, metadata: &Metadata) -> Result<StorageKey, MetadataError> {
        Ok(metadata
            .module(Self::MODULE)?
            .storage(Self::FIELD)?
            .plain()?
            .key())
    }
}

/// Map from all locked "stash" accounts to the controller account.
#[derive(Encode, Copy, Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub struct Bonded<T: Staking>(pub PhantomData<T>);

impl<T: Staking> Store<T> for Bonded<T> {
    const MODULE: &'static str = MODULE;
    const FIELD: &'static str = "Bonded";
    type Returns = Vec<T::AccountId>;

    fn key(&self, metadata: &Metadata) -> Result<StorageKey, MetadataError> {
        Ok(metadata
            .module(Self::MODULE)?
            .storage(Self::FIELD)?
            .map()?
            .key(&self.0))
    }
}

/// Map from all (unlocked) "controller" accounts to the info regarding the staking.
#[derive(Encode, Copy, Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub struct Ledger<T: Staking>(pub T::AccountId);

impl<T: Staking> Store<T> for Ledger<T> {
    const MODULE: &'static str = MODULE;
    const FIELD: &'static str = "Ledger";
    type Returns = Option<StakingLedger<T::AccountId, ()>>;

    fn key(&self, metadata: &Metadata) -> Result<StorageKey, MetadataError> {
        Ok(metadata
            .module(Self::MODULE)?
            .storage(Self::FIELD)?
            .map()?
            .key(&self.0))
    }
}

/// Where the reward payment should be made. Keyed by stash.
#[derive(Encode, Copy, Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub struct Payee<T: Staking>(pub T::AccountId);

impl<T: Staking> Store<T> for Payee<T> {
    const MODULE: &'static str = MODULE;
    const FIELD: &'static str = "Payee";
    type Returns = RewardDestination;

    fn key(&self, metadata: &Metadata) -> Result<StorageKey, MetadataError> {
        Ok(metadata
            .module(Self::MODULE)?
            .storage(Self::FIELD)?
            .map()?
            .key(&self.0))
    }
}

/// The map from (wannabe) validator stash key to the preferences of that validator.
#[derive(Encode, Copy, Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub struct Validators<T: Staking>(pub T::AccountId);

impl<T: Staking> Store<T> for Validators<T> {
    const MODULE: &'static str = MODULE;
    const FIELD: &'static str = "Validators";
    type Returns = ValidatorPrefs;

    fn key(&self, metadata: &Metadata) -> Result<StorageKey, MetadataError> {
        Ok(metadata
            .module(Self::MODULE)?
            .storage(Self::FIELD)?
            .map()?
            .key(&self.0))
    }
}

/// The map from nominator stash key to the set of stash keys of all validators to nominate.
#[derive(Encode, Copy, Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub struct Nominators<T: Staking>(pub T::AccountId);

impl<T: Staking> Store<T> for Nominators<T> {
    const MODULE: &'static str = MODULE;
    const FIELD: &'static str = "Nominators";
    type Returns = Option<Nominations<T::AccountId>>;

    fn key(&self, metadata: &Metadata) -> Result<StorageKey, MetadataError> {
        Ok(metadata
            .module(Self::MODULE)?
            .storage(Self::FIELD)?
            .map()?
            .key(&self.0))
    }
}

/// The current era index.
///
/// This is the latest planned era, depending on how the Session pallet queues the validator
/// set, it might be active or not.
#[derive(Encode, Copy, Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub struct CurrentEra<T: Staking>(pub PhantomData<T>);

impl<T: Staking> Store<T> for CurrentEra<T> {
    const MODULE: &'static str = MODULE;
    const FIELD: &'static str = "CurrentEra";
    type Returns = Option<EraIndex>;

    fn key(&self, metadata: &Metadata) -> Result<StorageKey, MetadataError> {
        Ok(metadata
            .module(Self::MODULE)?
            .storage(Self::FIELD)?
            .map()?
            .key(&self.0))
    }
}

/// The active era information, it holds index and start.
///
/// The active era is the era currently rewarded.
/// Validator set of this era must be equal to `SessionInterface::validators`.
#[derive(Encode, Copy, Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub struct ActiveEra<T: Staking>(pub PhantomData<T>);

impl<T: Staking> Store<T> for ActiveEra<T> {
    const MODULE: &'static str = MODULE;
    const FIELD: &'static str = "ActiveEra";
    type Returns = Option<ActiveEraInfo>;

    fn key(&self, metadata: &Metadata) -> Result<StorageKey, MetadataError> {
        Ok(metadata
            .module(Self::MODULE)?
            .storage(Self::FIELD)?
            .map()?
            .key(&self.0))
    }
}