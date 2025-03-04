use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use rstest::*;
use rstest_reuse::{self, *};

use crate::{
    binary_tree::LeafNodeIndex,
    framing::*,
    group::{config::CryptoConfig, errors::*, *},
    messages::{
        external_proposals::*,
        proposals::{AddProposal, Proposal, ProposalType},
    },
};

use openmls_traits::types::Ciphersuite;

use super::utils::*;

struct ProposalValidationTestSetup {
    alice_group: (MlsGroup, SignatureKeyPair),
    bob_group: (MlsGroup, SignatureKeyPair),
}

// Creates a standalone group
fn new_test_group(
    identity: &str,
    wire_format_policy: WireFormatPolicy,
    ciphersuite: Ciphersuite,
    backend: &impl OpenMlsCryptoProvider,
) -> (MlsGroup, CredentialWithKeyAndSigner) {
    let group_id = GroupId::from_slice(b"Test Group");

    // Generate credential bundles
    let credential_with_keys =
        generate_credential_bundle(identity.into(), ciphersuite.signature_algorithm(), backend);

    // Define the MlsGroup configuration
    let mls_group_config = MlsGroupConfig::builder()
        .wire_format_policy(wire_format_policy)
        .crypto_config(CryptoConfig::with_default_version(ciphersuite))
        .build();

    (
        MlsGroup::new_with_group_id(
            backend,
            &credential_with_keys.signer,
            &mls_group_config,
            group_id,
            credential_with_keys.credential_with_key.clone(),
        )
        .unwrap(),
        credential_with_keys,
    )
}

// Validation test setup
fn validation_test_setup(
    wire_format_policy: WireFormatPolicy,
    ciphersuite: Ciphersuite,
    backend: &impl OpenMlsCryptoProvider,
) -> ProposalValidationTestSetup {
    // === Alice creates a group ===
    let (mut alice_group, alice_signer_with_keys) =
        new_test_group("Alice", wire_format_policy, ciphersuite, backend);

    let bob_credential_bundle =
        generate_credential_bundle("Bob".into(), ciphersuite.signature_algorithm(), backend);

    let bob_key_package = generate_key_package(
        ciphersuite,
        Extensions::empty(),
        backend,
        bob_credential_bundle.clone(),
    );

    let (_message, welcome, _group_info) = alice_group
        .add_members(backend, &alice_signer_with_keys.signer, &[bob_key_package])
        .expect("error adding Bob to group");

    alice_group
        .merge_pending_commit(backend)
        .expect("error merging pending commit");

    // Define the MlsGroup configuration
    let mls_group_config = MlsGroupConfig::builder()
        .wire_format_policy(wire_format_policy)
        .crypto_config(CryptoConfig::with_default_version(ciphersuite))
        .build();

    let bob_group = MlsGroup::new_from_welcome(
        backend,
        &mls_group_config,
        welcome.into_welcome().expect("Unexpected message type."),
        Some(alice_group.export_ratchet_tree().into()),
    )
    .expect("error creating group from welcome");

    ProposalValidationTestSetup {
        alice_group: (alice_group, alice_signer_with_keys.signer),
        bob_group: (bob_group, bob_credential_bundle.signer),
    }
}

#[apply(ciphersuites_and_backends)]
fn external_add_proposal_should_succeed(
    ciphersuite: Ciphersuite,
    backend: &impl OpenMlsCryptoProvider,
) {
    for policy in WIRE_FORMAT_POLICIES {
        let ProposalValidationTestSetup {
            alice_group,
            bob_group,
        } = validation_test_setup(policy, ciphersuite, backend);
        let (mut alice_group, alice_signer) = alice_group;
        let (mut bob_group, _bob_signer) = bob_group;

        assert_eq!(alice_group.members().count(), 2);
        assert_eq!(bob_group.members().count(), 2);

        // A new client, Charlie, will now ask joining with an external Add proposal
        let charlie_credential = generate_credential_bundle(
            "Charlie".into(),
            ciphersuite.signature_algorithm(),
            backend,
        );

        let charlie_kp = generate_key_package(
            ciphersuite,
            Extensions::empty(),
            backend,
            charlie_credential.clone(),
        );

        let proposal = JoinProposal::new(
            charlie_kp.clone(),
            alice_group.group_id().clone(),
            alice_group.epoch(),
            &charlie_credential.signer,
        )
        .unwrap();

        // an external proposal is always plaintext and has sender type 'new_member_proposal'
        let verify_proposal = |msg: &PublicMessage| {
            *msg.sender() == Sender::NewMemberProposal
                && msg.content_type() == ContentType::Proposal
                && matches!(msg.content(), FramedContentBody::Proposal(p) if p.proposal_type() == ProposalType::Add)
        };
        assert!(
            matches!(proposal.body, MlsMessageOutBody::PublicMessage(ref msg) if verify_proposal(msg))
        );

        let msg = alice_group
            .process_message(backend, proposal.clone().into_protocol_message().unwrap())
            .unwrap();

        match msg.into_content() {
            ProcessedMessageContent::ExternalJoinProposalMessage(proposal) => {
                assert!(matches!(proposal.sender(), Sender::NewMemberProposal));
                assert!(matches!(
                    proposal.proposal(),
                    Proposal::Add(AddProposal { key_package }) if key_package == &charlie_kp
                ));
                alice_group.store_pending_proposal(*proposal)
            }
            _ => unreachable!(),
        }

        let msg = bob_group
            .process_message(backend, proposal.into_protocol_message().unwrap())
            .unwrap();

        match msg.into_content() {
            ProcessedMessageContent::ExternalJoinProposalMessage(proposal) => {
                bob_group.store_pending_proposal(*proposal)
            }
            _ => unreachable!(),
        }

        // and Alice will commit it
        let (commit, welcome, _group_info) = alice_group
            .commit_to_pending_proposals(backend, &alice_signer)
            .unwrap();
        alice_group.merge_pending_commit(backend).unwrap();
        assert_eq!(alice_group.members().count(), 3);

        // Bob will also process the commit
        let msg = bob_group
            .process_message(backend, commit.into_protocol_message().unwrap())
            .unwrap();
        match msg.into_content() {
            ProcessedMessageContent::StagedCommitMessage(commit) => {
                bob_group.merge_staged_commit(backend, *commit).unwrap()
            }
            _ => unreachable!(),
        }
        assert_eq!(bob_group.members().count(), 3);

        // Finally, Charlie can join with the Welcome
        let cfg = MlsGroupConfig::builder()
            .wire_format_policy(policy)
            .crypto_config(CryptoConfig::with_default_version(ciphersuite))
            .build();
        let charlie_group = MlsGroup::new_from_welcome(
            backend,
            &cfg,
            welcome.unwrap().into_welcome().unwrap(),
            Some(alice_group.export_ratchet_tree().into()),
        )
        .unwrap();
        assert_eq!(charlie_group.members().count(), 3);
    }
}

