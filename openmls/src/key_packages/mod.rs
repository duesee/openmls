//! # Key Packages
//!
//! Key packages are pre-published public keys that provide some information
//! about a user in order to facilitate the asynchronous addition of clients to
//! a group.
//!
//! A key package object specifies:
//!
//! - A **protocol version** and ciphersuite that the client supports
//! - A **public key** that others can use for key agreement
//! - A **credential** authenticating the client's application-layer identity
//! - A list of **extensions** for the key package (see
//!   [Extensions](`mod@crate::extensions`) for details)
//!
//! Key packages are intended to be used only once and SHOULD NOT be reused
//! except in case of last resort, i.e. if there's no other key package
//! available. Clients MAY generate and publish multiple KeyPackages to support
//! multiple ciphersuites.
//!
//! The value for HPKE init key MUST be a public key for the asymmetric
//! encryption scheme defined by ciphersuite, and it MUST be unique among the
//! set of key packages created by this client. The whole structure is signed
//! using the client's signature key. A key package object with an invalid
//! signature field is considered malformed.
//!
//! ## Creating key package bundles
//!
//! Key package bundles are key packages including their private key. A key
//! package bundle can be created as follows:
//!
//! ```
//! use openmls::prelude::*;
//! use openmls_rust_crypto::OpenMlsRustCrypto;
//! use openmls_basic_credential::SignatureKeyPair;
//!
//! let ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
//! let backend = OpenMlsRustCrypto::default();
//!
//! let credential = Credential::new("identity".into(), CredentialType::Basic).unwrap();
//! let signer =
//!     SignatureKeyPair::new(ciphersuite.signature_algorithm())
//!         .expect("Error generating a signature key pair.");
//! let credential_with_key = CredentialWithKey {
//!     credential,
//!     signature_key: signer.public().into(),
//! };
//! let key_package = KeyPackage::builder()
//!     .build(
//!         CryptoConfig {
//!             ciphersuite,
//!             version: ProtocolVersion::default(),
//!         },
//!         &backend,
//!         &signer,
//!         credential_with_key,
//!     )
//!     .unwrap();
//! ```
//!
//! See [`KeyPackage`] for more details and other ways to create key packages.
//!
//! ## Loading key packages
//!
//! When getting key packages from another user the serialized bytes are parsed
//! as follows;
//!
//! ```
//! use openmls::prelude::*;
//! use openmls::test_utils::hex_to_bytes;
//! use openmls_rust_crypto::OpenMlsRustCrypto;
//!
//! let backend = OpenMlsRustCrypto::default();
//! let ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
//!
//! let key_package_bytes = hex_to_bytes(
//!         "00010003205D1DEB647BB5BDBB417022D1161076275ED861BD427B10DD07AC0233ED\
//!         D1D54F20428F4A2DAE538BD64154892A1A2C85D0F6888CE9977E09AC53DCB3471DECD\
//!         F6120DFC735AD28400BD79EFC8043B326D90A75A7B66F2A36EEDE79E3620A885E1825\
//!         0001055361736861020001060001000200030200010C0001000200030004000500070\
//!         200010100000000643170770000000064A03C87004040033B4FCBCC85D1AFE86241E8\
//!         5CECB2B496C9D7F8CE98BEDDBDFFC02EB2CA54BC881A84CA800345005BB2622EDC377\
//!         B54F223E160BFC872D5FEA2287E16A44703004040D33B9F1DA0F86C78855036B5FADD\
//!         35983E14188A0798B455211654E594E3955026CD1AD275D284CA8FBEA5356A0BA4C6F\
//!         799707C0046C05FBF52FF1FC1E9640A");
//!
//! let key_package_in = KeyPackageIn::tls_deserialize(&mut key_package_bytes.as_slice())
//!     .expect("Could not deserialize KeyPackage");
//!
//! let key_package = key_package_in
//!     .validate(backend.crypto())
//!     .expect("Invalid KeyPackage");
//! ```
//!
//! See [`KeyPackage`] for more details on how to use key packages.

