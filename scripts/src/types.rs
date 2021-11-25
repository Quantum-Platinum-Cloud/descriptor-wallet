// Descriptor wallet library extending bitcoin & miniscript functionality
// by LNP/BP Association (https://lnp-bp.org)
// Written in 2020-2021 by
//     Dr. Maxim Orlovsky <orlovsky@pandoracore.com>
//
// To the extent possible under law, the author(s) have dedicated all
// copyright and related and neighboring rights to this software to
// the public domain worldwide. This software is distributed without
// any warranty.
//
// You should have received a copy of the Apache-2.0 License
// along with this software.
// If not, see <https://opensource.org/licenses/Apache-2.0>.

use std::convert::TryFrom;
use std::fmt::{self, Display, Formatter};
use std::io::{self, Read, Write};

use amplify::hex::ToHex;
use amplify::Wrapper;
use bitcoin::blockdata::script::*;
use bitcoin::blockdata::witness::Witness;
use bitcoin::hashes::Hash;
use bitcoin::util::taproot::{ControlBlock, LeafVersion, TaprootError};
use bitcoin::{
    secp256k1, Address, Network, PubkeyHash, SchnorrSig, SchnorrSigError, ScriptHash, WPubkeyHash,
    WScriptHash,
};
use miniscript::ToPublicKey;

use crate::Category;

/// Script whose knowledge is required for spending some specific transaction
/// output. This is the deepest nested version of Bitcoin script containing no
/// hashes of other scripts, including P2SH redeemScript hashes or
/// witnessProgram (hash or witness script), or public key hashes
#[derive(
    Wrapper, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug, Display, From
)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", transparent)
)]
#[display("{0}", alt = "{0:x}")]
#[wrapper(LowerHex, UpperHex)]
pub struct LockScript(Script);

impl strict_encoding::Strategy for LockScript {
    type Strategy = strict_encoding::strategies::Wrapped;
}

/// A representation of `scriptPubkey` data used during SegWit signing procedure
#[derive(
    Wrapper, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug, Display, From
)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", transparent)
)]
#[display("{0}", alt = "{0:x}")]
#[wrapper(LowerHex, UpperHex)]
pub struct ScriptCode(Script);

/// A content of `scriptPubkey` from a transaction output
#[derive(
    Wrapper, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug, Display, From
)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", transparent)
)]
#[display("{0}", alt = "{0:x}")]
#[wrapper(LowerHex, UpperHex)]
pub struct PubkeyScript(Script);

impl strict_encoding::Strategy for PubkeyScript {
    type Strategy = strict_encoding::strategies::Wrapped;
}

impl PubkeyScript {
    pub fn address(&self, network: Network) -> Option<Address> {
        Address::from_script(self.as_inner(), network)
    }

    pub fn script_code(&self) -> ScriptCode {
        if self.0.is_v0_p2wpkh() {
            let pubkey_hash =
                PubkeyHash::from_slice(&self.0[2..22]).expect("PubkeyHash hash length failure");
            ScriptCode::from_inner(Script::new_p2pkh(&pubkey_hash))
        } else {
            ScriptCode::from_inner(self.to_inner())
        }
    }
}

impl From<WPubkeyHash> for PubkeyScript {
    fn from(wpkh: WPubkeyHash) -> Self { Script::new_v0_p2wpkh(&wpkh).into() }
}

/// A content of `sigScript` from a transaction input
#[derive(
    Wrapper, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug, Display, From
)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", transparent)
)]
#[display("{0}", alt = "{_0:x}")]
#[wrapper(LowerHex, UpperHex)]
pub struct SigScript(Script);

impl strict_encoding::Strategy for SigScript {
    type Strategy = strict_encoding::strategies::Wrapped;
}

/// Errors for [`TaprootWitness`] construction from [`Witness`] and byte
/// representations
#[derive(Debug, Display, From)]
#[display(doc_comments)]
pub enum TaprootWitnessError {
    /// witness stack has zero elements
    EmptyWitnessStack,

    /// BIP-341 signature encoding error in witness data
    #[from]
    Bip341SigError(SchnorrSigError),

    #[display(inner)]
    #[from]
    TaprootError(TaprootError),

    /// script encoding error
    ScriptError(bitcoin::consensus::encode::Error),
}

impl std::error::Error for TaprootWitnessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TaprootWitnessError::EmptyWitnessStack => None,
            TaprootWitnessError::Bip341SigError(err) => Some(err),
            TaprootWitnessError::TaprootError(_) => None,
            TaprootWitnessError::ScriptError(err) => Some(err),
        }
    }
}

