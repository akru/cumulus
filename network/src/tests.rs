// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use super::*;
use cumulus_test_runtime::{Block, Header};
use futures::executor::block_on;
use polkadot_node_primitives::{SignedFullStatement, Statement};
use polkadot_primitives::v1::{
	Block as PBlock, BlockNumber, CandidateCommitments, CandidateDescriptor, CandidateEvent,
	CommittedCandidateReceipt, CoreState, GroupRotationInfo, Hash as PHash, Header as PHeader,
	Id as ParaId, OccupiedCoreAssumption, ParachainHost, PersistedValidationData, SessionIndex,
	SigningContext, ValidationCode, ValidationData, ValidatorId, ValidatorIndex,
};
use polkadot_test_runtime_client::{
	DefaultTestClientBuilderExt, TestClient, TestClientBuilder, TestClientBuilderExt,
};
use sp_api::{ApiRef, ProvideRuntimeApi};
use sp_blockchain::{Error as ClientError, HeaderBackend};
use sp_consensus::block_validation::BlockAnnounceValidator;
use sp_core::H256;
use sp_keyring::Sr25519Keyring;
use sp_runtime::traits::{NumberFor, Zero};

#[derive(Clone)]
struct DummyCollatorNetwork;

impl SyncOracle for DummyCollatorNetwork {
	fn is_major_syncing(&mut self) -> bool {
		false
	}

	fn is_offline(&mut self) -> bool {
		unimplemented!("Not required in tests")
	}
}

fn make_validator() -> JustifiedBlockAnnounceValidator<Block, TestApi> {
	let (validator, _client) = make_validator_and_client();

	validator
}

fn make_validator_and_client() -> (
	JustifiedBlockAnnounceValidator<Block, TestApi>,
	Arc<TestApi>,
) {
	let builder = TestClientBuilder::new();
	let client = Arc::new(TestApi::new(Arc::new(builder.build())));

	(
		JustifiedBlockAnnounceValidator::new(
			client.clone(),
			ParaId::from(56),
			Box::new(DummyCollatorNetwork),
		),
		client,
	)
}

fn default_header() -> Header {
	Header {
		number: 1,
		digest: Default::default(),
		extrinsics_root: Default::default(),
		parent_hash: Default::default(),
		state_root: Default::default(),
	}
}

fn make_gossip_message_and_header(
	client: Arc<TestApi>,
	relay_parent: H256,
	validator_index: u32,
) -> (SignedFullStatement, Header) {
	let key = Sr25519Keyring::Alice.pair().into();
	let session_index = client
		.runtime_api()
		.session_index_for_child(&BlockId::Hash(relay_parent))
		.unwrap();
	let signing_context = SigningContext {
		parent_hash: relay_parent,
		session_index,
	};

	let header = default_header();
	let candidate_receipt = CommittedCandidateReceipt {
		commitments: CandidateCommitments {
			head_data: header.encode().into(),
			..Default::default()
		},
		descriptor: CandidateDescriptor {
			relay_parent,
			..Default::default()
		},
	};
	let statement = Statement::Seconded(candidate_receipt);

	(
		SignedFullStatement::sign(statement, &signing_context, validator_index, &key),
		header,
	)
}

#[test]
fn valid_if_no_data_and_less_than_best_known_number() {
	let mut validator = make_validator();
	let header = Header {
		number: 0,
		..default_header()
	};
	let res = block_on(validator.validate(&header, &[]));

	assert_eq!(
		res.unwrap(),
		Validation::Success { is_new_best: false },
		"validating without data with block number < best known number is always a success",
	);
}

#[test]
fn invalid_if_no_data_exceeds_best_known_number() {
	let mut validator = make_validator();
	let header = Header {
		number: 1,
		..default_header()
	};
	let res = block_on(validator.validate(&header, &[]));

	assert_eq!(
		res.unwrap(),
		Validation::Failure,
		"validation fails if no justification and block number >= best known number",
	);
}

#[test]
fn check_statement_is_encoded_correctly() {
	let mut validator = make_validator();
	let header = default_header();
	let res = block_on(validator.validate(&header, &[0x42]))
		.err()
		.expect("Should fail on invalid encoded statement");

	assert!(matches!(
		*res.downcast::<ClientError>().unwrap(),
		ClientError::BadJustification(x) if x.contains("must be a `SignedFullStatement`"),
	));
}