use crate::{
    ciphersuite::{
        hash_ref::{make_key_package_ref, KeyPackageRef},
        signable::*,
        *,
    },
    credentials::*,
    error::LibraryError,
    extensions::ExtensionType,
    extensions::Extensions,
    group::config::CryptoConfig,
    treesync::{
        node::{
            encryption_keys::EncryptionKeyPair,
            leaf_node::{Capabilities, LeafNodeSource, Lifetime, NewLeafNodeParams, TreeInfoTbs},
        },
        LeafNode,
    },
    versions::ProtocolVersion,
};
use openmls_traits::{
    crypto::OpenMlsCrypto,
    key_store::{MlsEntity, MlsEntityId, OpenMlsKeyStore},
    signatures::Signer,
    types::Ciphersuite,
    OpenMlsCryptoProvider,
};
use serde::{Deserialize, Serialize};
use tls_codec::{Serialize as TlsSerializeTrait, TlsSerialize, TlsSize};

#[cfg(test)]
use crate::treesync::node::encryption_keys::EncryptionKey;

// Private
use errors::*;

// Public
pub mod errors;
pub mod key_package_in;

// Tests
#[cfg(test)]
pub(crate) mod test_key_packages;

// Public types
pub use key_package_in::KeyPackageIn;

/// The unsigned payload of a key package.
/// Any modification must happen on this unsigned struct. Use `sign` to get a
/// signed key package.
///
/// ```text
/// struct {
///     ProtocolVersion version;
///     CipherSuite cipher_suite;
///     HPKEPublicKey init_key;
///     LeafNode leaf_node;
///     Extension extensions<V>;
/// } KeyPackageTBS;
/// ```
#[derive(Debug, Clone, PartialEq, TlsSize, TlsSerialize, Serialize, Deserialize)]
struct KeyPackageTbs {
    protocol_version: ProtocolVersion,
    ciphersuite: Ciphersuite,
    init_key: HpkePublicKey,
    leaf_node: LeafNode,
    extensions: Extensions,
}

impl Signable for KeyPackageTbs {
    type SignedOutput = KeyPackage;

    fn unsigned_payload(&self) -> Result<Vec<u8>, tls_codec::Error> {
        self.tls_serialize_detached()
    }

    fn label(&self) -> &str {
        SIGNATURE_KEY_PACKAGE_LABEL
    }
}

impl From<KeyPackage> for KeyPackageTbs {
    fn from(kp: KeyPackage) -> Self {
        kp.payload
    }
}

/// The key package struct.
#[derive(Debug, Clone, Serialize, Deserialize, TlsSize, TlsSerialize)]
pub struct KeyPackage {
    payload: KeyPackageTbs,
    signature: Signature,
}

impl PartialEq for KeyPackage {
    fn eq(&self, other: &Self) -> bool {
        // We ignore the signature in the comparison. The same key package
        // may have different, valid signatures.
        self.payload == other.payload
    }
}

impl SignedStruct<KeyPackageTbs> for KeyPackage {
    fn from_payload(payload: KeyPackageTbs, signature: Signature) -> Self {
        Self { payload, signature }
    }
}

const SIGNATURE_KEY_PACKAGE_LABEL: &str = "KeyPackageTBS";

impl MlsEntity for KeyPackage {
    const ID: MlsEntityId = MlsEntityId::KeyPackage;
}

/// Helper struct containing the results of building a new [`KeyPackage`].
pub(crate) struct KeyPackageCreationResult {
    pub key_package: KeyPackage,
    pub encryption_keypair: EncryptionKeyPair,
    pub init_private_key: Vec<u8>,
}

// Public `KeyPackage` functions.
impl KeyPackage {
    /// Create a key package builder.
    ///
    /// This is provided for convenience. You can also use [`KeyPackageBuilder::new`].
    pub fn builder() -> KeyPackageBuilder {
        KeyPackageBuilder::new()
    }