/// Parsed witness stack for Taproot inputs containing script spendings
#[derive(Clone, PartialEq, Eq, Debug)]
// TODO: Uncomment once SchnorrSig will implement serde
/* #[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate")
)] */
pub enum TaprootWitness {
    /// Public key path spending
    PubkeySpending {
        /// BIP-341 signature
        sig: SchnorrSig,
        /// Optional annex data (annex prefix is removed)
        annex: Option<Box<[u8]>>,
    },

    /// Script path spending
    ScriptSpending {
        /// Taproot control block
        control_block: ControlBlock,
        /// Optional annex data (annex prefix is removed)
        annex: Option<Box<[u8]>>,
        /// Leaf script for the spending
        script: LeafScript,
        /// The remaining part of the witness stack
        script_input: Vec<Box<[u8]>>,
    },
}

impl TryFrom<Witness> for TaprootWitness {
    type Error = TaprootWitnessError;

    fn try_from(witness: Witness) -> Result<Self, Self::Error> {
        if witness.is_empty() {
            return Err(TaprootWitnessError::EmptyWitnessStack);
        }

        let (len, annex) = if witness.len() > 1 {
            let len = witness.len() - 1;
            let annex = &witness[len];
            (len, Some(Box::from(annex)))
        } else {
            (witness.len(), None)
        };

        Ok(if len == 1 {
            TaprootWitness::PubkeySpending {
                sig: SchnorrSig::from_slice(&witness[0])?,
                annex,
            }
        } else {
            let control_block = ControlBlock::from_slice(&witness[len - 1])?;
            let s = bitcoin::consensus::deserialize(&witness[len - 2])
                .map_err(TaprootWitnessError::ScriptError)?;
            let script = LeafScript {
                version: control_block.leaf_version,
                script: LockScript::from_inner(s),
            };
            TaprootWitness::ScriptSpending {
                control_block,
                annex,
                script,
                script_input: witness.iter().take(len - 2).map(Box::from).collect(),
            }
        })
    }
}

impl From<TaprootWitness> for Witness {
    #[inline]
    fn from(tw: TaprootWitness) -> Self { Witness::from(&tw) }
}

impl From<&TaprootWitness> for Witness {
    fn from(tw: &TaprootWitness) -> Self {
        let mut witness = Witness::default();
        match tw {
            TaprootWitness::PubkeySpending { sig, annex } => {
                witness.push(&sig.to_vec());
                if let Some(annex) = annex {
                    witness.push(&annex);
                }
            }
            TaprootWitness::ScriptSpending {
                control_block,
                annex,
                script,
                script_input,
            } => {
                for item in script_input {
                    witness.push(&item);
                }
                witness.push(&bitcoin::consensus::serialize(&script.script.0));
                witness.push(&control_block.serialize());
                if let Some(annex) = annex {
                    witness.push(&annex);
                }
            }
        }
        witness
    }
}

impl strict_encoding::Strategy for TaprootWitness {
    type Strategy = strict_encoding::strategies::BitcoinConsensus;
}

impl bitcoin::consensus::Encodable for TaprootWitness {
    fn consensus_encode<W: Write>(&self, writer: W) -> Result<usize, io::Error> {
        Witness::from(self).consensus_encode(writer)
    }
}

impl bitcoin::consensus::Decodable for TaprootWitness {
    fn consensus_decode<D: Read>(d: D) -> Result<Self, bitcoin::consensus::encode::Error> {
        TaprootWitness::try_from(Witness::consensus_decode(d)?).map_err(|_| {
            bitcoin::consensus::encode::Error::ParseFailed("witness does not conform to taproot")
        })
    }
}

/// `redeemScript` as part of the `witness` or `sigScript` structure; it is
///  hashed for P2(W)SH output
#[derive(
    Wrapper, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug, Display, From
)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", transparent)
)]
#[display("{0}", alt = "{_0:x}")]
#[wrapper(LowerHex, UpperHex)]
pub struct RedeemScript(Script);

impl strict_encoding::Strategy for RedeemScript {
    type Strategy = strict_encoding::strategies::Wrapped;
}

impl RedeemScript {
    pub fn script_hash(&self) -> ScriptHash { self.as_inner().script_hash() }
    pub fn to_p2sh(&self) -> PubkeyScript { self.to_pubkey_script(Category::Hashed) }
}