#[apply(ciphersuites_and_backends)]
fn external_add_proposal_should_be_signed_by_key_package_it_references(
    ciphersuite: Ciphersuite,
    backend: &impl OpenMlsCryptoProvider,
) {
    let ProposalValidationTestSetup { alice_group, .. } =
        validation_test_setup(PURE_PLAINTEXT_WIRE_FORMAT_POLICY, ciphersuite, backend);
    let (mut alice_group, _alice_signer) = alice_group;

    let attacker_credential = generate_credential_bundle(
        "Attacker".into(),
        ciphersuite.signature_algorithm(),
        backend,
    );

    // A new client, Charlie, will now ask joining with an external Add proposal
    let charlie_credential =
        generate_credential_bundle("Charlie".into(), ciphersuite.signature_algorithm(), backend);

    let charlie_kp = generate_key_package(
        ciphersuite,
        Extensions::empty(),
        backend,
        attacker_credential,
    );

    let invalid_proposal = JoinProposal::new(
        charlie_kp,
        alice_group.group_id().clone(),
        alice_group.epoch(),
        &charlie_credential.signer,
    )
    .unwrap();

    // fails because the message was not signed by the same credential as the one in the Add proposal
    assert!(matches!(
        alice_group
            .process_message(backend, invalid_proposal.into_protocol_message().unwrap())
            .unwrap_err(),
        ProcessMessageError::InvalidSignature
    ));
}

// TODO #1093: move this test to a dedicated external proposal ValSem test module once all external proposals implemented
#[apply(ciphersuites_and_backends)]
fn new_member_proposal_sender_should_be_reserved_for_join_proposals(
    ciphersuite: Ciphersuite,
    backend: &impl OpenMlsCryptoProvider,
) {
    let ProposalValidationTestSetup {
        alice_group,
        bob_group,
    } = validation_test_setup(PURE_PLAINTEXT_WIRE_FORMAT_POLICY, ciphersuite, backend);
    let (mut alice_group, alice_signer) = alice_group;
    let (mut bob_group, _bob_signer) = bob_group;

    // Add proposal can have a 'new_member_proposal' sender
    let any_credential =
        generate_credential_bundle("Any".into(), ciphersuite.signature_algorithm(), backend);

    let any_kp = generate_key_package(
        ciphersuite,
        Extensions::empty(),
        backend,
        any_credential.clone(),
    );

    let join_proposal = JoinProposal::new(
        any_kp,
        alice_group.group_id().clone(),
        alice_group.epoch(),
        &any_credential.signer,
    )
    .unwrap();

    if let MlsMessageOutBody::PublicMessage(plaintext) = &join_proposal.body {
        // Make sure it's an add proposal...
        assert!(matches!(
            plaintext.content(),
            FramedContentBody::Proposal(Proposal::Add(_))
        ));

        // ... and that it has the right sender type
        assert!(matches!(plaintext.sender(), Sender::NewMemberProposal));

        // Finally check that the message can be processed without errors
        assert!(bob_group
            .process_message(backend, join_proposal.into_protocol_message().unwrap())
            .is_ok());
    } else {
        panic!()
    };
    alice_group.clear_pending_proposals();

    // Remove proposal cannot have a 'new_member_proposal' sender
    let remove_proposal = alice_group
        .propose_remove_member(backend, &alice_signer, LeafNodeIndex::new(1))
        .map(|(out, _)| MlsMessageIn::from(out))
        .unwrap();
    if let MlsMessageInBody::PublicMessage(mut plaintext) = remove_proposal.body {
        plaintext.set_sender(Sender::NewMemberProposal);
        assert!(matches!(
            bob_group.process_message(backend, plaintext).unwrap_err(),
            ProcessMessageError::ValidationError(ValidationError::NotAnExternalAddProposal)
        ));
    } else {
        panic!()
    };
    alice_group.clear_pending_proposals();

    // Update proposal cannot have a 'new_member_proposal' sender
    let update_proposal = alice_group
        .propose_self_update(backend, &alice_signer, None)
        .map(|(out, _)| MlsMessageIn::from(out))
        .unwrap();
    if let MlsMessageInBody::PublicMessage(mut plaintext) = update_proposal.body {
        plaintext.set_sender(Sender::NewMemberProposal);
        assert!(matches!(
            bob_group.process_message(backend, plaintext).unwrap_err(),
            ProcessMessageError::ValidationError(ValidationError::NotAnExternalAddProposal)
        ));
    } else {
        panic!()
    };
    alice_group.clear_pending_proposals();
}