#[test]
fn check_relay_parent_is_head() {
	let (mut validator, client) = make_validator_and_client();
	let relay_chain_leaf = H256::zero();
	let (gossip_message, header) = make_gossip_message_and_header(client, relay_chain_leaf, 0);
	let data = gossip_message.encode();
	let res = block_on(validator.validate(&header, data.as_slice()));

	assert_eq!(
		res.unwrap(),
		Validation::Failure,
		"validation fails if the relay chain parent is not the relay chain head",
	);
}

#[test]
fn check_relay_parent_actually_exists() {
	let (mut validator, client) = make_validator_and_client();
	let relay_parent = H256::from_low_u64_be(42);
	let (signed_statement, header) = make_gossip_message_and_header(client, relay_parent, 0);
	let data = signed_statement.encode();
	let res = block_on(validator.validate(&header, &data))
		.err()
		.expect("Should fail on unknown relay parent");

	assert!(matches!(
		*res.downcast::<ClientError>().unwrap(),
		ClientError::UnknownBlock(_),
	));
}

#[test]
fn check_relay_parent_fails_if_cannot_retrieve_number() {
	let (mut validator, client) = make_validator_and_client();
	let relay_parent = H256::from_low_u64_be(0xdead);
	let (signed_statement, header) = make_gossip_message_and_header(client, relay_parent, 0);
	let data = signed_statement.encode();
	let res = block_on(validator.validate(&header, &data))
		.err()
		.expect("Should fail when the relay chain number could not be retrieved");

	assert!(matches!(
		*res.downcast::<ClientError>().unwrap(),
		ClientError::Backend(_),
	));
}

#[test]
fn check_signer_is_legit_validator() {
	let (mut validator, client) = make_validator_and_client();
	let relay_parent = H256::from_low_u64_be(1);

	let (signed_statement, header) = make_gossip_message_and_header(client, relay_parent, 1);
	let data = signed_statement.encode();

	let res = block_on(validator.validate(&header, &data))
		.err()
		.expect("Should fail on invalid validator");

	assert!(matches!(
		*res.downcast::<ClientError>().unwrap(),
		ClientError::BadJustification(x) if x.contains("signer is a validator")
	));
}

#[test]
fn check_statement_is_correctly_signed() {
	let (mut validator, client) = make_validator_and_client();
	let relay_parent = H256::from_low_u64_be(1);

	let (signed_statement, header) = make_gossip_message_and_header(client, relay_parent, 0);

	let mut data = signed_statement.encode();

	// The signature comes at the end of the type, so change a bit to make the signature invalid.
	let last = data.len() - 1;
	data[last] = data[last].wrapping_add(1);

	let res = block_on(validator.validate(&header, &data))
		.err()
		.expect("Validation should fail if the statement is not signed correctly");

	assert!(matches!(
		*res.downcast::<ClientError>().unwrap(),
		ClientError::BadJustification(x) if x.contains("signature is invalid")
	));
}

#[test]
fn check_statement_seconded() {
	let (mut validator, client) = make_validator_and_client();
	let header = default_header();
	let relay_parent = H256::from_low_u64_be(1);

	let key = Sr25519Keyring::Alice.pair().into();
	let session_index = client
		.runtime_api()
		.session_index_for_child(&BlockId::Hash(relay_parent))
		.unwrap();
	let signing_context = SigningContext {
		parent_hash: relay_parent,
		session_index,
	};

	let statement = Statement::Valid(H256::zero());

	let signed_statement = SignedFullStatement::sign(statement, &signing_context, 0, &key);
	let data = signed_statement.encode();

	let res = block_on(validator.validate(&header, &data))
		.err()
		.expect("validation should fail if not seconded statement");

	assert!(matches!(
		*res.downcast::<ClientError>().unwrap(),
		ClientError::BadJustification(x) if x.contains("must be a `Statement::Seconded`")
	));
}

#[test]
fn check_header_match_candidate_receipt_header() {
	let (mut validator, client) = make_validator_and_client();
	let relay_parent = H256::from_low_u64_be(1);

	let (signed_statement, mut header) = make_gossip_message_and_header(client, relay_parent, 0);
	let data = signed_statement.encode();
	header.number = 300;

	let res = block_on(validator.validate(&header, &data))
		.err()
		.expect("validation should fail if the header in doesn't match");

	assert!(matches!(
		*res.downcast::<ClientError>().unwrap(),
		ClientError::BadJustification(x) if x.contains("header does not match")
	));
}