impl ToPubkeyScript for RedeemScript {
    fn to_pubkey_script(&self, strategy: Category) -> PubkeyScript {
        LockScript::from(self.clone()).to_pubkey_script(strategy)
    }
}

/// A content of the script from `witness` structure; en equivalent of
/// `redeemScript` for witness-based transaction inputs. However, unlike
/// [`RedeemScript`], [`WitnessScript`] produce SHA256-based hashes of
/// [`WScriptHash`] type
#[derive(
    Wrapper, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug, Display, From
)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", transparent)
)]
#[display("{0}", alt = "{_0:x}")]
#[wrapper(LowerHex, UpperHex)]
pub struct WitnessScript(Script);

impl strict_encoding::Strategy for WitnessScript {
    type Strategy = strict_encoding::strategies::Wrapped;
}

impl WitnessScript {
    pub fn script_hash(&self) -> WScriptHash { self.as_inner().wscript_hash() }
    pub fn to_p2wsh(&self) -> PubkeyScript { self.to_pubkey_script(Category::SegWitV0) }
    pub fn to_p2sh_wsh(&self) -> PubkeyScript { self.to_pubkey_script(Category::SegWitV0Nested) }
}

impl ToPubkeyScript for WitnessScript {
    fn to_pubkey_script(&self, strategy: Category) -> PubkeyScript {
        LockScript::from(self.clone()).to_pubkey_script(strategy)
    }
}

impl From<LockScript> for WitnessScript {
    fn from(lock_script: LockScript) -> Self { WitnessScript(lock_script.to_inner()) }
}

impl From<LockScript> for RedeemScript {
    fn from(lock_script: LockScript) -> Self { RedeemScript(lock_script.to_inner()) }
}

impl From<WitnessScript> for LockScript {
    fn from(witness_script: WitnessScript) -> Self { LockScript(witness_script.to_inner()) }
}

impl From<RedeemScript> for LockScript {
    fn from(redeem_script: RedeemScript) -> Self { LockScript(redeem_script.to_inner()) }
}

/// Any valid branch of taproot script spending
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Display)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate")
)]
#[display("{version} {script}", alt = "{version:#} {script:x}")]
pub struct LeafScript {
    /// Leaf version of the script
    pub version: LeafVersion,

    /// Script data
    pub script: LockScript,
}

impl strict_encoding::StrictEncode for LeafScript {
    fn strict_encode<E: Write>(&self, mut e: E) -> Result<usize, strict_encoding::Error> {
        self.version.as_u8().strict_encode(&mut e)?;
        self.script.as_inner().to_bytes().strict_encode(&mut e)
    }
}

impl strict_encoding::StrictDecode for LeafScript {
    fn strict_decode<D: Read>(mut d: D) -> Result<Self, strict_encoding::Error> {
        let version = u8::strict_decode(&mut d)?;
        let version = LeafVersion::from_u8(version)
            .map_err(|_| bitcoin::consensus::encode::Error::ParseFailed("invalid leaf version"))?;
        let script = LockScript::from_inner(Script::from(Vec::<u8>::strict_decode(d)?));
        Ok(LeafScript { version, script })
    }
}

#[derive(
    Wrapper, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug, From
)]
pub struct WitnessProgram(Box<[u8]>);

impl strict_encoding::Strategy for WitnessProgram {
    type Strategy = strict_encoding::strategies::Wrapped;
}

impl Display for WitnessProgram {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result { writeln!(f, "{}", self.0.to_hex()) }
}

impl From<WPubkeyHash> for WitnessProgram {
    fn from(wpkh: WPubkeyHash) -> Self { WitnessProgram(Box::from(&wpkh[..])) }
}

impl From<WScriptHash> for WitnessProgram {
    fn from(wsh: WScriptHash) -> Self { WitnessProgram(Box::from(&wsh[..])) }
}

/// Scripting data for both transaction output and spending transaction input
/// parts that can be generated from some complete bitcoin Script
/// ([`LockScript`]) or public key using particular [`Category`]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Default)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate")
)]
pub struct ScriptSet {
    pub pubkey_script: PubkeyScript,
    pub sig_script: SigScript,
    pub witness: Option<Witness>,
}

impl Display for ScriptSet {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} {}",
            self.sig_script,
            self.witness
                .as_ref()
                .map(Witness::to_string)
                .unwrap_or_default(),
            self.pubkey_script
        )
    }
}

