// Copyright 2019-2022 Parity Technologies (UK) Ltd.
// This file is dual-licensed as Apache-2.0 or GPL-3.0.
// see LICENSE for license details.

//! RPC types and client for interacting with a substrate node.
//!
//! This is used behind the scenes by various `subxt` APIs, but can
//! also be used directly.
//!
//! # Example
//!
//! Fetching storage keys
//!
//! ```no_run
//! use subxt::{ PolkadotConfig, OnlineClient, storage::StorageKey };
//!
//! #[subxt::subxt(runtime_metadata_path = "../artifacts/polkadot_metadata.scale")]
//! pub mod polkadot {}
//!
//! # #[tokio::main]
//! # async fn main() {
//! let api = OnlineClient::<PolkadotConfig>::new().await.unwrap();
//!
//! let key = polkadot::storage()
//!     .xcm_pallet()
//!     .version_notifiers_root()
//!     .to_bytes();
//!
//! // Fetch up to 10 keys.
//! let keys = api
//!     .rpc()
//!     .storage_keys_paged(&key, 10, None, None)
//!     .await
//!     .unwrap();
//!
//! for key in keys.iter() {
//!     println!("Key: 0x{}", hex::encode(&key));
//! }
//! # }
//! ```

use super::{
    rpc_params,
    RpcClient,
    RpcClientT,
    Subscription,
};
use crate::{
    error::Error,
    utils::PhantomDataSendSync,
    Config,
    Metadata,
};
use codec::{
    Decode,
    Encode,
};
use frame_metadata::RuntimeMetadataPrefixed;
use serde::{
    Deserialize,
    Serialize,
};
use sp_core::{
    storage::{
        StorageChangeSet,
        StorageData,
        StorageKey,
    },
    Bytes,
    U256,
};
use sp_runtime::ApplyExtrinsicResult;
use std::{
    collections::HashMap,
    sync::Arc,
};

/// A number type that can be serialized both as a number or a string that encodes a number in a
/// string.
///
/// We allow two representations of the block number as input. Either we deserialize to the type
/// that is specified in the block type or we attempt to parse given hex value.
///
/// The primary motivation for having this type is to avoid overflows when using big integers in
/// JavaScript (which we consider as an important RPC API consumer).
#[derive(Copy, Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(untagged)]
pub enum NumberOrHex {
    /// The number represented directly.
    Number(u64),
    /// Hex representation of the number.
    Hex(U256),
}

/// The response from `chain_getBlock`
#[derive(Debug, Deserialize)]
#[serde(bound = "T: Config")]
pub struct ChainBlockResponse<T: Config> {
    /// The block itself.
    pub block: ChainBlock<T>,
    /// Block justification.
    pub justifications: Option<sp_runtime::Justifications>,
}

/// Block details in the [`ChainBlockResponse`].
#[derive(Debug, Deserialize)]
pub struct ChainBlock<T: Config> {
    /// The block header.
    pub header: T::Header,
    /// The accompanying extrinsics.
    pub extrinsics: Vec<ChainBlockExtrinsic>,
}

/// Bytes representing an extrinsic in a [`ChainBlock`].
#[derive(Debug)]
pub struct ChainBlockExtrinsic(pub Vec<u8>);

impl<'a> ::serde::Deserialize<'a> for ChainBlockExtrinsic {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'a>,
    {
        let r = sp_core::bytes::deserialize(de)?;
        let bytes = Decode::decode(&mut &r[..])
            .map_err(|e| ::serde::de::Error::custom(format!("Decode error: {}", e)))?;
        Ok(ChainBlockExtrinsic(bytes))
    }
}

/// Wrapper for NumberOrHex to allow custom From impls
#[derive(Serialize)]
pub struct BlockNumber(NumberOrHex);

impl From<NumberOrHex> for BlockNumber {
    fn from(x: NumberOrHex) -> Self {
        BlockNumber(x)
    }
}

impl Default for NumberOrHex {
    fn default() -> Self {
        Self::Number(Default::default())
    }
}