    /// Create a new key package for the given `ciphersuite` and `identity`.
    pub(crate) fn create<KeyStore: OpenMlsKeyStore>(
        config: CryptoConfig,
        backend: &impl OpenMlsCryptoProvider<KeyStoreProvider = KeyStore>,
        signer: &impl Signer,
        credential_with_key: CredentialWithKey,
        extensions: Extensions,
        leaf_node_capabilities: Capabilities,
        leaf_node_extensions: Extensions,
    ) -> Result<KeyPackageCreationResult, KeyPackageNewError<KeyStore::Error>> {
        if config.ciphersuite.signature_algorithm() != signer.signature_scheme() {
            return Err(KeyPackageNewError::CiphersuiteSignatureSchemeMismatch);
        }

        // Create a new HPKE key pair
        let ikm = Secret::random(config.ciphersuite, backend, config.version)
            .map_err(LibraryError::unexpected_crypto_error)?;
        let init_key = backend
            .crypto()
            .derive_hpke_keypair(config.ciphersuite.hpke_config(), ikm.as_slice());
        let (key_package, encryption_keypair) = Self::new_from_keys(
            config,
            backend,
            signer,
            credential_with_key,
            extensions,
            leaf_node_capabilities,
            leaf_node_extensions,
            init_key.public,
        )?;

        Ok(KeyPackageCreationResult {
            key_package,
            encryption_keypair,
            init_private_key: init_key.private,
        })
    }

    /// Create a new key package for the given `ciphersuite` and `identity`.
    ///
    /// The HPKE init key must have been generated before and the private part
    /// has to be stored in the key store.
    ///
    /// This function returns the new [`KeyPackage`] as well as the
    /// encryption key ([`HpkeKeyPair`]) of the leaf node.
    ///
    /// The caller is responsible for storing the new values.
    #[allow(clippy::too_many_arguments)]
    fn new_from_keys<KeyStore: OpenMlsKeyStore>(
        config: CryptoConfig,
        backend: &impl OpenMlsCryptoProvider<KeyStoreProvider = KeyStore>,
        signer: &impl Signer,
        credential_with_key: CredentialWithKey,
        extensions: Extensions,
        capabilities: Capabilities,
        leaf_node_extensions: Extensions,
        init_key: Vec<u8>,
    ) -> Result<(Self, EncryptionKeyPair), KeyPackageNewError<KeyStore::Error>> {
        // We don't need the private key here. It's stored in the key store for
        // use later when creating a group with this key package.

        let new_leaf_node_params = NewLeafNodeParams {
            config,
            leaf_node_source: LeafNodeSource::KeyPackage(Lifetime::default()),
            credential_with_key,
            capabilities,
            extensions: leaf_node_extensions,
            tree_info_tbs: TreeInfoTbs::KeyPackage,
        };

        let (leaf_node, encryption_key_pair) =
            LeafNode::new(backend, signer, new_leaf_node_params)?;

        let key_package_tbs = KeyPackageTbs {
            protocol_version: config.version,
            ciphersuite: config.ciphersuite,
            init_key: init_key.into(),
            leaf_node,
            extensions,
        };

        let key_package = key_package_tbs.sign(signer)?;

        Ok((key_package, encryption_key_pair))
    }

    /// Delete this key package and its private key from the key store.
    pub fn delete<KeyStore: OpenMlsKeyStore>(
        &self,
        backend: &impl OpenMlsCryptoProvider<KeyStoreProvider = KeyStore>,
    ) -> Result<(), KeyStore::Error> {
        backend
            .key_store()
            .delete::<Self>(self.hash_ref(backend.crypto()).unwrap().as_slice())?;
        backend
            .key_store()
            .delete::<HpkePrivateKey>(self.hpke_init_key().as_slice())
    }

    /// Get a reference to the extensions of this key package.
    pub fn extensions(&self) -> &Extensions {
        &self.payload.extensions
    }

