//! Sonobe is a library implementing various folding schemes for recursive SNARKs (Succinct Non-interactive ARguments of Knowledge)
//! and IVC (Incremental Verifiable Computation). It provides a modular, extensible framework for working with different folding
//! schemes including Nova, HyperNova, and other variants.
//!
//! # Overview
//!
//! The library is built around a core [`FoldingScheme`] trait that defines the interface for implementing different folding
//! schemes. Key features include:
//!
//! * Multiple folding scheme implementations including HyperNova and variants
//! * Support for both single-fold and multi-fold operations
//! * Flexible commitment schemes including Pedersen and KZG commitments
//! * CycleFold support for cross-curve folding operations
//! * Customizable arithmetic backends via the [`Arith`] trait
//! * Support for both R1CS and CCS (Customizable Constraint Systems)
//!
//! # Architecture
//!
//! The library is organized into several key modules:
//!
//! * `arith` - Core arithmetic abstractions including R1CS and CCS implementations
//! * `commitment` - Commitment scheme implementations like Pedersen and KZG
//! * `folding` - Main folding scheme implementations and associated traits
//! * `frontend` - User-facing circuit building interfaces
//! * `transcript` - Transcript handling for Fiat-Shamir transformations
//!
//! # Usage
//!
//! To use a folding scheme, you typically:
//!
//! 1. Define your computation circuit implementing [`FCircuit`]
//! 2. Choose and configure a folding scheme implementation
//! 3. Set up the necessary commitment scheme parameters
//! 4. Initialize the folding scheme with initial state
//! 5. Perform folding operations via `prove_step`
//!
//! # References
//!
//! The library implements protocols from several academic works:
//!
//! * [Nova: Recursive Zero-Knowledge Arguments from Folding Schemes](https://eprint.iacr.org/2021/370)
//! * [HyperNova: Recursive Arguments for Customizable Constraint Systems](https://eprint.iacr.org/2023/573)
//! * [CycleFold: Folding-scheme-based Recursive Arguments over Different Curves](https://eprint.iacr.org/2023/1192)

#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![warn(missing_docs, clippy::missing_docs_in_private_items)]

use ark_ec::{pairing::Pairing, CurveGroup};
use ark_ff::PrimeField;
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_std::rand::CryptoRng;
use ark_std::{fmt::Debug, rand::RngCore};
use thiserror::Error;

use crate::frontend::FCircuit;

pub mod arith;
pub mod commitment;
pub mod constants;
pub mod folding;
pub mod frontend;
pub mod transcript;
pub mod utils;