impl NumberOrHex {
    /// Converts this number into an U256.
    pub fn into_u256(self) -> U256 {
        match self {
            NumberOrHex::Number(n) => n.into(),
            NumberOrHex::Hex(h) => h,
        }
    }
}

impl From<u32> for NumberOrHex {
    fn from(n: u32) -> Self {
        NumberOrHex::Number(n.into())
    }
}

impl From<u64> for NumberOrHex {
    fn from(n: u64) -> Self {
        NumberOrHex::Number(n)
    }
}

impl From<u128> for NumberOrHex {
    fn from(n: u128) -> Self {
        NumberOrHex::Hex(n.into())
    }
}

impl From<U256> for NumberOrHex {
    fn from(n: U256) -> Self {
        NumberOrHex::Hex(n)
    }
}

/// An error type that signals an out-of-range conversion attempt.
#[derive(Debug, thiserror::Error)]
#[error("Out-of-range conversion attempt")]
pub struct TryFromIntError;

impl TryFrom<NumberOrHex> for u32 {
    type Error = TryFromIntError;
    fn try_from(num_or_hex: NumberOrHex) -> Result<u32, Self::Error> {
        num_or_hex
            .into_u256()
            .try_into()
            .map_err(|_| TryFromIntError)
    }
}

impl TryFrom<NumberOrHex> for u64 {
    type Error = TryFromIntError;
    fn try_from(num_or_hex: NumberOrHex) -> Result<u64, Self::Error> {
        num_or_hex
            .into_u256()
            .try_into()
            .map_err(|_| TryFromIntError)
    }
}

impl TryFrom<NumberOrHex> for u128 {
    type Error = TryFromIntError;
    fn try_from(num_or_hex: NumberOrHex) -> Result<u128, Self::Error> {
        num_or_hex
            .into_u256()
            .try_into()
            .map_err(|_| TryFromIntError)
    }
}

impl From<NumberOrHex> for U256 {
    fn from(num_or_hex: NumberOrHex) -> U256 {
        num_or_hex.into_u256()
    }
}

// All unsigned ints can be converted into a BlockNumber:
macro_rules! into_block_number {
    ($($t: ty)+) => {
        $(
            impl From<$t> for BlockNumber {
                fn from(x: $t) -> Self {
                    NumberOrHex::Number(x.into()).into()
                }
            }
        )+
    }
}
into_block_number!(u8 u16 u32 u64);

/// Arbitrary properties defined in the chain spec as a JSON object.
pub type SystemProperties = serde_json::Map<String, serde_json::Value>;

/// Possible transaction status events.
///
/// # Note
///
/// This is copied from `sp-transaction-pool` to avoid a dependency on that crate. Therefore it
/// must be kept compatible with that type from the target substrate version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubstrateTxStatus<Hash, BlockHash> {
    /// Transaction is part of the future queue.
    Future,
    /// Transaction is part of the ready queue.
    Ready,
    /// The transaction has been broadcast to the given peers.
    Broadcast(Vec<String>),
    /// Transaction has been included in block with given hash.
    InBlock(BlockHash),
    /// The block this transaction was included in has been retracted.
    Retracted(BlockHash),
    /// Maximum number of finality watchers has been reached,
    /// old watchers are being removed.
    FinalityTimeout(BlockHash),
    /// Transaction has been finalized by a finality-gadget, e.g GRANDPA
    Finalized(BlockHash),
    /// Transaction has been replaced in the pool, by another transaction
    /// that provides the same tags. (e.g. same (sender, nonce)).
    Usurped(Hash),
    /// Transaction has been dropped from the pool because of the limit.
    Dropped,
    /// Transaction is no longer valid in the current state.
    Invalid,
}