    /// Check whether the this key package supports all the required extensions
    /// in the provided list.
    pub fn check_extension_support(
        &self,
        required_extensions: &[ExtensionType],
    ) -> Result<(), KeyPackageExtensionSupportError> {
        for required_extension in required_extensions.iter() {
            if !self.extensions().contains(*required_extension) {
                return Err(KeyPackageExtensionSupportError::UnsupportedExtension);
            }
        }

        Ok(())
    }

    /// Compute the [`KeyPackageRef`] of this [`KeyPackage`].
    /// The [`KeyPackageRef`] is used to identify a new member that should get
    /// added to a group.
    pub fn hash_ref(&self, backend: &impl OpenMlsCrypto) -> Result<KeyPackageRef, LibraryError> {
        make_key_package_ref(
            &self
                .tls_serialize_detached()
                .map_err(LibraryError::missing_bound_check)?,
            self.payload.ciphersuite,
            backend,
        )
        .map_err(LibraryError::unexpected_crypto_error)
    }

    /// Get the [`Ciphersuite`].
    pub fn ciphersuite(&self) -> Ciphersuite {
        self.payload.ciphersuite
    }

    /// Get the [`LeafNode`] reference.
    pub fn leaf_node(&self) -> &LeafNode {
        &self.payload.leaf_node
    }

    /// Get the public HPKE init key of this key package.
    pub fn hpke_init_key(&self) -> &HpkePublicKey {
        &self.payload.init_key
    }
}

/// Crate visible `KeyPackage` functions.
impl KeyPackage {
    /// Get the `ProtocolVersion`.
    pub(crate) fn protocol_version(&self) -> ProtocolVersion {
        self.payload.protocol_version
    }
}

/// Helpers for testing.
#[cfg(any(feature = "test-utils", test))]
#[allow(clippy::too_many_arguments)]
impl KeyPackage {
    /// Generate a new key package with a given init key
    pub fn new_from_init_key<KeyStore: OpenMlsKeyStore>(
        config: CryptoConfig,
        backend: &impl OpenMlsCryptoProvider<KeyStoreProvider = KeyStore>,
        signer: &impl Signer,
        credential_with_key: CredentialWithKey,
        extensions: Extensions,
        leaf_node_capabilities: Capabilities,
        leaf_node_extensions: Extensions,
        init_key: Vec<u8>,
    ) -> Result<Self, KeyPackageNewError<KeyStore::Error>> {
        let (key_package, encryption_key_pair) = Self::new_from_keys(
            config,
            backend,
            signer,
            credential_with_key,
            extensions,
            leaf_node_capabilities,
            leaf_node_extensions,
            init_key,
        )?;

        // Store the key package in the key store with the hash reference as id
        // for retrieval when parsing welcome messages.
        backend
            .key_store()
            .store(
                key_package.hash_ref(backend.crypto())?.as_slice(),
                &key_package,
            )
            .map_err(KeyPackageNewError::KeyStoreError)?;

        // Store the encryption key pair in the key store.
        encryption_key_pair
            .write_to_key_store(backend)
            .map_err(KeyPackageNewError::KeyStoreError)?;

        Ok(key_package)
    }

