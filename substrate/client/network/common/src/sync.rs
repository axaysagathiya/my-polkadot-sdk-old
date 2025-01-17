// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Abstract interfaces and data structures related to network sync.

pub mod message;
pub mod metrics;
pub mod warp;

use crate::{role::Roles, types::ReputationChange};
use futures::Stream;

use libp2p_identity::PeerId;

use message::{BlockAnnounce, BlockRequest, BlockResponse};
use sc_consensus::{import_queue::RuntimeOrigin, IncomingBlock};
use sp_consensus::BlockOrigin;
use sp_runtime::{
	traits::{Block as BlockT, NumberFor},
	Justifications,
};
use warp::WarpSyncProgress;

use std::{any::Any, fmt, fmt::Formatter, pin::Pin, sync::Arc};

/// The sync status of a peer we are trying to sync with
#[derive(Debug)]
pub struct PeerInfo<Block: BlockT> {
	/// Their best block hash.
	pub best_hash: Block::Hash,
	/// Their best block number.
	pub best_number: NumberFor<Block>,
}

/// Info about a peer's known state (both full and light).
#[derive(Clone, Debug)]
pub struct ExtendedPeerInfo<B: BlockT> {
	/// Roles
	pub roles: Roles,
	/// Peer best block hash
	pub best_hash: B::Hash,
	/// Peer best block number
	pub best_number: NumberFor<B>,
}

/// Reported sync state.
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum SyncState<BlockNumber> {
	/// Initial sync is complete, keep-up sync is active.
	Idle,
	/// Actively catching up with the chain.
	Downloading { target: BlockNumber },
	/// All blocks are downloaded and are being imported.
	Importing { target: BlockNumber },
}

impl<BlockNumber> SyncState<BlockNumber> {
	/// Are we actively catching up with the chain?
	pub fn is_major_syncing(&self) -> bool {
		!matches!(self, SyncState::Idle)
	}
}

/// Reported state download progress.
#[derive(Clone, Eq, PartialEq, Debug)]
pub struct StateDownloadProgress {
	/// Estimated download percentage.
	pub percentage: u32,
	/// Total state size in bytes downloaded so far.
	pub size: u64,
}

/// Syncing status and statistics.
#[derive(Debug, Clone)]
pub struct SyncStatus<Block: BlockT> {
	/// Current global sync state.
	pub state: SyncState<NumberFor<Block>>,
	/// Target sync block number.
	pub best_seen_block: Option<NumberFor<Block>>,
	/// Number of peers participating in syncing.
	pub num_peers: u32,
	/// Number of peers known to `SyncingEngine` (both full and light).
	pub num_connected_peers: u32,
	/// Number of blocks queued for import
	pub queued_blocks: u32,
	/// State sync status in progress, if any.
	pub state_sync: Option<StateDownloadProgress>,
	/// Warp sync in progress, if any.
	pub warp_sync: Option<WarpSyncProgress<Block>>,
}

/// A peer did not behave as expected and should be reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadPeer(pub PeerId, pub ReputationChange);

impl fmt::Display for BadPeer {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "Bad peer {}; Reputation change: {:?}", self.0, self.1)
	}
}

impl std::error::Error for BadPeer {}

/// Result of [`ChainSync::on_block_data`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnBlockData<Block: BlockT> {
	/// The block should be imported.
	Import(BlockOrigin, Vec<IncomingBlock<Block>>),
	/// A new block request needs to be made to the given peer.
	Request(PeerId, BlockRequest<Block>),
	/// Continue processing events.
	Continue,
}

/// Result of [`ChainSync::on_block_justification`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnBlockJustification<Block: BlockT> {
	/// The justification needs no further handling.
	Nothing,
	/// The justification should be imported.
	Import {
		peer: PeerId,
		hash: Block::Hash,
		number: NumberFor<Block>,
		justifications: Justifications,
	},
}

/// Result of `ChainSync::on_state_data`.
#[derive(Debug)]
pub enum OnStateData<Block: BlockT> {
	/// The block and state that should be imported.
	Import(BlockOrigin, IncomingBlock<Block>),
	/// A new state request needs to be made to the given peer.
	Continue,
}

/// Block or justification request polled from `ChainSync`
#[derive(Debug)]
pub enum ImportResult<B: BlockT> {
	BlockImport(BlockOrigin, Vec<IncomingBlock<B>>),
	JustificationImport(RuntimeOrigin, B::Hash, NumberFor<B>, Justifications),
}

/// Sync operation mode.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SyncMode {
	/// Full block download and verification.
	Full,
	/// Download blocks and the latest state.
	LightState {
		/// Skip state proof download and verification.
		skip_proofs: bool,
		/// Download indexed transactions for recent blocks.
		storage_chain_mode: bool,
	},
	/// Warp sync - verify authority set transitions and the latest state.
	Warp,
}

impl SyncMode {
	/// Returns `true` if `self` is [`Self::Warp`].
	pub fn is_warp(&self) -> bool {
		matches!(self, Self::Warp)
	}

	/// Returns `true` if `self` is [`Self::LightState`].
	pub fn light_state(&self) -> bool {
		matches!(self, Self::LightState { .. })
	}
}

impl Default for SyncMode {
	fn default() -> Self {
		Self::Full
	}
}
#[derive(Debug)]
pub struct Metrics {
	pub queued_blocks: u32,
	pub fork_targets: u32,
	pub justifications: metrics::Metrics,
}

#[derive(Debug)]
pub enum PeerRequest<B: BlockT> {
	Block(BlockRequest<B>),
	State,
	WarpProof,
}

#[derive(Debug)]
pub enum PeerRequestType {
	Block,
	State,
	WarpProof,
}