/// This contains the runtime version information necessary to make transactions, as obtained from
/// the RPC call `state_getRuntimeVersion`,
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeVersion {
    /// Version of the runtime specification. A full-node will not attempt to use its native
    /// runtime in substitute for the on-chain Wasm runtime unless all of `spec_name`,
    /// `spec_version` and `authoring_version` are the same between Wasm and native.
    pub spec_version: u32,

    /// All existing dispatches are fully compatible when this number doesn't change. If this
    /// number changes, then `spec_version` must change, also.
    ///
    /// This number must change when an existing dispatchable (module ID, dispatch ID) is changed,
    /// either through an alteration in its user-level semantics, a parameter
    /// added/removed/changed, a dispatchable being removed, a module being removed, or a
    /// dispatchable/module changing its index.
    ///
    /// It need *not* change when a new module is added or when a dispatchable is added.
    pub transaction_version: u32,

    /// The other fields present may vary and aren't necessary for `subxt`; they are preserved in
    /// this map.
    #[serde(flatten)]
    pub other: HashMap<String, serde_json::Value>,
}

/// ReadProof struct returned by the RPC
///
/// # Note
///
/// This is copied from `sc-rpc-api` to avoid a dependency on that crate. Therefore it
/// must be kept compatible with that type from the target substrate version.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadProof<Hash> {
    /// Block hash used to generate the proof
    pub at: Hash,
    /// A proof used to prove that storage entries are included in the storage trie
    pub proof: Vec<Bytes>,
}

/// Statistics of a block returned by the `dev_getBlockStats` RPC.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockStats {
    /// The length in bytes of the storage proof produced by executing the block.
    pub witness_len: u64,
    /// The length in bytes of the storage proof after compaction.
    pub witness_compact_len: u64,
    /// Length of the block in bytes.
    ///
    /// This information can also be acquired by downloading the whole block. This merely
    /// saves some complexity on the client side.
    pub block_len: u64,
    /// Number of extrinsics in the block.
    ///
    /// This information can also be acquired by downloading the whole block. This merely
    /// saves some complexity on the client side.
    pub num_extrinsics: u64,
}

/// Health struct returned by the RPC
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    /// Number of connected peers
    pub peers: usize,
    /// Is the node syncing
    pub is_syncing: bool,
    /// Should this node have any peers
    ///
    /// Might be false for local chains or when running without discovery.
    pub should_have_peers: bool,
}

/// Client for substrate rpc interfaces
pub struct Rpc<T: Config> {
    client: RpcClient,
    _marker: PhantomDataSendSync<T>,
}

impl<T: Config> Clone for Rpc<T> {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            _marker: PhantomDataSendSync::new(),
        }
    }
}

// Expose subscribe/request, and also subscribe_raw/request_raw
// from the even-deeper `dyn RpcClientT` impl.
impl<T: Config> std::ops::Deref for Rpc<T> {
    type Target = RpcClient;
    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

impl<T: Config> Rpc<T> {
    /// Create a new [`Rpc`]
    pub fn new<R: RpcClientT>(client: Arc<R>) -> Self {
        Self {
            client: RpcClient::new(client),
            _marker: PhantomDataSendSync::new(),
        }
    }

    /// Fetch the raw bytes for a given storage key
    pub async fn storage(
        &self,
        key: &[u8],
        hash: Option<T::Hash>,
    ) -> Result<Option<StorageData>, Error> {
        let params = rpc_params![to_hex(key), hash];
        let data = self.client.request("state_getStorage", params).await?;
        Ok(data)
    }

    /// Returns the keys with prefix with pagination support.
    /// Up to `count` keys will be returned.
    /// If `start_key` is passed, return next keys in storage in lexicographic order.
    pub async fn storage_keys_paged(
        &self,
        key: &[u8],
        count: u32,
        start_key: Option<&[u8]>,
        hash: Option<T::Hash>,
    ) -> Result<Vec<StorageKey>, Error> {
        let start_key = start_key.map(to_hex);
        let params = rpc_params![to_hex(key), count, start_key, hash];
        let data = self.client.request("state_getKeysPaged", params).await?;
        Ok(data)
    }