    /// Create new key package with a leaf node encryption key set to the
    /// provided `encryption_key`.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_from_encryption_key<KeyStore: OpenMlsKeyStore>(
        config: CryptoConfig,
        backend: &impl OpenMlsCryptoProvider<KeyStoreProvider = KeyStore>,
        signer: &impl Signer,
        credential_with_key: CredentialWithKey,
        extensions: Extensions,
        leaf_node_capabilities: Capabilities,
        leaf_node_extensions: Extensions,
        encryption_key: EncryptionKey,
    ) -> Result<Self, KeyPackageNewError<KeyStore::Error>> {
        // Create a new HPKE init key pair
        let ikm = Secret::random(config.ciphersuite, backend, config.version).unwrap();
        let init_key = backend
            .crypto()
            .derive_hpke_keypair(config.ciphersuite.hpke_config(), ikm.as_slice());

        // Store the private part of the init_key into the key store.
        // The key is the public key.
        backend
            .key_store()
            .store::<HpkePrivateKey>(&init_key.public, &init_key.private.into())
            .map_err(KeyPackageNewError::KeyStoreError)?;

        // We don't need the private key here. It's stored in the key store for
        // use later when creating a group with this key package.
        let leaf_node = LeafNode::create_new_with_key(
            encryption_key,
            credential_with_key,
            LeafNodeSource::KeyPackage(Lifetime::default()),
            leaf_node_capabilities,
            leaf_node_extensions,
            TreeInfoTbs::KeyPackage,
            signer,
        )
        .unwrap();

        let key_package = KeyPackageTbs {
            protocol_version: config.version,
            ciphersuite: config.ciphersuite,
            init_key: init_key.public.into(),
            leaf_node,
            extensions,
        };

        let key_package = key_package.sign(signer)?;

        // Store the key package in the key store with the hash reference as id
        // for retrieval when parsing welcome messages.
        backend
            .key_store()
            .store(
                key_package.hash_ref(backend.crypto())?.as_slice(),
                &key_package,
            )
            .map_err(KeyPackageNewError::KeyStoreError)?;

        Ok(key_package)
    }

    pub fn into_with_init_key(
        self,
        config: CryptoConfig,
        signer: &impl Signer,
        init_key: Vec<u8>,
    ) -> Result<Self, SignatureError> {
        let key_package_tbs = KeyPackageTbs {
            protocol_version: config.version,
            ciphersuite: config.ciphersuite,
            init_key: init_key.into(),
            leaf_node: self.leaf_node().clone(),
            extensions: self.extensions().clone(),
        };

        key_package_tbs.sign(signer)
    }

    /// Resign this key package with another credential.
    pub fn resign(mut self, signer: &impl Signer, credential_with_key: CredentialWithKey) -> Self {
        self.payload
            .leaf_node
            .set_credential(credential_with_key.credential.clone());
        self.payload
            .leaf_node
            .set_signature_key(credential_with_key.signature_key.clone());

        self.payload
            .leaf_node
            .resign(signer, credential_with_key, TreeInfoTbs::KeyPackage);

        self.payload.sign(signer).unwrap()
    }

    /// Replace the public key in the KeyPackage.
    pub fn set_public_key(&mut self, public_key: HpkePublicKey) {
        self.payload.init_key = public_key
    }

    /// Replace the version in the KeyPackage.
    pub fn set_version(&mut self, version: ProtocolVersion) {
        self.payload.protocol_version = version
    }

    /// Replace the ciphersuite in the KeyPackage.
    pub fn set_ciphersuite(&mut self, ciphersuite: Ciphersuite) {
        self.payload.ciphersuite = ciphersuite
    }

    /// Set the [`LeafNode`].
    pub fn set_leaf_node(&mut self, leaf_node: LeafNode) {
        self.payload.leaf_node = leaf_node;
    }
}

/// Builder that helps creating (and configuring) a [`KeyPackage`].
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct KeyPackageBuilder {
    key_package_extensions: Option<Extensions>,
    leaf_node_capabilities: Option<Capabilities>,
    leaf_node_extensions: Option<Extensions>,
}

impl KeyPackageBuilder {
    /// Create a key package builder.
    pub fn new() -> Self {
        Self {
            key_package_extensions: None,
            leaf_node_capabilities: None,
            leaf_node_extensions: None,
        }
    }

    /// Set the key package extensions.
    pub fn key_package_extensions(mut self, extensions: Extensions) -> Self {
        self.key_package_extensions = Some(extensions);
        self
    }

    /// Set the leaf node capabilities.
    pub fn leaf_node_capabilities(mut self, capabilities: Capabilities) -> Self {
        self.leaf_node_capabilities = Some(capabilities);
        self
    }

