//! This module contains tests regarding the use of [`MessageSecretsStore`] in [`MlsGroup`]

use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::{types::Ciphersuite, OpenMlsCryptoProvider};

use rstest::*;
use rstest_reuse::{self, *};

use super::utils::{generate_credential_bundle, generate_key_package};
use crate::{
    framing::{MessageDecryptionError, ProcessedMessageContent},
    group::{config::CryptoConfig, errors::*, *},
};

#[apply(ciphersuites_and_backends)]
fn test_past_secrets_in_group(ciphersuite: Ciphersuite, backend: &impl OpenMlsCryptoProvider) {
    // Test this for different parameters
    for max_epochs in (0..10usize).step_by(2) {
        let group_id = GroupId::from_slice(b"Test Group");

        // Generate credentials
        let alice_credential_with_keys = generate_credential_bundle(
            b"Alice".to_vec(),
            ciphersuite.signature_algorithm(),
            backend,
        );
        let bob_credential_with_keys =
            generate_credential_bundle(b"Bob".to_vec(), ciphersuite.signature_algorithm(), backend);

        // Generate KeyPackages
        let bob_key_package = generate_key_package(
            ciphersuite,
            Extensions::empty(),
            backend,
            bob_credential_with_keys,
        );

        // Define the MlsGroup configuration

        let mls_group_config = MlsGroupConfig::builder()
            .max_past_epochs(max_epochs / 2)
            .crypto_config(CryptoConfig::with_default_version(ciphersuite))
            .build();

        // === Alice creates a group ===
        let mut alice_group = MlsGroup::new_with_group_id(
            backend,
            &alice_credential_with_keys.signer,
            &mls_group_config,
            group_id.clone(),
            alice_credential_with_keys.credential_with_key.clone(),
        )
        .expect("An unexpected error occurred.");

        // Alice adds Bob
        let (_message, welcome, _group_info) = alice_group
            .add_members(
                backend,
                &alice_credential_with_keys.signer,
                &[bob_key_package],
            )
            .expect("An unexpected error occurred.");

        alice_group
            .merge_pending_commit(backend)
            .expect("error merging pending commit");

        let mut bob_group = MlsGroup::new_from_welcome(
            backend,
            &mls_group_config,
            welcome.into_welcome().expect("Unexpected message type."),
            Some(alice_group.export_ratchet_tree().into()),
        )
        .expect("Error creating group from Welcome");

        // Generate application message for different epochs

        let mut application_messages = Vec::new();
        let mut update_commits = Vec::new();

        for _ in 0..max_epochs {
            let application_message = alice_group
                .create_message(backend, &alice_credential_with_keys.signer, &[1, 2, 3])
                .expect("An unexpected error occurred.");

            application_messages.push(application_message.into_protocol_message().unwrap());

            let (message, _welcome, _group_info) = alice_group
                .self_update(backend, &alice_credential_with_keys.signer)
                .expect("An unexpected error occurred.");

            update_commits.push(message.clone());

            alice_group
                .merge_pending_commit(backend)
                .expect("error merging pending commit");
        }

        // Bob processes all update commits

        for update_commit in update_commits {
            let bob_processed_message = bob_group
                .process_message(backend, update_commit.into_protocol_message().unwrap())
                .expect("An unexpected error occurred.");

            if let ProcessedMessageContent::StagedCommitMessage(staged_commit) =
                bob_processed_message.into_content()
            {
                bob_group
                    .merge_staged_commit(backend, *staged_commit)
                    .expect("Error merging commit.");
            } else {
                unreachable!("Expected a StagedCommit.");
            }
        }

        // === Test application messages from older epochs ===

        // The first messages should fail
        for application_message in application_messages.iter().take(max_epochs / 2) {
            let err = bob_group
                .process_message(backend, application_message.clone())
                .expect_err("An unexpected error occurred.");
            assert_eq!(
                err,
                ProcessMessageError::ValidationError(ValidationError::UnableToDecrypt(
                    MessageDecryptionError::AeadError
                ),)
            );
        }

        // The last messages should not fail
        for application_message in application_messages.iter().skip(max_epochs / 2) {
            let bob_processed_message = bob_group
                .process_message(backend, application_message.clone())
                .expect("An unexpected error occurred.");

            if let ProcessedMessageContent::ApplicationMessage(application_message) =
                bob_processed_message.into_content()
            {
                assert_eq!(application_message.into_bytes(), &[1, 2, 3]);
            } else {
                unreachable!("Expected an ApplicationMessage.");
            }
        }
    }
}