#[derive(Debug, Error)]
pub enum Error {
    // Wrappers on top of other errors
    #[error("ark_relations::r1cs::SynthesisError")]
    SynthesisError(#[from] ark_relations::r1cs::SynthesisError),
    #[error("ark_serialize::SerializationError")]
    SerializationError(#[from] ark_serialize::SerializationError),
    #[error("ark_poly_commit::Error")]
    PolyCommitError(#[from] ark_poly_commit::Error),
    #[error("crate::utils::espresso::virtual_polynomial::ArithErrors")]
    ArithError(#[from] utils::espresso::virtual_polynomial::ArithErrors),
    #[error(transparent)]
    ProtoGalaxy(folding::protogalaxy::ProtoGalaxyError),
    #[error("std::io::Error")]
    IOError(#[from] std::io::Error),

    // Relation errors
    #[error("Relation not satisfied")]
    NotSatisfied,
    #[error("SNARK setup failed: {0}")]
    SNARKSetupFail(String),
    #[error("SNARK verification failed")]
    SNARKVerificationFail,
    #[error("IVC verification failed")]
    IVCVerificationFail,
    #[error("zkIVC verification failed")]
    zkIVCVerificationFail,
    #[error("Committed instance is expected to be an incoming (fresh) instance")]
    NotIncomingCommittedInstance,
    #[error("R1CS instance is expected to not be relaxed")]
    R1CSUnrelaxedFail,
    #[error("Could not find the inner ConstraintSystem")]
    NoInnerConstraintSystem,
    #[error("Sum-check prove failed: {0}")]
    SumCheckProveError(String),
    #[error("Sum-check verify failed: {0}")]
    SumCheckVerifyError(String),

    // Comparators errors
    #[error("Not equal")]
    NotEqual,
    #[error("Vectors should have the same length ({0}: {1}, {2}: {3})")]
    NotSameLength(String, usize, String, usize),
    #[error("Vector's length ({0}) is not the expected ({1})")]
    NotExpectedLength(usize, usize),
    #[error("Vector ({0}) length ({1}) is not a power of two")]
    NotPowerOfTwo(String, usize),
    #[error("Can not be empty")]
    Empty,
    #[error("Value out of bounds")]
    OutOfBounds,
    #[error("Could not construct the Evaluation Domain")]
    NewDomainFail,
    #[error("The number of folded steps must be greater than 1")]
    NotEnoughSteps,
    #[error("Evaluation failed")]
    EvaluationFail,
    #[error("{0} can not be zero")]
    CantBeZero(String),

    // Commitment errors
    #[error("Pedersen parameters length is not sufficient (generators.len={0} < vector.len={1} unsatisfied)")]
    PedersenParamsLen(usize, usize),
    #[error("Blinding factor not 0 for Commitment without hiding")]
    BlindingNotZero,
    #[error("Blinding factors incorrect, blinding is set to {0} but blinding values are {1}")]
    IncorrectBlinding(bool, String),
    #[error("Commitment verification failed")]
    CommitmentVerificationFail,

    // Polynomial IOP errors, from https://github.com/EspressoSystems/hyperplonk/blob/main/subroutines/src/poly_iop/errors.rs
    #[error("Invalid Polynomial IOP Prover: {0}")]
    InvalidPolyIOPProver(String),
    #[error("Invalid Polynomial IOP Verifier: {0}")]
    InvalidPolyIOPVerifier(String),
    #[error("Invalid Polynomial IOP Proof: {0}")]
    InvalidPolyIOPProof(String),
    #[error("Invalid Polynomial IOP Parameters: {0}")]
    InvalidPolyIOPParameters(String),

    // Other
    #[error("{0}")]
    Other(String),
    #[error("Randomness for blinding not found")]
    MissingRandomness,
    #[error("Missing value: {0}")]
    MissingValue(String),
    #[error("Feature '{0}' not supported yet")]
    NotSupportedYet(String),
    #[error("Feature '{0}' is not supported and it will not be")]
    NotSupported(String),
    #[error("max i-th step reached (usize limit reached)")]
    MaxStep,
    #[error("Witness calculation error: {0}")]
    WitnessCalculationError(String),
    #[error("Failed to convert {0} into {1}: {2}")]
    ConversionError(String, String, String),
    #[error("Failed to serde: {0}")]
    JSONSerdeError(String),
    #[error("Multi instances folding not supported in this scheme")]
    NoMultiInstances,
    #[error("Missing 'other' instances, since this is a multi-instances folding scheme. Expected number of instances, mu:{0}, nu:{1}")]
    MissingOtherInstances(usize, usize),
}

/// FoldingScheme defines trait that is implemented by the diverse folding schemes. It is defined
/// over a cycle of curves (C1, C2), where:
/// - C1 is the main curve, which ScalarField we use as our F for all the field operations
/// - C2 is the auxiliary curve, which we use for the commitments, whose BaseField (for point
///   coordinates) are in the C1::ScalarField.
///
/// In other words, C1.Fq == C2.Fr, and C1.Fr == C2.Fq.
pub trait FoldingScheme<C1: CurveGroup, C2: CurveGroup, FC>: Clone + Debug
where
    C1: CurveGroup<BaseField = C2::ScalarField, ScalarField = C2::BaseField>,
    C2::BaseField: PrimeField,
    FC: FCircuit<C1::ScalarField>,
{
    type PreprocessorParam: Debug + Clone;
    type ProverParam: Debug + Clone + CanonicalSerialize;
    type VerifierParam: Debug + Clone + CanonicalSerialize;
    type RunningInstance: Debug; // contains the CommittedInstance + Witness
    type IncomingInstance: Debug; // contains the CommittedInstance + Witness
    type MultiCommittedInstanceWithWitness: Debug; // type used for the extra instances in the multi-instance folding setting
    type CFInstance: Debug; // CycleFold CommittedInstance & Witness
    type IVCProof: PartialEq + Eq + Clone + Debug + CanonicalSerialize + CanonicalDeserialize;

    /// deserialize Self::ProverParam and recover the not serialized data that is recomputed on the
    /// fly to save serialized bytes.
    /// Internally it generates the r1cs/ccs & cf_r1cs needed for the VerifierParams. In this way
    /// we avoid needing to serialize them, saving significant space in the VerifierParams
    /// serialized size.
    fn pp_deserialize_with_mode<R: std::io::prelude::Read>(
        reader: R,
        compress: ark_serialize::Compress,
        validate: ark_serialize::Validate,
        fc_params: FC::Params, // FCircuit params
    ) -> Result<Self::ProverParam, Error>;

    /// deserialize Self::VerifierParam and recover the not serialized data that is recomputed on
    /// the fly to save serialized bytes.
    /// Internally it generates the r1cs/ccs & cf_r1cs needed for the VerifierParams. In this way
    /// we avoid needing to serialize them, saving significant space in the VerifierParams
    /// serialized size.
    fn vp_deserialize_with_mode<R: std::io::prelude::Read>(
        reader: R,
        compress: ark_serialize::Compress,
        validate: ark_serialize::Validate,
        fc_params: FC::Params, // FCircuit params
    ) -> Result<Self::VerifierParam, Error>;

    fn preprocess(
        rng: impl RngCore,
        prep_param: &Self::PreprocessorParam,
    ) -> Result<(Self::ProverParam, Self::VerifierParam), Error>;

    fn init(
        params: &(Self::ProverParam, Self::VerifierParam),
        step_circuit: FC,
        z_0: Vec<C1::ScalarField>, // initial state
    ) -> Result<Self, Error>;

    fn prove_step(
        &mut self,
        rng: impl RngCore,
        external_inputs: Vec<C1::ScalarField>,
        other_instances: Option<Self::MultiCommittedInstanceWithWitness>,
    ) -> Result<(), Error>;

    /// returns the state at the current step
    fn state(&self) -> Vec<C1::ScalarField>;

    /// returns the last IVC state proof, which can be verified in the `verify` method
    fn ivc_proof(&self) -> Self::IVCProof;

    /// constructs the FoldingScheme instance from the given IVCProof, ProverParams, VerifierParams
    /// and PoseidonConfig.
    /// This method is useful for when the IVCProof is sent between different parties, so that they
    /// can continue iterating the IVC from the received IVCProof.
    fn from_ivc_proof(
        ivc_proof: Self::IVCProof,
        fcircuit_params: FC::Params,
        params: (Self::ProverParam, Self::VerifierParam),
    ) -> Result<Self, Error>;

    fn verify(vp: Self::VerifierParam, ivc_proof: Self::IVCProof) -> Result<(), Error>;
}

/// Trait with auxiliary methods for multi-folding schemes (ie. HyperNova, ProtoGalaxy, etc),
/// allowing to create new instances for the multifold.
pub trait MultiFolding<C1: CurveGroup, C2: CurveGroup, FC>: Clone + Debug
where
    C1: CurveGroup<BaseField = C2::ScalarField, ScalarField = C2::BaseField>,
    C2::BaseField: PrimeField,
    FC: FCircuit<C1::ScalarField>,
{
    type RunningInstance: Debug;
    type IncomingInstance: Debug;
    type MultiInstance: Debug;

    /// Creates a new RunningInstance for the given state, to be folded in the multi-folding step.
    fn new_running_instance(
        &self,
        rng: impl RngCore,
        state: Vec<C1::ScalarField>,
        external_inputs: Vec<C1::ScalarField>,
    ) -> Result<Self::RunningInstance, Error>;

    /// Creates a new IncomingInstance for the given state, to be folded in the multi-folding step.
    fn new_incoming_instance(
        &self,
        rng: impl RngCore,
        state: Vec<C1::ScalarField>,
        external_inputs: Vec<C1::ScalarField>,
    ) -> Result<Self::IncomingInstance, Error>;
}

pub trait Decider<
    C1: CurveGroup,
    C2: CurveGroup,
    FC: FCircuit<C1::ScalarField>,
    FS: FoldingScheme<C1, C2, FC>,
> where
    C1: CurveGroup<BaseField = C2::ScalarField, ScalarField = C2::BaseField>,
    C2::BaseField: PrimeField,
{
    type PreprocessorParam: Debug;
    type ProverParam: Clone;
    type Proof;
    type VerifierParam;
    type PublicInput: Debug;
    type CommittedInstance: Clone + Debug;

    fn preprocess(
        rng: impl RngCore + CryptoRng,
        prep_param: Self::PreprocessorParam,
        fs: FS,
    ) -> Result<(Self::ProverParam, Self::VerifierParam), Error>;

    fn prove(
        rng: impl RngCore + CryptoRng,
        pp: Self::ProverParam,
        folding_scheme: FS,
    ) -> Result<Self::Proof, Error>;

    fn verify(
        vp: Self::VerifierParam,
        i: C1::ScalarField,
        z_0: Vec<C1::ScalarField>,
        z_i: Vec<C1::ScalarField>,
        running_instance: &Self::CommittedInstance,
        incoming_instance: &Self::CommittedInstance,
        proof: &Self::Proof,
        // returns `Result<bool, Error>` to differentiate between an error occurred while performing
        // the verification steps, and the verification logic of the scheme not passing.
    ) -> Result<bool, Error>;
}

/// DeciderOnchain extends the Decider into preparing the calldata
pub trait DeciderOnchain<E: Pairing, C1: CurveGroup, C2: CurveGroup>
where
    C1: CurveGroup<BaseField = C2::ScalarField, ScalarField = C2::BaseField>,
    C2::BaseField: PrimeField,
{
    type Proof;
    type CommittedInstance: Clone + Debug;

    fn prepare_calldata(
        i: C1::ScalarField,
        z_0: Vec<C1::ScalarField>,
        z_i: Vec<C1::ScalarField>,
        running_instance: &Self::CommittedInstance,
        incoming_instance: &Self::CommittedInstance,
        proof: Self::Proof,
    ) -> Result<Vec<u8>, Error>;
}