#[derive(Default)]
struct ApiData {
	validators: Vec<ValidatorId>,
}

#[derive(Clone)]
struct TestApi {
	data: Arc<Mutex<ApiData>>,
	client: Arc<TestClient>,
}

impl TestApi {
	fn new(client: Arc<TestClient>) -> Self {
		Self {
			client,
			data: Arc::new(Mutex::new(ApiData {
				validators: vec![Sr25519Keyring::Alice.public().into()],
				..Default::default()
			})),
		}
	}
}

#[derive(Default)]
struct RuntimeApi {
	data: Arc<Mutex<ApiData>>,
}

impl ProvideRuntimeApi<PBlock> for TestApi {
	type Api = RuntimeApi;

	fn runtime_api<'a>(&'a self) -> ApiRef<'a, Self::Api> {
		RuntimeApi {
			data: self.data.clone(),
		}
		.into()
	}
}

sp_api::mock_impl_runtime_apis! {
	impl ParachainHost<PBlock> for RuntimeApi {
		type Error = sp_blockchain::Error;

		fn validators(&self) -> Vec<ValidatorId> {
			self.data.lock().validators.clone()
		}

		fn validator_groups(&self) -> (Vec<Vec<ValidatorIndex>>, GroupRotationInfo<BlockNumber>) {
			(Vec::new(), GroupRotationInfo { session_start_block: 0, group_rotation_frequency: 0, now: 0 })
		}

		fn availability_cores(&self) -> Vec<CoreState<BlockNumber>> {
			Vec::new()
		}

		fn full_validation_data(&self, _: ParaId, _: OccupiedCoreAssumption) -> Option<ValidationData<BlockNumber>> {
			None
		}

		fn persisted_validation_data(&self, _: ParaId, _: OccupiedCoreAssumption) -> Option<PersistedValidationData<BlockNumber>> {
			Some(PersistedValidationData {
				parent_head: HeadData::<Block> { header: default_header() }.encode().into(),
				..Default::default()
			})
		}

		fn session_index_for_child(&self) -> SessionIndex {
			0
		}

		fn validation_code(&self, _: ParaId, _: OccupiedCoreAssumption) -> Option<ValidationCode> {
			None
		}

		fn candidate_pending_availability(&self, _: ParaId) -> Option<CommittedCandidateReceipt<PHash>> {
			None
		}

		fn candidate_events(&self) -> Vec<CandidateEvent<PHash>> {
			Vec::new()
		}
	}
}

/// Blockchain database header backend. Does not perform any validation.
impl HeaderBackend<PBlock> for TestApi {
	fn header(
		&self,
		_id: BlockId<PBlock>,
	) -> std::result::Result<Option<PHeader>, sp_blockchain::Error> {
		Ok(None)
	}

	fn info(&self) -> sc_client_api::blockchain::Info<PBlock> {
		let best_hash = H256::from_low_u64_be(1);

		sc_client_api::blockchain::Info {
			best_hash,
			best_number: 1,
			finalized_hash: Default::default(),
			finalized_number: Zero::zero(),
			genesis_hash: Default::default(),
			number_leaves: Default::default(),
		}
	}

	fn status(
		&self,
		_id: BlockId<PBlock>,
	) -> std::result::Result<sc_client_api::blockchain::BlockStatus, sp_blockchain::Error> {
		Ok(sc_client_api::blockchain::BlockStatus::Unknown)
	}

	fn number(
		&self,
		hash: PHash,
	) -> std::result::Result<Option<NumberFor<PBlock>>, sp_blockchain::Error> {
		if hash == H256::zero() {
			Ok(Some(0))
		} else if hash == H256::from_low_u64_be(1) {
			Ok(Some(1))
		} else if hash == H256::from_low_u64_be(0xdead) {
			Err(sp_blockchain::Error::Backend("dead".to_string()))
		} else {
			Ok(None)
		}
	}

	fn hash(
		&self,
		_number: NumberFor<PBlock>,
	) -> std::result::Result<Option<PHash>, sp_blockchain::Error> {
		Ok(None)
	}
}
