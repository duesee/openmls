//! Unit tests for the ciphersuites.
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::types::HpkeCiphertext;

use crate::{ciphersuite::*, test_utils::*};

// Spot test to make sure hpke seal/open work.
#[apply(ciphersuites_and_backends)]
fn test_hpke_seal_open(ciphersuite: Ciphersuite, backend: &impl OpenMlsCryptoProvider) {
    let plaintext = &[1, 2, 3];
    let kp = backend.crypto().derive_hpke_keypair(
        ciphersuite.hpke_config(),
        Secret::random(ciphersuite, backend, None)
            .expect("Not enough randomness.")
            .as_slice(),
    );
    let ciphertext = hpke::encrypt_with_label(
        &kp.public,
        "label",
        &[1, 2, 3],
        plaintext,
        ciphersuite,
        backend.crypto(),
    )
    .unwrap();
    let decrypted_payload = hpke::decrypt_with_label(
        &kp.private,
        "label",
        &[1, 2, 3],
        &ciphertext,
        ciphersuite,
        backend.crypto(),
    )
    .expect("Unexpected error while decrypting a valid ciphertext.");
    assert_eq!(decrypted_payload, plaintext);

    let mut broken_kem_output = ciphertext.kem_output.clone();
    broken_kem_output.pop();
    let mut broken_ciphertext = ciphertext.ciphertext.clone();
    broken_ciphertext.pop();
    let broken_ciphertext1 = HpkeCiphertext {
        kem_output: broken_kem_output,
        ciphertext: ciphertext.ciphertext.clone(),
    };
    let broken_ciphertext2 = HpkeCiphertext {
        kem_output: ciphertext.kem_output,
        ciphertext: broken_ciphertext,
    };
    assert_eq!(
        hpke::decrypt_with_label(
            &kp.private,
            "label",
            &[1, 2, 3],
            &broken_ciphertext1,
            ciphersuite,
            backend.crypto(),
        )
        .map_err(|_| CryptoError::HpkeDecryptionError)
        .expect_err("Erroneously correct ciphertext decryption of broken ciphertext."),
        CryptoError::HpkeDecryptionError
    );
    assert_eq!(
        hpke::decrypt_with_label(
            &kp.private,
            "label",
            &[1, 2, 3],
            &broken_ciphertext2,
            ciphersuite,
            backend.crypto(),
        )
        .map_err(|_| CryptoError::HpkeDecryptionError)
        .expect_err("Erroneously correct ciphertext decryption of broken ciphertext."),
        CryptoError::HpkeDecryptionError
    );
}