    /// Query historical storage entries
    pub async fn query_storage(
        &self,
        keys: impl IntoIterator<Item = &[u8]>,
        from: T::Hash,
        to: Option<T::Hash>,
    ) -> Result<Vec<StorageChangeSet<T::Hash>>, Error> {
        let keys: Vec<String> = keys.into_iter().map(to_hex).collect();
        let params = rpc_params![keys, from, to];
        self.client
            .request("state_queryStorage", params)
            .await
            .map_err(Into::into)
    }

    /// Query historical storage entries
    pub async fn query_storage_at(
        &self,
        keys: impl IntoIterator<Item = &[u8]>,
        at: Option<T::Hash>,
    ) -> Result<Vec<StorageChangeSet<T::Hash>>, Error> {
        let keys: Vec<String> = keys.into_iter().map(to_hex).collect();
        let params = rpc_params![keys, at];
        self.client
            .request("state_queryStorageAt", params)
            .await
            .map_err(Into::into)
    }

    /// Fetch the genesis hash
    pub async fn genesis_hash(&self) -> Result<T::Hash, Error> {
        let block_zero = 0u32;
        let params = rpc_params![block_zero];
        let genesis_hash: Option<T::Hash> =
            self.client.request("chain_getBlockHash", params).await?;
        genesis_hash.ok_or_else(|| "Genesis hash not found".into())
    }

    /// Fetch the metadata
    pub async fn metadata(&self, at: Option<T::Hash>) -> Result<Metadata, Error> {
        let bytes: Bytes = self
            .client
            .request("state_getMetadata", rpc_params![at])
            .await?;
        let meta: RuntimeMetadataPrefixed = Decode::decode(&mut &bytes[..])?;
        let metadata: Metadata = meta.try_into()?;
        Ok(metadata)
    }

    /// Fetch system properties
    pub async fn system_properties(&self) -> Result<SystemProperties, Error> {
        self.client
            .request("system_properties", rpc_params![])
            .await
    }

    /// Fetch system health
    pub async fn system_health(&self) -> Result<Health, Error> {
        self.client.request("system_health", rpc_params![]).await
    }

    /// Fetch system chain
    pub async fn system_chain(&self) -> Result<String, Error> {
        self.client.request("system_chain", rpc_params![]).await
    }

    /// Fetch system name
    pub async fn system_name(&self) -> Result<String, Error> {
        self.client.request("system_name", rpc_params![]).await
    }

    /// Fetch system version
    pub async fn system_version(&self) -> Result<String, Error> {
        self.client.request("system_version", rpc_params![]).await
    }

    /// Fetch the current nonce for the given account ID.
    pub async fn system_account_next_index(
        &self,
        account: &T::AccountId,
    ) -> Result<T::Index, Error> {
        self.client
            .request("system_accountNextIndex", rpc_params![account])
            .await
    }

    /// Get a header
    pub async fn header(
        &self,
        hash: Option<T::Hash>,
    ) -> Result<Option<T::Header>, Error> {
        let params = rpc_params![hash];
        let header = self.client.request("chain_getHeader", params).await?;
        Ok(header)
    }

    /// Get a block hash, returns hash of latest block by default
    pub async fn block_hash(
        &self,
        block_number: Option<BlockNumber>,
    ) -> Result<Option<T::Hash>, Error> {
        let params = rpc_params![block_number];
        let block_hash = self.client.request("chain_getBlockHash", params).await?;
        Ok(block_hash)
    }

    /// Get a block hash of the latest finalized block
    pub async fn finalized_head(&self) -> Result<T::Hash, Error> {
        let hash = self
            .client
            .request("chain_getFinalizedHead", rpc_params![])
            .await?;
        Ok(hash)
    }

    /// Get a Block
    pub async fn block(
        &self,
        hash: Option<T::Hash>,
    ) -> Result<Option<ChainBlockResponse<T>>, Error> {
        let params = rpc_params![hash];
        let block = self.client.request("chain_getBlock", params).await?;
        Ok(block)
    }