impl<B: BlockT> PeerRequest<B> {
	pub fn get_type(&self) -> PeerRequestType {
		match self {
			PeerRequest::Block(_) => PeerRequestType::Block,
			PeerRequest::State => PeerRequestType::State,
			PeerRequest::WarpProof => PeerRequestType::WarpProof,
		}
	}
}

/// Wrapper for implementation-specific state request.
///
/// NOTE: Implementation must be able to encode and decode it for network purposes.
pub struct OpaqueStateRequest(pub Box<dyn Any + Send>);

impl fmt::Debug for OpaqueStateRequest {
	fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
		f.debug_struct("OpaqueStateRequest").finish()
	}
}

/// Wrapper for implementation-specific state response.
///
/// NOTE: Implementation must be able to encode and decode it for network purposes.
pub struct OpaqueStateResponse(pub Box<dyn Any + Send>);

impl fmt::Debug for OpaqueStateResponse {
	fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
		f.debug_struct("OpaqueStateResponse").finish()
	}
}

/// Provides high-level status of syncing.
#[async_trait::async_trait]
pub trait SyncStatusProvider<Block: BlockT>: Send + Sync {
	/// Get high-level view of the syncing status.
	async fn status(&self) -> Result<SyncStatus<Block>, ()>;
}

#[async_trait::async_trait]
impl<T, Block> SyncStatusProvider<Block> for Arc<T>
where
	T: ?Sized,
	T: SyncStatusProvider<Block>,
	Block: BlockT,
{
	async fn status(&self) -> Result<SyncStatus<Block>, ()> {
		T::status(self).await
	}
}

/// Syncing-related events that other protocols can subscribe to.
pub enum SyncEvent {
	/// Peer that the syncing implementation is tracking connected.
	PeerConnected(PeerId),

	/// Peer that the syncing implementation was tracking disconnected.
	PeerDisconnected(PeerId),
}

pub trait SyncEventStream: Send + Sync {
	/// Subscribe to syncing-related events.
	fn event_stream(&self, name: &'static str) -> Pin<Box<dyn Stream<Item = SyncEvent> + Send>>;
}

impl<T> SyncEventStream for Arc<T>
where
	T: ?Sized,
	T: SyncEventStream,
{
	fn event_stream(&self, name: &'static str) -> Pin<Box<dyn Stream<Item = SyncEvent> + Send>> {
		T::event_stream(self, name)
	}
}

/// Something that represents the syncing strategy to download past and future blocks of the chain.
pub trait ChainSync<Block: BlockT>: Send {
	/// Returns the state of the sync of the given peer.
	///
	/// Returns `None` if the peer is unknown.
	fn peer_info(&self, who: &PeerId) -> Option<PeerInfo<Block>>;

	/// Returns the current sync status.
	fn status(&self) -> SyncStatus<Block>;

	/// Number of active forks requests. This includes
	/// requests that are pending or could be issued right away.
	fn num_sync_requests(&self) -> usize;

	/// Number of downloaded blocks.
	fn num_downloaded_blocks(&self) -> usize;

	/// Returns the current number of peers stored within this state machine.
	fn num_peers(&self) -> usize;

	/// Handle a new connected peer.
	///
	/// Call this method whenever we connect to a new peer.
	fn new_peer(
		&mut self,
		who: PeerId,
		best_hash: Block::Hash,
		best_number: NumberFor<Block>,
	) -> Result<Option<BlockRequest<Block>>, BadPeer>;

	/// Signal that a new best block has been imported.
	fn update_chain_info(&mut self, best_hash: &Block::Hash, best_number: NumberFor<Block>);

	/// Schedule a justification request for the given block.
	fn request_justification(&mut self, hash: &Block::Hash, number: NumberFor<Block>);

	/// Clear all pending justification requests.
	fn clear_justification_requests(&mut self);

	/// Request syncing for the given block from given set of peers.
	fn set_sync_fork_request(
		&mut self,
		peers: Vec<PeerId>,
		hash: &Block::Hash,
		number: NumberFor<Block>,
	);

	/// Handle a response from the remote to a block request that we made.
	///
	/// `request` must be the original request that triggered `response`.
	/// or `None` if data comes from the block announcement.
	///
	/// If this corresponds to a valid block, this outputs the block that
	/// must be imported in the import queue.
	fn on_block_data(
		&mut self,
		who: &PeerId,
		request: Option<BlockRequest<Block>>,
		response: BlockResponse<Block>,
	) -> Result<OnBlockData<Block>, BadPeer>;

	/// Handle a response from the remote to a justification request that we made.
	///
	/// `request` must be the original request that triggered `response`.
	fn on_block_justification(
		&mut self,
		who: PeerId,
		response: BlockResponse<Block>,
	) -> Result<OnBlockJustification<Block>, BadPeer>;

	/// Call this when a justification has been processed by the import queue,
	/// with or without errors.
	fn on_justification_import(
		&mut self,
		hash: Block::Hash,
		number: NumberFor<Block>,
		success: bool,
	);

	/// Notify about finalization of the given block.
	fn on_block_finalized(&mut self, hash: &Block::Hash, number: NumberFor<Block>);

	/// Notify about pre-validated block announcement.
	fn on_validated_block_announce(
		&mut self,
		is_best: bool,
		who: PeerId,
		announce: &BlockAnnounce<Block::Header>,
	);

	/// Call when a peer has disconnected.
	/// Canceled obsolete block request may result in some blocks being ready for
	/// import, so this functions checks for such blocks and returns them.
	fn peer_disconnected(&mut self, who: &PeerId);

	/// Return some key metrics.
	fn metrics(&self) -> Metrics;
}