    /// Set the leaf node extensions.
    pub fn leaf_node_extensions(mut self, extensions: Extensions) -> Self {
        self.leaf_node_extensions = Some(extensions);
        self
    }

    pub(crate) fn build_without_key_storage<KeyStore: OpenMlsKeyStore>(
        self,
        config: CryptoConfig,
        backend: &impl OpenMlsCryptoProvider<KeyStoreProvider = KeyStore>,
        signer: &impl Signer,
        credential_with_key: CredentialWithKey,
    ) -> Result<KeyPackageCreationResult, KeyPackageNewError<KeyStore::Error>> {
        KeyPackage::create(
            config,
            backend,
            signer,
            credential_with_key,
            self.key_package_extensions.unwrap_or_default(),
            self.leaf_node_capabilities.unwrap_or_default(),
            self.leaf_node_extensions.unwrap_or_default(),
        )
    }

    /// Finalize and build the key package.
    pub fn build<KeyStore: OpenMlsKeyStore>(
        self,
        config: CryptoConfig,
        backend: &impl OpenMlsCryptoProvider<KeyStoreProvider = KeyStore>,
        signer: &impl Signer,
        credential_with_key: CredentialWithKey,
    ) -> Result<KeyPackage, KeyPackageNewError<KeyStore::Error>> {
        let KeyPackageCreationResult {
            key_package,
            encryption_keypair,
            init_private_key,
        } = KeyPackage::create(
            config,
            backend,
            signer,
            credential_with_key,
            self.key_package_extensions.unwrap_or_default(),
            self.leaf_node_capabilities.unwrap_or_default(),
            self.leaf_node_extensions.unwrap_or_default(),
        )?;

        // Store the key package in the key store with the hash reference as id
        // for retrieval when parsing welcome messages.
        backend
            .key_store()
            .store(
                key_package.hash_ref(backend.crypto())?.as_slice(),
                &key_package,
            )
            .map_err(KeyPackageNewError::KeyStoreError)?;

        // Store the encryption key pair in the key store.
        encryption_keypair
            .write_to_key_store(backend)
            .map_err(KeyPackageNewError::KeyStoreError)?;

        // Store the private part of the init_key into the key store.
        // The key is the public key.
        backend
            .key_store()
            .store::<HpkePrivateKey>(
                key_package.hpke_init_key().as_slice(),
                &init_private_key.into(),
            )
            .map_err(KeyPackageNewError::KeyStoreError)?;

        Ok(key_package)
    }
}

/// A [`KeyPackageBundle`] contains a [`KeyPackage`] and the corresponding private
/// key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) struct KeyPackageBundle {
    pub(crate) key_package: KeyPackage,
    pub(crate) private_key: HpkePrivateKey,
}

// Public `KeyPackageBundle` functions.
impl KeyPackageBundle {
    /// Get a reference to the public part of this bundle, i.e. the [`KeyPackage`].
    pub(crate) fn key_package(&self) -> &KeyPackage {
        &self.key_package
    }

    /// Get a reference to the private key.
    pub fn private_key(&self) -> &HpkePrivateKey {
        &self.private_key
    }
}

#[cfg(test)]
impl KeyPackageBundle {
    pub(crate) fn new(
        backend: &impl OpenMlsCryptoProvider,
        signer: &impl Signer,
        ciphersuite: Ciphersuite,
        credential_with_key: CredentialWithKey,
    ) -> Self {
        let key_package = KeyPackage::builder()
            .build(
                CryptoConfig {
                    ciphersuite,
                    version: ProtocolVersion::default(),
                },
                backend,
                signer,
                credential_with_key,
            )
            .unwrap();
        let private_key = backend
            .key_store()
            .read::<HpkePrivateKey>(key_package.hpke_init_key().as_slice())
            .unwrap();
        Self {
            key_package,
            private_key,
        }
    }
}