    /// Reexecute the specified `block_hash` and gather statistics while doing so.
    ///
    /// This function requires the specified block and its parent to be available
    /// at the queried node. If either the specified block or the parent is pruned,
    /// this function will return `None`.
    pub async fn block_stats(
        &self,
        block_hash: T::Hash,
    ) -> Result<Option<BlockStats>, Error> {
        let params = rpc_params![block_hash];
        let stats = self.client.request("dev_getBlockStats", params).await?;
        Ok(stats)
    }

    /// Get proof of storage entries at a specific block's state.
    pub async fn read_proof(
        &self,
        keys: impl IntoIterator<Item = &[u8]>,
        hash: Option<T::Hash>,
    ) -> Result<ReadProof<T::Hash>, Error> {
        let keys: Vec<String> = keys.into_iter().map(to_hex).collect();
        let params = rpc_params![keys, hash];
        let proof = self.client.request("state_getReadProof", params).await?;
        Ok(proof)
    }

    /// Fetch the runtime version
    pub async fn runtime_version(
        &self,
        at: Option<T::Hash>,
    ) -> Result<RuntimeVersion, Error> {
        let params = rpc_params![at];
        let version = self
            .client
            .request("state_getRuntimeVersion", params)
            .await?;
        Ok(version)
    }

    /// Subscribe to all new best block headers.
    pub async fn subscribe_best_block_headers(
        &self,
    ) -> Result<Subscription<T::Header>, Error> {
        let subscription = self
            .client
            .subscribe(
                // Despite the name, this returns a stream of all new blocks
                // imported by the node that happen to be added to the current best chain
                // (ie all best blocks).
                "chain_subscribeNewHeads",
                rpc_params![],
                "chain_unsubscribeNewHeads",
            )
            .await?;

        Ok(subscription)
    }

    /// Subscribe to all new block headers.
    pub async fn subscribe_all_block_headers(
        &self,
    ) -> Result<Subscription<T::Header>, Error> {
        let subscription = self
            .client
            .subscribe(
                // Despite the name, this returns a stream of all new blocks
                // imported by the node that happen to be added to the current best chain
                // (ie all best blocks).
                "chain_subscribeAllHeads",
                rpc_params![],
                "chain_unsubscribeAllHeads",
            )
            .await?;

        Ok(subscription)
    }

    /// Subscribe to finalized block headers.
    ///
    /// Note: this may not produce _every_ block in the finalized chain;
    /// sometimes multiple blocks are finalized at once, and in this case only the
    /// latest one is returned. the higher level APIs that use this "fill in" the
    /// gaps for us.
    pub async fn subscribe_finalized_block_headers(
        &self,
    ) -> Result<Subscription<T::Header>, Error> {
        let subscription = self
            .client
            .subscribe(
                "chain_subscribeFinalizedHeads",
                rpc_params![],
                "chain_unsubscribeFinalizedHeads",
            )
            .await?;
        Ok(subscription)
    }

    /// Subscribe to runtime version updates that produce changes in the metadata.
    pub async fn subscribe_runtime_version(
        &self,
    ) -> Result<Subscription<RuntimeVersion>, Error> {
        let subscription = self
            .client
            .subscribe(
                "state_subscribeRuntimeVersion",
                rpc_params![],
                "state_unsubscribeRuntimeVersion",
            )
            .await?;
        Ok(subscription)
    }

    /// Create and submit an extrinsic and return corresponding Hash if successful
    pub async fn submit_extrinsic<X: Encode>(
        &self,
        extrinsic: X,
    ) -> Result<T::Hash, Error> {
        let bytes: Bytes = extrinsic.encode().into();
        let params = rpc_params![bytes];
        let xt_hash = self
            .client
            .request("author_submitExtrinsic", params)
            .await?;
        Ok(xt_hash)
    }

    /// Create and submit an extrinsic and return a subscription to the events triggered.
    pub async fn watch_extrinsic<X: Encode>(
        &self,
        extrinsic: X,
    ) -> Result<Subscription<SubstrateTxStatus<T::Hash, T::Hash>>, Error> {
        let bytes: Bytes = extrinsic.encode().into();
        let params = rpc_params![bytes];
        let subscription = self
            .client
            .subscribe(
                "author_submitAndWatchExtrinsic",
                params,
                "author_unwatchExtrinsic",
            )
            .await?;
        Ok(subscription)
    }