impl ScriptSet {
    /// Detects whether the structure contains witness data
    #[inline]
    pub fn has_witness(&self) -> bool { self.witness != None }

    /// Detects whether the structure is either P2SH-P2WPKH or P2SH-P2WSH
    pub fn is_witness_sh(&self) -> bool {
        return !self.sig_script.as_inner().is_empty() && self.has_witness();
    }

    /// Tries to convert witness-based script structure into pre-SegWit – and
    /// vice verse. Returns `true` if the conversion is possible and was
    /// successful, `false` if the conversion is impossible; in the later case
    /// the `self` is not changed. The conversion is impossible in the following
    /// cases:
    /// * for P2SH-P2WPKH or P2SH-P2WPSH variants (can be detected with
    ///   [ScriptSet::is_witness_sh] function)
    /// * for scripts that are internally inconsistent
    pub fn transmutate(&mut self, use_witness: bool) -> bool {
        // We can't transmutate P2SH-contained P2WSH/P2WPKH
        if self.is_witness_sh() {
            return false;
        }
        if self.has_witness() != use_witness {
            if use_witness {
                self.witness = Some(
                    self.sig_script
                        .as_inner()
                        .instructions_minimal()
                        .filter_map(|instr| {
                            if let Ok(Instruction::PushBytes(bytes)) = instr {
                                Some(bytes.to_vec())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<Vec<u8>>>()
                        .into(),
                );
                self.sig_script = SigScript::default();
                true
            } else if let Some(ref witness_script) = self.witness {
                self.sig_script = witness_script
                    .iter()
                    .fold(Builder::new(), |builder, bytes| builder.push_slice(bytes))
                    .into_script()
                    .into();
                self.witness = None;
                true
            } else {
                false
            }
        } else {
            false
        }
    }
}

/// Conversion to [`LockScript`], which later may be used for creating different
/// end-point scripts, like [`PubkeyScript`], [`SigScript`], [`Witness`]
/// etc.
pub trait ToLockScript {
    fn to_lock_script(&self, strategy: Category) -> LockScript;
}

/// Conversion for data types (public keys, different types of script) into
/// a `pubkeyScript` (using [`PubkeyScript`] type) using particular conversion
/// [`Category`]
pub trait ToPubkeyScript {
    fn to_pubkey_script(&self, strategy: Category) -> PubkeyScript;
}

/// Script set generation from public keys or a given [`LockScript`] (with
/// [`TapScript`] support planned for the future).
pub trait ToScripts
where
    Self: ToPubkeyScript,
{
    fn to_scripts(&self, strategy: Category) -> ScriptSet {
        ScriptSet {
            pubkey_script: self.to_pubkey_script(strategy),
            sig_script: self.to_sig_script(strategy),
            witness: self.to_witness(strategy),
        }
    }
    fn to_sig_script(&self, strategy: Category) -> SigScript;
    fn to_witness(&self, strategy: Category) -> Option<Witness>;
}

impl ToPubkeyScript for LockScript {
    fn to_pubkey_script(&self, strategy: Category) -> PubkeyScript {
        match strategy {
            Category::Bare => self.to_inner().into(),
            Category::Hashed => Script::new_p2sh(&self.script_hash()).into(),
            Category::SegWitV0 => Script::new_v0_p2wsh(&self.wscript_hash()).into(),
            Category::SegWitV0Nested => {
                // Here we support only V0 version, since V1 version can't
                // be generated from `LockScript` and will require
                // `TapScript` source
                let redeem_script =
                    LockScript::from(self.to_pubkey_script(Category::SegWitV0).to_inner());
                Script::new_p2sh(&redeem_script.script_hash()).into()
            }
            Category::Taproot => unimplemented!(),
        }
    }
}

impl ToScripts for LockScript {
    fn to_sig_script(&self, strategy: Category) -> SigScript {
        match strategy {
            // sigScript must contain just a plain signatures, which will be
            // added later
            Category::Bare => SigScript::default(),
            Category::Hashed => Builder::new()
                .push_slice(WitnessScript::from(self.clone()).as_bytes())
                .into_script()
                .into(),
            Category::SegWitV0Nested => {
                // Here we support only V0 version, since V1 version can't
                // be generated from `LockScript` and will require
                // `TapScript` source
                let redeem_script =
                    LockScript::from(self.to_pubkey_script(Category::SegWitV0).to_inner());
                Builder::new()
                    .push_slice(redeem_script.as_bytes())
                    .into_script()
                    .into()
            }
            // For any segwit version the sigScript must be empty (with the
            // exception to the case of P2SH-embedded outputs, which is already
            // covered above
            _ => SigScript::default(),
        }
    }

    fn to_witness(&self, strategy: Category) -> Option<Witness> {
        match strategy {
            Category::Bare | Category::Hashed => None,
            Category::SegWitV0 | Category::SegWitV0Nested => {
                let witness_script = WitnessScript::from(self.clone());
                Some(Witness::from(vec![witness_script.to_bytes()]))
            }
            Category::Taproot => unimplemented!(),
        }
    }
}

impl ToLockScript for bitcoin::PublicKey {
    fn to_lock_script(&self, strategy: Category) -> LockScript {
        match strategy {
            Category::Bare => Script::new_p2pk(self).into(),
            Category::Hashed => Script::new_p2pkh(&self.pubkey_hash()).into(),
            // TODO #16: Detect uncompressed public key and return error
            Category::SegWitV0 => Script::new_v0_p2wpkh(
                &self
                    .wpubkey_hash()
                    .expect("Uncompressed public key used in witness script"),
            )
            .into(),
            Category::SegWitV0Nested => {
                let redeem_script = self.to_pubkey_script(Category::SegWitV0);
                Script::new_p2sh(&redeem_script.script_hash()).into()
            }
            Category::Taproot => unimplemented!(),
        }
    }
}

impl ToPubkeyScript for bitcoin::PublicKey {
    fn to_pubkey_script(&self, strategy: Category) -> PubkeyScript {
        self.to_lock_script(strategy).into_inner().into()
    }
}

impl ToScripts for bitcoin::PublicKey {
    fn to_sig_script(&self, strategy: Category) -> SigScript {
        match strategy {
            // sigScript must contain just a plain signatures, which will be
            // added later
            Category::Bare => SigScript::default(),
            Category::Hashed => Builder::new()
                .push_slice(&self.to_bytes())
                .into_script()
                .into(),
            Category::SegWitV0Nested => {
                let redeem_script =
                    LockScript::from(self.to_pubkey_script(Category::SegWitV0).into_inner());
                Builder::new()
                    .push_slice(redeem_script.as_bytes())
                    .into_script()
                    .into()
            }
            // For any segwit version the sigScript must be empty (with the
            // exception to the case of P2SH-embedded outputs, which is already
            // covered above
            _ => SigScript::default(),
        }
    }

    fn to_witness(&self, strategy: Category) -> Option<Witness> {
        match strategy {
            Category::Bare | Category::Hashed => None,
            Category::SegWitV0 | Category::SegWitV0Nested => {
                Some(Witness::from(vec![self.to_bytes()]))
            }
            Category::Taproot => unimplemented!(),
        }
    }
}

impl ToLockScript for secp256k1::PublicKey {
    #[inline]
    fn to_lock_script(&self, strategy: Category) -> LockScript {
        bitcoin::PublicKey {
            compressed: true,
            key: *self,
        }
        .to_lock_script(strategy)
    }
}

impl ToPubkeyScript for secp256k1::PublicKey {
    fn to_pubkey_script(&self, strategy: Category) -> PubkeyScript {
        self.to_lock_script(strategy).into_inner().into()
    }
}

impl ToScripts for secp256k1::PublicKey {
    #[inline]
    fn to_sig_script(&self, strategy: Category) -> SigScript {
        bitcoin::PublicKey {
            compressed: true,
            key: *self,
        }
        .to_sig_script(strategy)
    }

    #[inline]
    fn to_witness(&self, strategy: Category) -> Option<Witness> {
        bitcoin::PublicKey {
            compressed: true,
            key: *self,
        }
        .to_witness(strategy)
    }
}

pub trait ToP2pkh {
    fn to_p2pkh(&self) -> PubkeyScript;
    fn to_p2wpkh(&self) -> PubkeyScript;
    fn to_p2sh_wpkh(&self) -> PubkeyScript;
}

impl<T> ToP2pkh for T
where
    T: ToPublicKey,
{
    fn to_p2pkh(&self) -> PubkeyScript { self.to_public_key().to_pubkey_script(Category::Hashed) }

    fn to_p2wpkh(&self) -> PubkeyScript {
        self.to_public_key().to_pubkey_script(Category::SegWitV0)
    }

    fn to_p2sh_wpkh(&self) -> PubkeyScript {
        self.to_public_key()
            .to_pubkey_script(Category::SegWitV0Nested)
    }
}