    /// Insert a key into the keystore.
    pub async fn insert_key(
        &self,
        key_type: String,
        suri: String,
        public: Bytes,
    ) -> Result<(), Error> {
        let params = rpc_params![key_type, suri, public];
        self.client.request("author_insertKey", params).await?;
        Ok(())
    }

    /// Generate new session keys and returns the corresponding public keys.
    pub async fn rotate_keys(&self) -> Result<Bytes, Error> {
        self.client
            .request("author_rotateKeys", rpc_params![])
            .await
    }

    /// Checks if the keystore has private keys for the given session public keys.
    ///
    /// `session_keys` is the SCALE encoded session keys object from the runtime.
    ///
    /// Returns `true` iff all private keys could be found.
    pub async fn has_session_keys(&self, session_keys: Bytes) -> Result<bool, Error> {
        let params = rpc_params![session_keys];
        self.client.request("author_hasSessionKeys", params).await
    }

    /// Checks if the keystore has private keys for the given public key and key type.
    ///
    /// Returns `true` if a private key could be found.
    pub async fn has_key(
        &self,
        public_key: Bytes,
        key_type: String,
    ) -> Result<bool, Error> {
        let params = rpc_params![public_key, key_type];
        self.client.request("author_hasKey", params).await
    }

    /// Submits the extrinsic to the dry_run RPC, to test if it would succeed.
    ///
    /// Returns `Ok` with an [`ApplyExtrinsicResult`], which is the result of applying of an extrinsic.
    pub async fn dry_run(
        &self,
        encoded_signed: &[u8],
        at: Option<T::Hash>,
    ) -> Result<ApplyExtrinsicResult, Error> {
        let params = rpc_params![to_hex(encoded_signed), at];
        let result_bytes: Bytes = self.client.request("system_dryRun", params).await?;
        let data: ApplyExtrinsicResult =
            codec::Decode::decode(&mut result_bytes.0.as_slice())?;
        Ok(data)
    }
}

fn to_hex(bytes: impl AsRef<[u8]>) -> String {
    format!("0x{}", hex::encode(bytes.as_ref()))
}

#[cfg(test)]
mod test {
    use super::*;

    /// A util function to assert the result of serialization and deserialization is the same.
    pub(crate) fn assert_deser<T>(s: &str, expected: T)
    where
        T: std::fmt::Debug
            + serde::ser::Serialize
            + serde::de::DeserializeOwned
            + PartialEq,
    {
        assert_eq!(serde_json::from_str::<T>(s).unwrap(), expected);
        assert_eq!(serde_json::to_string(&expected).unwrap(), s);
    }

    #[test]
    fn test_deser_runtime_version() {
        let val: RuntimeVersion = serde_json::from_str(
            r#"{
            "specVersion": 123,
            "transactionVersion": 456,
            "foo": true,
            "wibble": [1,2,3]
        }"#,
        )
        .expect("deserializing failed");

        let mut m = std::collections::HashMap::new();
        m.insert("foo".to_owned(), serde_json::json!(true));
        m.insert("wibble".to_owned(), serde_json::json!([1, 2, 3]));

        assert_eq!(
            val,
            RuntimeVersion {
                spec_version: 123,
                transaction_version: 456,
                other: m
            }
        );
    }

    #[test]
    fn should_serialize_and_deserialize() {
        assert_deser(r#""0x1234""#, NumberOrHex::Hex(0x1234.into()));
        assert_deser(r#""0x0""#, NumberOrHex::Hex(0.into()));
        assert_deser(r#"5"#, NumberOrHex::Number(5));
        assert_deser(r#"10000"#, NumberOrHex::Number(10000));
        assert_deser(r#"0"#, NumberOrHex::Number(0));
        assert_deser(r#"1000000000000"#, NumberOrHex::Number(1000000000000));
    }
}
